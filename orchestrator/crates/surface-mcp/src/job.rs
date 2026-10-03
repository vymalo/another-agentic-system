//! What the tools say about a job: the summary of `get_job` and the small answers of the others.
//! Everything here is a function of the thread record and its event log.

use orch_app::{App, AppError, Requester};
use orch_core::{CheckSource, CheckStatus, EventBody, EventKind, ThreadRecord, pull_request_url};
use orch_ports::Ports;
use serde::Serialize;

/// How many of the newest artifacts are looked at for the pull request: one bounded read of the
/// store, whatever the length of the log. A job that produced more artifacts than this after
/// its pull request shows none.
const ARTIFACTS_LOOKED_AT: u32 = 32;
/// Longest texts of the summary that come from outside (a CI provider, an agent, a verifier).
/// They are untrusted, and one tool result must stay small.
const MAX_NAME_CHARS: usize = 256;
const MAX_SUMMARY_CHARS: usize = 1024;
const MAX_FINDING_CHARS: usize = 1024;

/// The branch the agent pushed.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Branch {
    /// The repository, `host/owner/name`.
    pub repository: String,
    /// The branch.
    pub branch: String,
    /// The full commit hash.
    pub commit: String,
}

/// A pull request the agent opened.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PullRequest {
    /// Where it is.
    pub url: String,
}

/// What the last check of a source said.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct LastCheck {
    /// The check's name (a CI check has one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `passed`, `failed` or `pending`.
    pub status: &'static str,
    /// The commit it was about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The source's one-line summary. It is untrusted text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// What one source found wrong. The items are untrusted text.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Findings {
    /// `ci`, `agent_checks` or `verifier`.
    pub source: &'static str,
    /// What is wrong.
    pub items: Vec<String>,
}

/// The result of `get_job`.
#[derive(Debug, Serialize)]
pub struct JobSummary {
    /// The job (the thread) id.
    pub job_id: String,
    /// Its title.
    pub title: String,
    /// The agent it was given to.
    pub agent: String,
    /// Which job of the thread this is, from 1: a message to a finished job starts the next one
    /// (ADR 0020). Everything below is about this job.
    pub job: u32,
    /// `queued`, `working`, `verifying`, `blocked`, `done`, `failed` or `cancelled`.
    pub state: &'static str,
    /// Whether the state is final.
    pub finished: bool,
    /// Why the job waits although the agent finished, when verification could not finish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<&'static str>,
    /// The attempt the agent is on. Only for a job under a gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// The attempts there are. Only for a job under a gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    /// The branch the agent pushed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    /// The pull request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequest>,
    /// The last CI result of this attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<LastCheck>,
    /// What failed checks found, per source.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Findings>,
    /// The sequence number of the last event: the cursor for `after_seq`.
    pub last_seq: i64,
    /// When the job was created.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

fn status(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pending => "pending",
        CheckStatus::Passed => "passed",
        CheckStatus::Failed => "failed",
    }
}

/// `text` cut to at most `max` characters, with an ellipsis where it was cut.
fn cap(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The newest pull request the log mentions, from one bounded read of the newest artifacts.
async fn find_pull_request<P: Ports>(
    app: &App<P>,
    user: &impl Requester,
    thread: &ThreadRecord,
) -> Result<Option<PullRequest>, AppError> {
    let id = thread.id;
    let newest = app
        .latest_events(user, id, EventKind::Artifact, ARTIFACTS_LOOKED_AT)
        .await?;
    // Only this job's: the pull request of an earlier job of the thread is not this job's (ADR
    // 0020). The job started at the last `job_started`; the first job has no such event.
    let since = if thread.job.number > 1 {
        app.latest_events(user, id, EventKind::JobStarted, 1)
            .await?
            .first()
            .map_or(0, |e| e.seq)
    } else {
        0
    };
    Ok(newest_pull_request(&newest, since))
}

/// The newest pull request among `newest` (artifacts, newest first) that came after the event
/// `since` (the job's start; 0 for the first job). Pure.
fn newest_pull_request(newest: &[orch_core::Event], since: i64) -> Option<PullRequest> {
    newest
        .iter()
        .filter(|event| event.seq > since)
        .find_map(|event| {
            let EventBody::Artifact(a) = &event.body else {
                return None;
            };
            pull_request_url(&a.name, a.uri.as_deref(), a.text.as_deref())
                .map(|url| PullRequest { url })
        })
}

/// Summarises `thread`, which belongs to `user`.
pub async fn summarise<P: Ports>(
    app: &App<P>,
    user: &impl Requester,
    thread: &ThreadRecord,
) -> Result<JobSummary, AppError> {
    let pull_request = find_pull_request(app, user, thread).await?;
    Ok(summary_of(thread, pull_request))
}

/// The summary of `thread`, given the pull request its log mentions. Pure.
pub fn summary_of(thread: &ThreadRecord, pull_request: Option<PullRequest>) -> JobSummary {
    let job = &thread.job;
    let gated = job.gate.is_active();
    let ci = job
        .results
        .iter()
        .rev()
        .find(|r| r.source == CheckSource::Ci)
        .map(|r| LastCheck {
            name: r.name.as_deref().map(|n| cap(n, MAX_NAME_CHARS)),
            status: status(r.status),
            commit: r.commit.as_deref().map(|c| cap(c, MAX_NAME_CHARS)),
            summary: r.summary.as_deref().map(|s| cap(s, MAX_SUMMARY_CHARS)),
        });
    let mut findings: Vec<Findings> = Vec::new();
    for result in &job.results {
        if result.status == CheckStatus::Failed && !result.findings.is_empty() {
            findings.push(Findings {
                source: result.source.as_str(),
                items: result
                    .findings
                    .iter()
                    .map(|f| cap(f, MAX_FINDING_CHARS))
                    .collect(),
            });
        }
    }
    JobSummary {
        job_id: thread.id.to_string(),
        title: thread.title.clone(),
        agent: thread.target.agent_id.to_string(),
        job: job.number,
        state: thread.state.as_str(),
        finished: thread.state.is_terminal(),
        hold: job.hold.map(orch_core::Hold::as_str),
        attempt: gated.then_some(job.attempt),
        max_attempts: gated.then(|| job.gate.max()),
        branch: job.pushed.as_ref().map(|p| Branch {
            repository: p.repository.clone(),
            branch: p.branch.clone(),
            commit: p.commit.clone(),
        }),
        pull_request,
        ci,
        findings,
        last_seq: thread.last_seq,
        created_at: thread.created_at.to_string(),
        updated_at: thread.updated_at.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::{
        AgentId, AgentTarget, CheckResult, GatePolicy, Hold, Job, PushedRef, ThreadId, ThreadState,
        UserId,
    };

    use super::*;

    fn record(state: ThreadState, job: Job) -> ThreadRecord {
        ThreadRecord {
            id: ThreadId("00000000-0000-7000-8000-000000000001".parse().unwrap()),
            owner: UserId::new("alice@example.com"),
            title: "fix it".to_owned(),
            description: None,
            target: AgentTarget {
                agent_id: AgentId::new("coder"),
                release: None,
            },
            state,
            job,
            version: 3,
            forked_from: None,
            share: None,
            last_seq: 9,
            created_at: "2026-09-30T10:00:00Z".parse().unwrap(),
            updated_at: "2026-09-30T10:05:00Z".parse().unwrap(),
        }
    }

    fn result(source: CheckSource, status: CheckStatus, findings: &[&str]) -> CheckResult {
        CheckResult {
            source,
            name: (source == CheckSource::Ci).then(|| "build".to_owned()),
            attempt: 1,
            commit: Some("a".repeat(40)),
            status,
            summary: Some("one line".to_owned()),
            stale: false,
            findings: findings.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn pr_event(seq: i64, url: &str) -> orch_core::Event {
        orch_core::Event {
            seq,
            thread_id: ThreadId("00000000-0000-7000-8000-000000000001".parse().unwrap()),
            at: "2026-09-30T10:00:00Z".parse().unwrap(),
            actor: orch_core::Actor::system(),
            body: EventBody::Artifact(orch_core::ArtifactData {
                name: "pull_request".to_owned(),
                mime_type: None,
                uri: Some(url.to_owned()),
                text: None,
                file: None,
            }),
        }
    }

    #[test]
    fn the_pull_request_of_an_earlier_job_is_not_this_jobs() {
        // Newest first: job 2 (started at seq 10) opened none, job 1 opened #1 at seq 4.
        let newest = [pr_event(4, "https://github.com/acme/demo/pull/1")];
        assert_eq!(
            newest_pull_request(&newest, 0).map(|p| p.url),
            Some("https://github.com/acme/demo/pull/1".to_owned())
        );
        assert_eq!(newest_pull_request(&newest, 10), None);
        // The job's own, when it opened one.
        let newest = [
            pr_event(14, "https://github.com/acme/demo/pull/2"),
            pr_event(4, "https://github.com/acme/demo/pull/1"),
        ];
        assert_eq!(
            newest_pull_request(&newest, 10).map(|p| p.url),
            Some("https://github.com/acme/demo/pull/2".to_owned())
        );
    }

    #[test]
    fn what_comes_from_outside_is_cut_to_size() {
        assert_eq!(cap("short", 10), "short");
        let cut = cap(&"é".repeat(50), 10);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));

        let mut job = Job::with_gate(GatePolicy::requiring([CheckSource::Ci]));
        let mut r = result(CheckSource::Ci, CheckStatus::Failed, &[]);
        r.name = Some("n".repeat(5_000));
        r.summary = Some("s".repeat(50_000));
        r.findings = vec!["f".repeat(50_000); 3];
        job.results = vec![r];
        let s = summary_of(&record(ThreadState::Working, job), None);
        let ci = s.ci.as_ref().unwrap();
        assert!(ci.name.as_ref().unwrap().chars().count() <= MAX_NAME_CHARS);
        assert!(ci.summary.as_ref().unwrap().chars().count() <= MAX_SUMMARY_CHARS);
        assert!(
            s.findings[0]
                .items
                .iter()
                .all(|f| f.chars().count() <= MAX_FINDING_CHARS)
        );
    }

    #[test]
    fn a_job_without_a_gate_says_state_and_nothing_about_attempts() {
        let s = summary_of(&record(ThreadState::Working, Job::default()), None);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "job_id": "00000000-0000-7000-8000-000000000001",
                "title": "fix it", "agent": "coder", "job": 1, "state": "working", "finished": false,
                "last_seq": 9,
                "created_at": "2026-09-30T10:00:00Z", "updated_at": "2026-09-30T10:05:00Z"
            })
        );
    }

    #[test]
    fn a_gated_job_says_attempt_branch_last_ci_and_what_failed() {
        let mut job = Job::with_gate(GatePolicy::requiring([
            CheckSource::Ci,
            CheckSource::Verifier,
        ]));
        job.attempt = 2;
        job.pushed = Some(PushedRef {
            repository: "github.com/acme/demo".to_owned(),
            branch: "agent/fix".to_owned(),
            commit: "a".repeat(40),
        });
        job.hold = Some(Hold::CiTimeout);
        job.results = vec![
            result(CheckSource::Ci, CheckStatus::Failed, &["test x failed"]),
            result(CheckSource::Verifier, CheckStatus::Passed, &["noted"]),
            result(CheckSource::Ci, CheckStatus::Passed, &[]),
        ];
        let s = summary_of(
            &record(ThreadState::Blocked, job),
            Some(PullRequest {
                url: "https://github.com/acme/demo/pull/1".to_owned(),
            }),
        );
        assert_eq!((s.attempt, s.max_attempts), (Some(2), Some(3)));
        assert_eq!(s.hold, Some("ci_timeout"));
        assert_eq!(s.branch.as_ref().unwrap().branch, "agent/fix");
        assert_eq!(
            s.pull_request.as_ref().unwrap().url,
            "https://github.com/acme/demo/pull/1"
        );
        // The last CI result, not the first.
        let ci = s.ci.as_ref().unwrap();
        assert_eq!((ci.name.as_deref(), ci.status), (Some("build"), "passed"));
        // Findings only from checks that failed.
        assert_eq!(
            s.findings,
            vec![Findings {
                source: "ci",
                items: vec!["test x failed".to_owned()]
            }]
        );
        assert!(!s.finished);
        assert!(summary_of(&record(ThreadState::Done, Job::default()), None).finished);
    }
}
