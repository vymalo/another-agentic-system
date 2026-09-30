//! The job ledger and the verification gate: the types (pure data, no I/O).
//!
//! One thread holds one job (ADR 0016). The [`Job`] is stored beside the thread state and
//! changes in the same commit; it says what the gate asks for ([`GatePolicy`], copied in when
//! the thread is created, so a configuration change never touches a running job), which attempt
//! the agent is on, what it pushed and what every check has said so far. The decisions are in
//! [`transition`](crate::transition); this module only holds the shapes, the recognition of the
//! two artifacts an agent reports and the rules that keep findings bounded (ADR 0018).

use std::collections::BTreeSet;
use std::fmt;

use jiff::SignedDuration;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AgentId, ThreadId};
use crate::thread::ThreadState;

/// Most findings a source keeps, and so most the rework prompt quotes (ADR 0018).
pub const MAX_FINDINGS: usize = 20;
/// Most bytes of findings a source keeps (ADR 0018).
pub const MAX_FINDINGS_BYTES: usize = 16 * 1024;
/// Most bytes of the user's task the ledger keeps, for the verifier's prompt.
pub const MAX_TASK_BYTES: usize = 8 * 1024;
/// Most bytes of the agent's own summary of its work the ledger keeps, for the verifier's prompt.
pub const MAX_SUMMARY_BYTES: usize = 4 * 1024;
/// Attempts when nothing else is configured.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
/// Seconds CI may take before the thread blocks, when nothing else is configured.
pub const DEFAULT_CI_TIMEOUT_SECS: i64 = 3600;
/// Seconds the verifier may take before the thread blocks, when nothing else is configured.
pub const DEFAULT_VERIFIER_TIMEOUT_SECS: i64 = 1800;

/// Where a verdict on the agent's work comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSource {
    /// Continuous integration on the pushed commit (reported by webhook).
    Ci,
    /// The checks the agent ran itself (its `checks` artifact).
    AgentChecks,
    /// A second agent that reviews the pushed commit.
    Verifier,
}

impl CheckSource {
    /// Every source, in the order of the enum.
    pub const ALL: [CheckSource; 3] = [
        CheckSource::Ci,
        CheckSource::AgentChecks,
        CheckSource::Verifier,
    ];

    /// The spelling configuration uses (`ORCH_GATE`, `AGENTS_FILE`, `forwardedProps`): the wire
    /// spelling of [`as_str`](Self::as_str) with dashes. Input accepts both
    /// ([`from_config_name`](Self::from_config_name)); the API only emits the wire spelling.
    pub fn config_name(self) -> &'static str {
        match self {
            CheckSource::Ci => "ci",
            CheckSource::AgentChecks => "agent-checks",
            CheckSource::Verifier => "verifier",
        }
    }

    /// The source a spelling names: the configuration spelling (`agent-checks`) or the wire
    /// spelling the API emits (`agent_checks`), so a `gate` read from `Thread.job` or a
    /// `check_result` can be sent back as a request.
    pub fn from_config_name(name: &str) -> Option<CheckSource> {
        CheckSource::ALL
            .into_iter()
            .find(|source| source.config_name() == name || source.as_str() == name)
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            CheckSource::Ci => "ci",
            CheckSource::AgentChecks => "agent_checks",
            CheckSource::Verifier => "verifier",
        }
    }

    /// A name for people, used in prompts and messages.
    pub fn label(self) -> &'static str {
        match self {
            CheckSource::Ci => "CI",
            CheckSource::AgentChecks => "the agent's own checks",
            CheckSource::Verifier => "the verifier",
        }
    }
}

/// What CI must say. `required` names the checks that count. A gate that requires CI must name at
/// least one (configuration refuses it otherwise); with none, nothing counts and the job ends
/// `Blocked` by the CI deadline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CiPolicy {
    /// The names of the checks that must pass.
    pub required: BTreeSet<String>,
    /// How long CI may take before the thread blocks (`ci_timeout`). It does not use an attempt.
    pub timeout: SignedDuration,
}

impl Default for CiPolicy {
    fn default() -> Self {
        CiPolicy {
            required: BTreeSet::new(),
            timeout: SignedDuration::from_secs(DEFAULT_CI_TIMEOUT_SECS),
        }
    }
}

/// What has to be true before a thread may be `done`. An empty `require` is no gate: the agent
/// completing is enough, exactly as before the gate existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GatePolicy {
    /// The sources that must all pass.
    pub require: BTreeSet<CheckSource>,
    /// Attempts the agent gets, the first included (at least 1).
    pub max_attempts: u32,
    /// The rules for [`CheckSource::Ci`].
    pub ci: CiPolicy,
    /// The agent that verifies, when [`CheckSource::Verifier`] is required.
    pub verifier: Option<AgentId>,
    /// How long the verifier may take before the thread blocks.
    pub verifier_timeout: SignedDuration,
}

impl Default for GatePolicy {
    /// No gate.
    fn default() -> Self {
        GatePolicy {
            require: BTreeSet::new(),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            ci: CiPolicy::default(),
            verifier: None,
            verifier_timeout: SignedDuration::from_secs(DEFAULT_VERIFIER_TIMEOUT_SECS),
        }
    }
}

impl GatePolicy {
    /// A gate that requires `sources`, with the default attempts and timeouts.
    pub fn requiring(sources: impl IntoIterator<Item = CheckSource>) -> Self {
        GatePolicy {
            require: sources.into_iter().collect(),
            ..GatePolicy::default()
        }
    }

    /// Sets how long the verifier may take, in whole seconds (clamped to at least one).
    pub fn set_verifier_timeout_secs(&mut self, secs: i64) {
        self.verifier_timeout = SignedDuration::from_secs(secs.max(1));
    }

    /// Whether any source is required. When not, the job is never touched.
    pub fn is_active(&self) -> bool {
        !self.require.is_empty()
    }

    /// Whether `source` must pass.
    pub fn requires(&self, source: CheckSource) -> bool {
        self.require.contains(&source)
    }

    /// The attempts the agent gets: `max_attempts`, but never fewer than one.
    pub fn max(&self) -> u32 {
        self.max_attempts.max(1)
    }
}

/// The commit an agent pushed: what CI runs on and the verifier reviews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushedRef {
    /// The repository as [`repo_key`] normalises it: `host/owner/name`, lower case.
    pub repository: String,
    /// The branch.
    pub branch: String,
    /// The full commit hash, lower case.
    pub commit: String,
}

/// What a check said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Not answered yet.
    Pending,
    /// Passed.
    Passed,
    /// Failed.
    Failed,
}

/// One answer of one source, for one attempt. It is the payload of a `check_result` event and
/// the entry the ledger keeps; the ledger may hold several entries per source (one per CI check
/// name), the log carries the source's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    /// Who answered.
    pub source: CheckSource,
    /// The CI check this is, for a CI entry of the ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The attempt it belongs to (from 1).
    pub attempt: u32,
    /// The commit it was about, when the source named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The answer.
    pub status: CheckStatus,
    /// A one-line summary from the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Set on a `check_result` event that arrived for a verification that is no longer the
    /// current one (or a second time): it is in the log for the record and decided nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// What is wrong, at most [`MAX_FINDINGS`] items and [`MAX_FINDINGS_BYTES`] bytes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
}

/// Why a thread that was being verified waits for the user. A user message clears it and
/// re-delegates without using an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// CI did not report within [`CiPolicy::timeout`].
    CiTimeout,
    /// The verifier did not answer within [`GatePolicy::verifier_timeout`].
    VerifierTimeout,
    /// The verifier could not be reached (its delivery was dead-lettered).
    VerifierFailed,
}

impl Hold {
    /// The wire spelling; `ci_timeout` is the interrupt reason of the projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Hold::CiTimeout => "ci_timeout",
            Hold::VerifierTimeout => "verifier_timeout",
            Hold::VerifierFailed => "verifier_failed",
        }
    }
}

/// The ledger of a thread's job. `Job::default()` (and the database default `{}`) is a job with
/// no gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Job {
    /// The policy this job runs under.
    pub gate: GatePolicy,
    /// The attempt the agent is on, from 1; never above `gate.max()`.
    pub attempt: u32,
    /// How many verifications have started. A report or timer of an earlier one is stale even
    /// when the attempt is the same (a user message abandons a verification without using an
    /// attempt).
    pub verification: u32,
    /// The user's task, kept (capped) for the verifier's prompt. Only kept under an active gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// What the agent said about its work in this attempt (its last final message, or the text
    /// of its `completed`), kept (capped) for the verifier's prompt as untrusted data. Only kept
    /// under a gate that requires the verifier; a rework forgets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The commit the agent pushed, once it said so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pushed: Option<PushedRef>,
    /// What the sources said in this attempt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<CheckResult>,
    /// Why the thread waits, when verification could not finish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
}

impl Default for Job {
    fn default() -> Self {
        Job {
            gate: GatePolicy::default(),
            attempt: 1,
            verification: 0,
            task: None,
            summary: None,
            pushed: None,
            results: Vec::new(),
            hold: None,
        }
    }
}

impl Job {
    /// A job under `gate`.
    pub fn with_gate(gate: GatePolicy) -> Self {
        Job {
            gate,
            ..Job::default()
        }
    }
}

/// What a client is told about a job: `Thread.job` of the resource API and `job` in the AG-UI
/// `STATE_SNAPSHOT`. Only a job under an active gate has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    /// The attempt the agent is on, from 1.
    pub attempt: u32,
    /// The attempts there are.
    pub max_attempts: u32,
    /// The sources the gate requires, in the order of [`CheckSource`].
    pub gate: Vec<CheckSource>,
    /// The commit the agent pushed in this attempt, once it said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

impl JobView {
    /// The view of a job under `gate`, on `attempt`, that pushed `sha`.
    pub fn new(gate: &GatePolicy, attempt: u32, sha: Option<String>) -> Self {
        JobView {
            attempt,
            max_attempts: gate.max(),
            gate: gate.require.iter().copied().collect(),
            sha,
        }
    }
}

impl Job {
    /// What clients are told about this job; `None` when the gate requires nothing.
    pub fn view(&self) -> Option<JobView> {
        self.gate.is_active().then(|| {
            JobView::new(
                &self.gate,
                self.attempt,
                self.pushed.as_ref().map(|p| p.commit.clone()),
            )
        })
    }
}

/// A thread's state and job: what [`transition`](crate::transition) takes and returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The thread state.
    pub state: ThreadState,
    /// The job ledger.
    pub job: Job,
}

impl Snapshot {
    /// `state` with a job that has no gate.
    pub fn new(state: ThreadState) -> Self {
        Snapshot {
            state,
            job: Job::default(),
        }
    }

    /// A new thread: `queued`, under `gate`.
    pub fn queued(gate: GatePolicy) -> Self {
        Snapshot {
            state: ThreadState::Queued,
            job: Job::with_gate(gate),
        }
    }
}

/// What a CI provider concluded. Closed: `success`, `neutral` and `skipped` pass, everything
/// else fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiConclusion {
    /// Passed.
    Success,
    /// Neither passed nor failed; counts as a pass.
    Neutral,
    /// Not run; counts as a pass.
    Skipped,
    /// Failed.
    Failure,
    /// Cancelled before it finished.
    Cancelled,
    /// Ran out of time.
    TimedOut,
    /// Waits for someone to act.
    ActionRequired,
    /// Superseded.
    Stale,
    /// Could not start.
    StartupFailure,
}

impl CiConclusion {
    /// Whether this counts as a pass.
    pub fn passes(self) -> bool {
        match self {
            CiConclusion::Success | CiConclusion::Neutral | CiConclusion::Skipped => true,
            CiConclusion::Failure
            | CiConclusion::Cancelled
            | CiConclusion::TimedOut
            | CiConclusion::ActionRequired
            | CiConclusion::Stale
            | CiConclusion::StartupFailure => false,
        }
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            CiConclusion::Success => "success",
            CiConclusion::Neutral => "neutral",
            CiConclusion::Skipped => "skipped",
            CiConclusion::Failure => "failure",
            CiConclusion::Cancelled => "cancelled",
            CiConclusion::TimedOut => "timed_out",
            CiConclusion::ActionRequired => "action_required",
            CiConclusion::Stale => "stale",
            CiConclusion::StartupFailure => "startup_failure",
        }
    }
}

/// Which adapter normalised a CI report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiProvider {
    /// A GitHub webhook.
    Github,
    /// The generic signed webhook.
    Generic,
}

/// A completed CI check on a commit, already normalised by a surface. It is the input
/// [`Input::CiReported`](crate::Input::CiReported) carries and the payload of a `ci_result`
/// event (the card the chat shows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiReport {
    /// Who reported.
    pub provider: CiProvider,
    /// The repository, as [`repo_key`] normalises it.
    pub repository: String,
    /// The full commit hash, lower case.
    pub sha: String,
    /// The branch, when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The name of the check, app or workflow.
    pub name: String,
    /// What it concluded.
    pub conclusion: CiConclusion,
    /// A link to the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A summary from the provider. It is untrusted text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// The verifier's answer, from its `verdict` artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// Whether the work is acceptable.
    pub passed: bool,
    /// What is wrong (or, when it passed, what it noted).
    #[serde(default)]
    pub findings: Vec<String>,
}

impl Verdict {
    /// The verdict of a verifier that finished without giving one: a failed check
    /// (fail closed, ADR 0018).
    pub fn missing() -> Self {
        Verdict {
            passed: false,
            findings: vec![
                "no verdict: the verifier finished without reporting a `verdict` artifact"
                    .to_owned(),
            ],
        }
    }

    /// The verdict of a verifier whose `verdict` artifact cannot be used (`reason` says why): a
    /// failed check as well.
    pub fn unusable(reason: &str) -> Self {
        Verdict {
            passed: false,
            findings: cap_findings([format!(
                "no verdict: the verifier's `verdict` artifact cannot be used: {reason}"
            )]),
        }
    }
}

/// Reads a `verdict` artifact `{passed, findings[]}` from its inline JSON text (an A2A data part
/// arrives as JSON text). Pure. The verifier is another agent and its words are untrusted, so
/// the findings are bounded here ([`cap_findings`]: at most [`MAX_FINDINGS`] items and
/// [`MAX_FINDINGS_BYTES`] bytes); a `findings` that is not a list, or a `passed` that is not a
/// boolean, makes the artifact unusable rather than guessed at.
///
/// ```
/// # use orch_core::parse_verdict;
/// let v = parse_verdict(Some(r#"{"passed": false, "findings": ["no tests"]}"#)).unwrap();
/// assert!(!v.passed);
/// assert_eq!(v.findings, ["no tests"]);
/// assert!(parse_verdict(Some(r#"{"findings": []}"#)).is_err());
/// ```
pub fn parse_verdict(text: Option<&str>) -> Result<Verdict, String> {
    let Some(text) = text else {
        return Err("it carries no data".to_owned());
    };
    let value: Value = serde_json::from_str(text).map_err(|e| format!("it is not JSON: {e}"))?;
    let Some(object) = value.as_object() else {
        return Err("it is not a JSON object".to_owned());
    };
    let Some(passed) = object.get("passed").and_then(Value::as_bool) else {
        return Err("`passed` is missing or not a boolean".to_owned());
    };
    let findings = match object.get("findings") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => cap_findings(items.iter().map(|item| match item {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })),
        Some(_) => return Err("`findings` is not a list".to_owned()),
    };
    Ok(Verdict { passed, findings })
}

/// A deadline the application arms and feeds back as [`Input::TimerFired`](crate::Input::TimerFired).
/// It names the verification it belongs to, so one that fires late is recognised as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Timer {
    /// CI must have reported by now.
    CiDeadline {
        /// The attempt it was armed in.
        attempt: u32,
        /// The verification it was armed in.
        verification: u32,
    },
    /// The verifier must have answered by now.
    VerifierDeadline {
        /// The attempt it was armed in.
        attempt: u32,
        /// The verification it was armed in.
        verification: u32,
    },
}

/// The A2A context a verification runs in: `<thread>-verify-<attempt>-<verification>`. It is the
/// verifier's own, separate from the worker's (the thread's context), and it is new for every
/// verification, so a verification that repeats within one attempt (the user answered a hold, the
/// agent finished again) never lands in the context of one that is over (ADR 0018).
pub fn verifier_context(thread: ThreadId, attempt: u32, verification: u32) -> String {
    format!("{thread}-verify-{attempt}-{verification}")
}

/// The key an inbound CI report is matched by: `ci:<repo-key>@<sha>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WatchKey(String);

impl WatchKey {
    /// The key of CI on `sha` in `repository` (a [`repo_key`]).
    pub fn ci(repository: &str, sha: &str) -> Self {
        WatchKey(format!("ci:{repository}@{sha}"))
    }

    /// The key as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WatchKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Findings of one source, for the `rework` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFindings {
    /// Who found them.
    pub source: CheckSource,
    /// What they found.
    pub findings: Vec<String>,
}

/// `data` of a `rework` event: the agent is sent back to work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReworkData {
    /// The attempt that starts now.
    pub attempt: u32,
    /// The attempts there are.
    pub max_attempts: u32,
    /// Why, per failed source.
    pub findings: Vec<SourceFindings>,
}

// ---- repository keys ---------------------------------------------------------------------

/// Normalises a repository address to `host/owner/name`: lower case, no scheme, credentials,
/// query, trailing slash or `.git`, the default port dropped. Accepts `https://` and `ssh://`
/// URLs and the `git@host:owner/name` form. `None` when there is no host or fewer than two path
/// segments.
///
/// ```
/// # use orch_core::repo_key;
/// assert_eq!(repo_key("https://GitHub.com/Vymalo/Repo.git").as_deref(), Some("github.com/vymalo/repo"));
/// assert_eq!(repo_key("git@github.com:vymalo/repo.git").as_deref(), Some("github.com/vymalo/repo"));
/// ```
pub fn repo_key(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_lowercase();
    if lowered.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (scheme, rest) = match lowered.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_owned()), rest.to_owned()),
        None => (None, lowered),
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    // `user@host:path` without a scheme is the scp form; `host:8080/path` is a port.
    let rest = match scheme {
        None => scp_to_path(rest),
        Some(_) => rest.to_owned(),
    };
    let mut parts = rest.split('/');
    let authority = parts.next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = drop_default_port(host, scheme.as_deref());
    if host.is_empty() {
        return None;
    }
    let mut segments: Vec<&str> = parts.filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| *s == "." || *s == "..") {
        return None;
    }
    let last = segments.last_mut()?;
    if let Some(stripped) = last.strip_suffix(".git") {
        *last = stripped;
    }
    if segments.len() < 2 || segments.iter().any(|s| s.is_empty()) {
        return None;
    }
    Some(format!("{host}/{}", segments.join("/")))
}

/// `user@host:owner/name` becomes `user@host/owner/name`; anything that already has a port or
/// no colon in its authority is left alone.
fn scp_to_path(rest: &str) -> String {
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    match authority.split_once(':') {
        Some((host, after)) if !after.is_empty() && !after.chars().all(|c| c.is_ascii_digit()) => {
            // The scp form puts the path after the colon: `host:owner/name`.
            let joined = if path.is_empty() {
                after.to_owned()
            } else {
                format!("{after}/{path}")
            };
            format!("{host}/{joined}")
        }
        Some(_) | None => rest.to_owned(),
    }
}

fn drop_default_port<'a>(host: &'a str, scheme: Option<&str>) -> &'a str {
    let default = match scheme {
        Some("https") => ":443",
        Some("http") => ":80",
        Some("ssh") => ":22",
        Some(_) | None => "",
    };
    if default.is_empty() {
        host
    } else {
        host.strip_suffix(default).unwrap_or(host)
    }
}

/// The longest branch name taken from an artifact.
pub const MAX_BRANCH_BYTES: usize = 255;

/// Whether `name` is a branch name `git check-ref-format --branch` accepts (which also refuses a
/// name that starts with `-`, since a command line would read it as an option). The name comes
/// from the worker and is repeated to other agents and to users, so what git forbids is
/// forbidden here: no whitespace or control characters, none of
/// `` ` `` `~` `^` `:` `?` `*` `[` `\`, no `..` or `@{`, no empty component (`//`, a leading or
/// trailing `/`), no component that starts with `.` or ends with `.lock`, no trailing `.`, not
/// `@`, at most [`MAX_BRANCH_BYTES`] bytes.
///
/// ```
/// # use orch_core::is_branch_name;
/// assert!(is_branch_name("agent/fix-login"));
/// assert!(!is_branch_name("agent/fix login"));
/// assert!(!is_branch_name("a..b"));
/// ```
pub fn is_branch_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_BRANCH_BYTES
        || name == "@"
        || name.starts_with('-')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
    {
        return false;
    }
    if name.chars().any(|c| {
        c.is_control()
            || c.is_whitespace()
            || matches!(c, '`' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return false;
    }
    name.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// Whether `s` is a full commit hash: 40 or 64 lower-case hex digits.
pub fn is_commit_hash(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ---- findings ----------------------------------------------------------------------------

const OMITTED_RESERVE: usize = 48;

/// Bounds findings the way ADR 0018 says: at most [`MAX_FINDINGS`] items and
/// [`MAX_FINDINGS_BYTES`] bytes. When something is cut, the last item says how many were left
/// out. Empty items are dropped.
pub fn cap_findings<I, S>(findings: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let all: Vec<String> = findings
        .into_iter()
        .map(Into::into)
        .filter(|f| !f.trim().is_empty())
        .collect();
    let total_bytes: usize = all.iter().map(String::len).sum();
    if all.len() <= MAX_FINDINGS && total_bytes <= MAX_FINDINGS_BYTES {
        return all;
    }
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0_usize;
    let budget = MAX_FINDINGS_BYTES - OMITTED_RESERVE;
    for item in &all {
        if kept.len() >= MAX_FINDINGS - 1 || used >= budget {
            break;
        }
        let piece = truncate_to(item, budget - used);
        used += piece.len();
        if piece.len() < item.len() {
            kept.push(format!("{piece} [cut]"));
            break;
        }
        kept.push(piece.to_owned());
    }
    let left = all.len() - kept.len();
    if left > 0 {
        kept.push(format!("[{left} more findings omitted]"));
    }
    kept
}

/// The longest prefix of `s` of at most `max` bytes that ends on a character boundary.
pub(crate) fn truncate_to(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---- artifact recognition ----------------------------------------------------------------

/// A `checks` artifact: the agent ran its own checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksReport {
    /// Whether they passed.
    pub passed: bool,
    /// The commit they ran on.
    pub commit: String,
    /// A one-line summary.
    pub summary: Option<String>,
    /// What failed, bounded by [`cap_findings`].
    pub findings: Vec<String>,
}

/// What an agent's artifact means to the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognised {
    /// `branch {repository, branch, commit}`: the agent pushed.
    Branch(PushedRef),
    /// `checks {passed, commit, summary?, findings?}`: the agent's own checks.
    Checks(ChecksReport),
    /// One of the two names, but the payload cannot be used.
    Malformed {
        /// Which artifact.
        artifact: KnownArtifact,
        /// What is wrong, worded for the log.
        reason: String,
    },
    /// Any other artifact: the gate has no opinion.
    Other,
}

/// The artifact names the gate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownArtifact {
    /// `branch`.
    Branch,
    /// `checks`.
    Checks,
}

/// Reads the artifact `name` with the inline JSON `text` (an A2A data part arrives as JSON
/// text). Pure: nothing is looked up.
pub fn recognise_artifact(name: &str, text: Option<&str>) -> Recognised {
    let artifact = match name {
        "branch" => KnownArtifact::Branch,
        "checks" => KnownArtifact::Checks,
        _ => return Recognised::Other,
    };
    let malformed = |reason: String| Recognised::Malformed { artifact, reason };
    let Some(text) = text else {
        return malformed("it carries no data".to_owned());
    };
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return malformed(format!("it is not JSON: {e}")),
    };
    let Some(object) = value.as_object() else {
        return malformed("it is not a JSON object".to_owned());
    };
    let string = |key: &str| object.get(key).and_then(Value::as_str);
    match artifact {
        KnownArtifact::Branch => {
            let Some(repository) = string("repository").and_then(repo_key) else {
                return malformed("`repository` is missing or not a repository address".to_owned());
            };
            let Some(branch) = string("branch").map(str::trim).filter(|b| !b.is_empty()) else {
                return malformed("`branch` is missing".to_owned());
            };
            if !is_branch_name(branch) {
                return malformed(
                    "`branch` is not a name git accepts for a branch (git check-ref-format)"
                        .to_owned(),
                );
            }
            let Some(commit) = string("commit").map(str::to_lowercase) else {
                return malformed("`commit` is missing".to_owned());
            };
            if !is_commit_hash(&commit) {
                return malformed("`commit` is not a full commit hash".to_owned());
            }
            Recognised::Branch(PushedRef {
                repository,
                branch: branch.to_owned(),
                commit,
            })
        }
        KnownArtifact::Checks => {
            let Some(passed) = object.get("passed").and_then(Value::as_bool) else {
                return malformed("`passed` is missing or not a boolean".to_owned());
            };
            let Some(commit) = string("commit").map(str::to_lowercase) else {
                return malformed("`commit` is missing".to_owned());
            };
            if !is_commit_hash(&commit) {
                return malformed("`commit` is not a full commit hash".to_owned());
            }
            let findings = match object.get("findings") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => cap_findings(items.iter().map(|item| match item {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })),
                Some(_) => return malformed("`findings` is not a list".to_owned()),
            };
            Recognised::Checks(ChecksReport {
                passed,
                commit,
                summary: string("summary")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| truncate_to(s, 1024).to_owned()),
                findings,
            })
        }
    }
}
