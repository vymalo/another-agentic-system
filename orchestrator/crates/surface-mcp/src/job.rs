//! What the tools say about a job: the summary of `get_job` and the small answers of the others.
//! Everything here is a function of the thread record and its event log.

use orch_app::{App, AppError};
use orch_core::{CheckSource, CheckStatus, EventBody, ThreadId, ThreadRecord, UserId};
use orch_ports::Ports;
use serde::Serialize;

/// Events read per page when looking for the pull request.
const PAGE: u32 = 500;
/// Pages read at most: a job with a log longer than this shows no pull request, rather than
/// making one `get_job` read an unbounded log.
const MAX_PAGES: u32 = 20;

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

/// The URL of a pull request artifact: the agents name it `pull_request` (a data part whose JSON
/// has a `url`) or "Pull request" (a url part), and the URL is in `uri` or in the JSON text.
fn pull_request_url(name: &str, uri: Option<&str>, text: Option<&str>) -> Option<String> {
    let name: String = name
        .chars()
        .map(|c| {
            if c == '_' || c == '-' {
                ' '
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    if name.trim() != "pull request" {
        return None;
    }
    let from_json = || {
        let value: serde_json::Value = serde_json::from_str(text?).ok()?;
        ["url", "html_url"]
            .iter()
            .find_map(|key| value.get(key)?.as_str().map(str::to_owned))
    };
    uri.map(str::to_owned).or_else(from_json)
}

/// The last pull request the log mentions.
async fn find_pull_request<P: Ports>(
    app: &App<P>,
    user: &UserId,
    id: ThreadId,
) -> Result<Option<PullRequest>, AppError> {
    let mut found = None;
    let mut after = 0;
    for _ in 0..MAX_PAGES {
        let page = app.list_events(user, id, after, PAGE).await?;
        for event in &page {
            if let EventBody::Artifact(a) = &event.body
                && let Some(url) = pull_request_url(&a.name, a.uri.as_deref(), a.text.as_deref())
            {
                found = Some(PullRequest { url });
            }
        }
        match page.last() {
            Some(last) if page.len() == PAGE as usize => after = last.seq,
            _ => break,
        }
    }
    Ok(found)
}

/// Summarises `thread`, which belongs to `user`.
pub async fn summarise<P: Ports>(
    app: &App<P>,
    user: &UserId,
    thread: &ThreadRecord,
) -> Result<JobSummary, AppError> {
    let pull_request = find_pull_request(app, user, thread.id).await?;
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
            name: r.name.clone(),
            status: status(r.status),
            commit: r.commit.clone(),
            summary: r.summary.clone(),
        });
    let mut findings: Vec<Findings> = Vec::new();
    for result in &job.results {
        if result.status == CheckStatus::Failed && !result.findings.is_empty() {
            findings.push(Findings {
                source: result.source.as_str(),
                items: result.findings.clone(),
            });
        }
    }
    JobSummary {
        job_id: thread.id.to_string(),
        title: thread.title.clone(),
        agent: thread.target.agent_id.to_string(),
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
        AgentId, AgentTarget, CheckResult, GatePolicy, Hold, Job, PushedRef, ThreadState,
    };

    use super::*;

    fn record(state: ThreadState, job: Job) -> ThreadRecord {
        ThreadRecord {
            id: ThreadId("00000000-0000-7000-8000-000000000001".parse().unwrap()),
            owner: UserId::new("alice@example.com"),
            title: "fix it".to_owned(),
            target: AgentTarget {
                agent_id: AgentId::new("coder"),
                release: None,
            },
            state,
            job,
            version: 3,
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

    #[test]
    fn a_job_without_a_gate_says_state_and_nothing_about_attempts() {
        let s = summary_of(&record(ThreadState::Working, Job::default()), None);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "job_id": "00000000-0000-7000-8000-000000000001",
                "title": "fix it", "agent": "coder", "state": "working", "finished": false,
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

    #[test]
    fn a_pull_request_artifact_is_recognised_by_name_from_either_agent_shape() {
        let url = "https://github.com/acme/demo/pull/1";
        // The mock agent: a url part.
        assert_eq!(
            pull_request_url("Pull request", Some(url), None).as_deref(),
            Some(url)
        );
        // adam-coder: a data part, JSON in the text.
        assert_eq!(
            pull_request_url("pull_request", None, Some(&format!(r#"{{"url":"{url}"}}"#)))
                .as_deref(),
            Some(url)
        );
        assert_eq!(pull_request_url("branch", Some(url), None), None);
        assert_eq!(
            pull_request_url("pull_request", None, Some("not json")),
            None
        );
        assert_eq!(pull_request_url("pull_request", None, None), None);
    }
}
