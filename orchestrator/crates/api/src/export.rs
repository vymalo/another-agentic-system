//! The thread export document (`GET /api/threads/{id}/export`): one JSON file that holds what a
//! developer needs to see what happened in a chat, without access to the deployment.
//!
//! The document is versioned. `version` moves only when a member changes meaning or goes away;
//! adding a member does not move it, so a reader ignores what it does not know.

use std::collections::BTreeMap;

use orch_app::ThreadExport;
use orch_core::{
    ActorType, AgentBuild, AgentId, AgentTaskState, Event, EventBody, Job, ThreadRecord, Timestamp,
};
use serde::Serialize;

/// The `format` member: what kind of file this is.
pub const FORMAT: &str = "another-agentic-system/thread-export";
/// The `version` member: the document's shape, from 1.
pub const VERSION: u32 = 1;

/// What a member that says a build says when it cannot: the build is not known.
pub const UNKNOWN: &str = "unknown";

/// The revision of this orchestrator build: the commit it was built from, which the image build
/// passes as `ORCH_BUILD_REVISION` (`orchestrator/Dockerfile`), and [`UNKNOWN`] for a build that
/// was given none (a local `cargo build`).
pub const ORCHESTRATOR_REVISION: &str = match option_env!("ORCH_BUILD_REVISION") {
    Some(revision) if !revision.is_empty() => revision,
    _ => UNKNOWN,
};

/// The request header the web sends its revision in.
pub const WEB_REVISION_HEADER: &str = "x-web-revision";

/// The most bytes of a web revision that are kept (it is a header of the caller, so untrusted).
const MAX_WEB_REVISION_BYTES: usize = 64;

/// The revision of the web that asked for the export, from the header `X-Web-Revision` that the
/// web sends with the request (ADR 0053): kept when it is 1 to 64 characters of letters, digits
/// and `._+-`, [`UNKNOWN`] otherwise. The header is the caller's word, like a `User-Agent`: it
/// authorises nothing and is only written into the file the caller downloads.
pub fn web_revision(header: Option<&str>) -> String {
    header
        .map(str::trim)
        .filter(|r| {
            !r.is_empty()
                && r.len() <= MAX_WEB_REVISION_BYTES
                && r.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        })
        .map_or_else(|| UNKNOWN.to_owned(), str::to_owned)
}

/// The document for an export, borrowed from it: serialising it writes the file straight from the
/// thread and its events, with no copy of the log in between.
///
/// * `thread`: the contract `Thread` (what `GET /api/threads/{id}` answers), so a reader that
///   knows the API knows this member.
/// * `job`: the whole job ledger, which `Thread.job` only summarises (and omits without a
///   gate): `number` (which job of the thread this is, ADR 0020), the gate policy, the attempt,
///   the verification counter, the pushed commit, every result of the current attempt, any
///   hold, and `afterStop` (the text the next job starts with while a Stop & send is landing,
///   ADR 0036; absent unless the thread is stopping). A job without a gate is `{}` apart from the defaults, exactly as the store keeps it.
/// * `binding`: the A2A agent, context and task the thread is bound to.
/// * `events`: the append-only log in order, each exactly as the contract `Event` and the store
///   serialise it. Every card and line of the chat is derived from it.
/// * `versions`: which builds made the thread (ADR 0053): `orchestrator` `{version, revision}` (the
///   commit this build was made from, or `unknown`), `agents` (every agent that worked in the
///   thread, with the card's `name` and `version` as the orchestrator read them when it gave the
///   agent work, and the card's `build` parameters when it has them: `version` is `unknown` for an
///   agent whose card was never recorded, as in every thread older than the field), and `web`
///   `{revision}` (the build of the web that asked for the file, from its `X-Web-Revision` header,
///   or `unknown`). A member of it that is not known says `unknown`; none is ever missing.
/// * `eventsTruncated`: `true` when the log was longer than the export reads (it reads the head
///   of the log, up to a count of events and a number of bytes).
///
/// The owner's identity is the `owner` of `thread` (as `GET /api/threads/{id}` says it), and in the
/// log, as the `actor.name` of the owner's messages.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document<'a> {
    format: &'static str,
    version: u32,
    exported_at: &'a Timestamp,
    thread: &'a ThreadRecord,
    job: &'a Job,
    binding: Option<Binding<'a>>,
    events: &'a [Event],
    events_truncated: bool,
    versions: Versions,
}

/// The `versions` member.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Versions {
    orchestrator: OrchestratorBuild,
    agents: Vec<AgentVersion>,
    web: WebBuild,
}

#[derive(Debug, Serialize)]
struct OrchestratorBuild {
    version: &'static str,
    revision: &'static str,
}

#[derive(Debug, Serialize)]
struct WebBuild {
    revision: String,
}

/// One agent build, or an agent whose build is unknown.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentVersion {
    agent: String,
    name: Option<String>,
    version: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    build: BTreeMap<String, String>,
    /// The first job of the thread this build worked in; absent when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_job: Option<u32>,
}

impl From<&AgentBuild> for AgentVersion {
    fn from(b: &AgentBuild) -> Self {
        AgentVersion {
            agent: b.agent.to_string(),
            name: b.name.clone(),
            version: b.version.clone().unwrap_or_else(|| UNKNOWN.to_owned()),
            build: b.build.clone(),
            from_job: Some(b.job),
        }
    }
}

/// The agents that worked in the thread, each with the builds recorded for it, or once with
/// `version: unknown` when none was: the agent of the binding first, then the actors of the log and
/// the agents it says were asked, in the order the log names them.
fn agent_versions(export: &ThreadExport) -> Vec<AgentVersion> {
    let mut agents: Vec<String> = Vec::new();
    let mut note = |name: &str| {
        if !agents.iter().any(|a| a == name) {
            agents.push(name.to_owned());
        }
    };
    if let Some(binding) = &export.binding {
        note(binding.agent_id.as_str());
    }
    for event in &export.events {
        if event.actor.r#type == ActorType::Agent {
            note(&event.actor.name);
        }
        if let EventBody::AskStarted(ask) = &event.body {
            note(ask.agent.as_str());
        }
    }
    for build in &export.thread.job.builds {
        note(build.agent.as_str());
    }
    let mut out = Vec::new();
    for agent in agents {
        let recorded: Vec<AgentVersion> = export
            .thread
            .job
            .builds
            .iter()
            .filter(|b| b.agent.as_str() == agent)
            .map(AgentVersion::from)
            .collect();
        if recorded.is_empty() {
            out.push(AgentVersion {
                agent,
                name: None,
                version: UNKNOWN.to_owned(),
                build: BTreeMap::new(),
                from_job: None,
            });
        } else {
            out.extend(recorded);
        }
    }
    out
}

/// The `binding` member.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Binding<'a> {
    agent_id: &'a AgentId,
    context_id: Option<&'a str>,
    task_id: Option<&'a str>,
    task_state: Option<AgentTaskState>,
    revision: Option<&'a str>,
}

/// The document for `export`, asked for by the web whose revision is `web_revision` (the caller
/// passes it through [`web_revision`]).
pub fn document(export: &ThreadExport, web_revision: String) -> Document<'_> {
    Document {
        format: FORMAT,
        version: VERSION,
        exported_at: &export.exported_at,
        thread: &export.thread,
        job: &export.thread.job,
        binding: export.binding.as_ref().map(|b| Binding {
            agent_id: &b.agent_id,
            context_id: b.context_id.as_deref(),
            task_id: b.task_id.as_deref(),
            task_state: b.task_state,
            revision: b.revision.as_deref(),
        }),
        events: &export.events,
        events_truncated: export.truncated,
        versions: Versions {
            orchestrator: OrchestratorBuild {
                version: env!("CARGO_PKG_VERSION"),
                revision: ORCHESTRATOR_REVISION,
            },
            agents: agent_versions(export),
            web: WebBuild {
                revision: web_revision,
            },
        },
    }
}
