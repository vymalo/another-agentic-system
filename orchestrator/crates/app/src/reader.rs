//! The reader projection (ADR 0040, section 7): what a person who reads a shared thread through its
//! link is allowed to see of it.
//!
//! A set of pure functions turns the thread's log, event by event, into the log a reader may have,
//! and the thread's record into the view a reader is given. The HTTP and AG-UI layers only serve
//! what comes out. It is built per audience ([`ReaderAudience`]) and tested as a table:
//!
//! | | internal reader | public reader |
//! |---|---|---|
//! | messages, answers, cards, steps' labels and states, the job's ledger, title, description | yes | yes |
//! | step input and output | yes | no, unless `sharing.public.stepIo` |
//! | files | yes (the route asks `artifact.read`) | no, unless `sharing.public.files` |
//! | the owner's e-mail (the thread's owner, the actor of a message) | no: "the owner" | no: "the owner" |
//! | fork markers, the parent's id, the UI catalog, the sharing events themselves | no | no |
//!
//! **An event a reader may not see is not dropped: it is replaced by an inert one with the same
//! `seq`** ([`inert`]), so the log's numbering and a client's resume cursor stay valid and a stream
//! that has read the whole log knows it has. The AG-UI projection says nothing for the inert event.
//!
//! What this cannot scrub is free text: a message in which the owner pasted an address or a secret
//! is shown as written, which is why the dialog warns and why step input and output, where tools
//! echo the most, are off for the public by default. ADR 0030's redaction (bounds, known secret
//! shapes, headers) applies before any projection and is not repeated here.

use orch_core::{
    Actor, ActorType, AgentTarget, Event, EventBody, JobView, ThreadId, ThreadRecord, ThreadState,
    ThreadUnsharedData, Timestamp, Visibility,
};
use serde::Serialize;

/// What a reader is told in place of the owner's e-mail.
pub const THE_OWNER: &str = "the owner";

/// Who is reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderAudience {
    /// A signed-in person, by the signed-in route.
    Internal,
    /// Anybody, by the public route.
    Public,
}

/// What an audience may see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderRules {
    /// Who is reading.
    pub audience: ReaderAudience,
    /// Whether steps carry their input, output and detail.
    pub step_io: bool,
    /// Whether the thread's files are named.
    pub files: bool,
}

impl ReaderRules {
    /// A signed-in reader sees everything a reader can: steps' input and output, and the files
    /// (opening one is the `artifact.read` of the route).
    pub const fn internal() -> Self {
        ReaderRules {
            audience: ReaderAudience::Internal,
            step_io: true,
            files: true,
        }
    }

    /// A public reader sees steps' labels and states, and what the deployment adds
    /// (`sharing.public`).
    pub const fn public(step_io: bool, files: bool) -> Self {
        ReaderRules {
            audience: ReaderAudience::Public,
            step_io,
            files,
        }
    }
}

/// The actor of an event as a reader sees it: a person is "the owner", never an e-mail. Agents and
/// the orchestrator are named as they are.
fn actor_for_reader(actor: &Actor) -> Actor {
    match actor.r#type {
        ActorType::User => Actor {
            r#type: ActorType::User,
            name: THE_OWNER.to_owned(),
            revision: None,
        },
        ActorType::Agent | ActorType::System => actor.clone(),
    }
}

/// The event that stands for one a reader may not see: the same `seq`, `at` and thread, from the
/// orchestrator, saying nothing (it is a revocation, which no projection of ours renders).
pub fn inert(event: &Event) -> Event {
    Event {
        seq: event.seq,
        thread_id: event.thread_id,
        at: event.at,
        actor: Actor::system(),
        body: EventBody::ThreadUnshared(ThreadUnsharedData {}),
    }
}

/// The event as `rules` lets a reader see it, with the same `seq`. See the module's table.
pub fn reader_event(event: &Event, rules: &ReaderRules) -> Event {
    let body = match &event.body {
        // What is said: kept, by "the owner" for a person.
        EventBody::UserMessage(_)
        | EventBody::AgentMessage(_)
        | EventBody::AgentStatus(_)
        | EventBody::ThreadState(_)
        | EventBody::Error(_)
        | EventBody::UiSurface(_)
        | EventBody::UiAction(_)
        | EventBody::CiResult(_)
        | EventBody::CheckResult(_)
        | EventBody::Rework(_)
        | EventBody::JobStarted(_)
        | EventBody::ThreadTitled(_)
        | EventBody::ThreadDescribed(_)
        | EventBody::ToolsAttached(_)
        | EventBody::ToolsDetached(_)
        | EventBody::AskStarted(_)
        | EventBody::AskFinished(_) => event.body.clone(),
        // A step: its label and state always; its detail, input and output only when the
        // audience may have them.
        EventBody::AgentStep(step) => {
            let mut step = step.clone();
            if !rules.step_io {
                step.input = None;
                step.output = None;
                step.detail = None;
            }
            EventBody::AgentStep(step)
        }
        // A file: when the audience may not have files, the artifact is what it says besides the
        // file (a link, a text); one that is only a file is not there at all.
        EventBody::Artifact(artifact) => {
            if rules.files || artifact.file.is_none() {
                event.body.clone()
            } else if artifact.uri.is_some() || artifact.text.is_some() {
                let mut artifact = artifact.clone();
                artifact.file = None;
                EventBody::Artifact(artifact)
            } else {
                return inert(event);
            }
        }
        // The person's screen and the thread's branching and sharing are the owner's business.
        EventBody::UiCatalog(_)
        | EventBody::ThreadForked(_)
        | EventBody::ThreadShared(_)
        | EventBody::ThreadUnshared(_) => return inert(event),
    };
    Event {
        seq: event.seq,
        thread_id: event.thread_id,
        at: event.at,
        actor: actor_for_reader(&event.actor),
        body,
    }
}

/// The thread as a reader is given it (`GET /api/shared/{token}`, `GET /api/public/shared/{token}`):
/// the contract's `SharedThread`. No owner, no parent, no tools, no way to act: the owner's e-mail
/// is nowhere in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedThreadView {
    /// The thread's id: not a capability, and the AG-UI `threadId` of the stream.
    pub id: ThreadId,
    /// Title.
    pub title: String,
    /// Description, when the thread has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The agent the thread talks to.
    pub target: AgentTarget,
    /// State.
    pub state: ThreadState,
    /// The job's ledger, under an active gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<JobView>,
    /// The last event's sequence number.
    pub last_seq: i64,
    /// What the link is served at now: `min(visibility, cap)`, `internal` or `public`.
    pub visibility: Visibility,
    /// Whether the person reading is the thread's owner: the web sends the owner to the thread
    /// itself. Always `false` for a public read, which has no person.
    pub is_owner: bool,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last change time.
    pub updated_at: Timestamp,
}

/// The view of `thread` for a reader, served at `effective`.
pub fn reader_thread(
    thread: &ThreadRecord,
    effective: Visibility,
    is_owner: bool,
) -> SharedThreadView {
    SharedThreadView {
        id: thread.id,
        title: thread.title.clone(),
        description: thread.description.clone(),
        target: thread.target.clone(),
        state: thread.state,
        job: thread.job.view(),
        last_seq: thread.last_seq,
        visibility: effective,
        is_owner,
        created_at: thread.created_at,
        updated_at: thread.updated_at,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::{
        AgentId, AgentMessageData, AgentStepData, ArtifactData, AskStartedData, FileRef, Job,
        ShareLevel, StepKind, StepOutput, StepPhase, StepState, ThreadForkedData, ThreadSharedData,
        ThreadStateData, UiCatalogData, UserId, UserMessageData,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    const EMAIL: &str = "owner@example.com";

    fn tid() -> ThreadId {
        ThreadId(Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0001))
    }

    fn owner() -> Actor {
        Actor::user(&UserId::new(EMAIL))
    }

    fn coder() -> Actor {
        Actor::agent(&AgentId::new("coder"), Some("rev-1".into()))
    }

    fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
        Event {
            seq,
            thread_id: tid(),
            at: Timestamp::from_second(1_790_000_000 + seq).unwrap(),
            actor,
            body,
        }
    }

    fn step() -> AgentStepData {
        let mut input = serde_json::Map::new();
        input.insert("command".into(), json!("cat /etc/hosts"));
        AgentStepData {
            id: "T/1".into(),
            path: vec![],
            kind: StepKind::Tool,
            label: "Run the tests".into(),
            state: StepState::Completed,
            phase: StepPhase::End,
            icon: Some("terminal".into()),
            detail: Some("1 failed".into()),
            input: Some(input),
            output: Some(StepOutput {
                text: "a secret-looking line".into(),
                truncated: false,
                bytes: None,
                error: false,
            }),
            io_dropped: false,
        }
    }

    fn file_only() -> ArtifactData {
        ArtifactData {
            name: "chart.png".into(),
            mime_type: Some("image/png".into()),
            uri: None,
            text: None,
            file: Some(FileRef {
                sha256: "a".repeat(64),
                size: 10,
                filename: Some("chart.png".into()),
            }),
        }
    }

    /// One of every kind of event the log has, by the owner or an agent.
    fn log() -> Vec<Event> {
        vec![
            ev(
                1,
                owner(),
                EventBody::UserMessage(UserMessageData::new("fix it")),
            ),
            ev(
                2,
                coder(),
                EventBody::AgentMessage(AgentMessageData {
                    text: "on it".into(),
                    message_id: "m1".into(),
                    is_final: true,
                    purpose: None,
                    via: None,
                }),
            ),
            ev(3, coder(), EventBody::AgentStep(step())),
            ev(4, coder(), EventBody::Artifact(file_only())),
            ev(
                5,
                Actor::system(),
                EventBody::ThreadState(ThreadStateData {
                    state: ThreadState::Done,
                }),
            ),
            ev(
                6,
                owner(),
                EventBody::UiCatalog(UiCatalogData {
                    catalog_id: "https://example.com/catalog".into(),
                    version: 1,
                    digest: format!("sha256:{}", "0".repeat(64)),
                    catalog: json!({"catalogId": "https://example.com/catalog", "components": {}}),
                }),
            ),
            ev(
                7,
                owner(),
                EventBody::ThreadForked(ThreadForkedData {
                    from: orch_core::ForkSource {
                        thread_id: tid(),
                        seq: 3,
                    },
                    kind: orch_core::ForkKind::Fork,
                    title: "t".into(),
                    description: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("coder"),
                        release: None,
                    },
                }),
            ),
            ev(
                8,
                owner(),
                EventBody::ThreadShared(ThreadSharedData {
                    visibility: ShareLevel::Public,
                    nonce_sha256: "00".into(),
                }),
            ),
            ev(9, owner(), EventBody::ThreadUnshared(ThreadUnsharedData {})),
            ev(
                10,
                owner(),
                EventBody::AskStarted(AskStartedData {
                    ask: 1,
                    agent: AgentId::new("researcher"),
                    by: orch_core::Caller::Main,
                    depth: 1,
                    text: "find it".into(),
                    step_id: "ask-1".into(),
                    parent_step_id: None,
                }),
            ),
        ]
    }

    fn is_inert(e: &Event) -> bool {
        e.actor == Actor::system() && matches!(e.body, EventBody::ThreadUnshared(_))
    }

    #[test]
    fn the_owners_address_is_nowhere_in_what_a_reader_gets() {
        for rules in [
            ReaderRules::internal(),
            ReaderRules::public(false, false),
            ReaderRules::public(true, true),
        ] {
            for event in log() {
                let out = reader_event(&event, &rules);
                assert_eq!(out.seq, event.seq, "numbering is kept");
                assert_eq!(out.thread_id, event.thread_id);
                let json = serde_json::to_string(&out).unwrap();
                assert!(
                    !json.contains(EMAIL),
                    "{:?} {rules:?}: {json}",
                    event.kind()
                );
                if event.actor.r#type == ActorType::User && !is_inert(&out) {
                    assert_eq!(out.actor.name, THE_OWNER);
                    assert_eq!(out.actor.r#type, ActorType::User);
                }
            }
        }
    }

    #[test]
    fn what_is_said_is_kept_and_what_is_the_owners_business_is_inert() {
        let rules = ReaderRules::internal();
        for event in log() {
            let out = reader_event(&event, &rules);
            let expect_inert = matches!(
                event.body,
                EventBody::UiCatalog(_)
                    | EventBody::ThreadForked(_)
                    | EventBody::ThreadShared(_)
                    | EventBody::ThreadUnshared(_)
            );
            assert_eq!(is_inert(&out), expect_inert, "{:?}", event.kind());
            if !expect_inert {
                assert_eq!(out.body, event.body, "{:?}", event.kind());
            }
        }
        // an agent keeps its name and revision, the orchestrator its own
        let agent = reader_event(&log()[1], &rules);
        assert_eq!(agent.actor, coder());
        assert_eq!(reader_event(&log()[4], &rules).actor, Actor::system());
    }

    #[test]
    fn a_public_reader_sees_step_labels_and_not_their_input_output_or_detail() {
        let event = ev(3, coder(), EventBody::AgentStep(step()));
        for (rules, shown) in [
            (ReaderRules::internal(), true),
            (ReaderRules::public(true, false), true),
            (ReaderRules::public(false, false), false),
            (ReaderRules::public(false, true), false),
        ] {
            let EventBody::AgentStep(out) = reader_event(&event, &rules).body else {
                panic!("a step stays a step");
            };
            // labels and states are always there
            assert_eq!(out.label, "Run the tests");
            assert_eq!(out.state, StepState::Completed);
            assert_eq!(out.id, "T/1");
            assert_eq!(
                (
                    out.input.is_some(),
                    out.output.is_some(),
                    out.detail.is_some()
                ),
                (shown, shown, shown),
                "{rules:?}"
            );
        }
    }

    #[test]
    fn a_public_reader_has_the_files_only_when_the_deployment_says_so() {
        let only_a_file = ev(4, coder(), EventBody::Artifact(file_only()));
        let with_a_link = ev(
            5,
            coder(),
            EventBody::Artifact(ArtifactData {
                uri: Some("https://github.com/acme/demo/pull/1".into()),
                ..file_only()
            }),
        );
        let plain = ev(
            6,
            coder(),
            EventBody::Artifact(ArtifactData {
                name: "pr".into(),
                mime_type: None,
                uri: Some("https://github.com/acme/demo/pull/1".into()),
                text: None,
                file: None,
            }),
        );
        for (rules, files) in [
            (ReaderRules::internal(), true),
            (ReaderRules::public(false, true), true),
            (ReaderRules::public(true, false), false),
        ] {
            let kept = |e: &Event| match reader_event(e, &rules).body {
                EventBody::Artifact(a) => Some(a.file.is_some()),
                _ => None,
            };
            if files {
                assert_eq!(kept(&only_a_file), Some(true), "{rules:?}");
                assert_eq!(kept(&with_a_link), Some(true), "{rules:?}");
            } else {
                // a file that is only a file is not there; a link keeps its link and loses the file
                assert!(is_inert(&reader_event(&only_a_file, &rules)), "{rules:?}");
                assert_eq!(kept(&with_a_link), Some(false), "{rules:?}");
            }
            // an artifact that is no file is a link, whoever reads
            assert_eq!(kept(&plain), Some(false));
        }
    }

    #[test]
    fn the_view_has_no_owner_no_parent_and_no_tools() {
        let record = ThreadRecord {
            id: tid(),
            owner: UserId::new(EMAIL),
            title: "Fix the build".into(),
            description: Some("Moving to Rust.".into()),
            target: AgentTarget {
                agent_id: AgentId::new("coder"),
                release: Some("stable".into()),
            },
            state: ThreadState::Done,
            job: Job {
                tools: vec!["websearch".into()],
                ..Job::default()
            },
            version: 4,
            forked_from: Some(orch_core::ForkedFrom {
                thread_id: Some(ThreadId(Uuid::from_u128(9))),
                seq: 3,
                kind: orch_core::ForkKind::Fork,
            }),
            share: None,
            last_seq: 12,
            created_at: Timestamp::from_second(1_790_000_000).unwrap(),
            updated_at: Timestamp::from_second(1_790_000_100).unwrap(),
        };
        let view = serde_json::to_value(reader_thread(&record, Visibility::Public, false)).unwrap();
        assert_eq!(
            view,
            json!({
                "id": tid().to_string(),
                "title": "Fix the build",
                "description": "Moving to Rust.",
                "target": {"agentId": "coder", "release": "stable"},
                "state": "done",
                "lastSeq": 12,
                "visibility": "public",
                "isOwner": false,
                "createdAt": "2026-09-21T14:13:20Z",
                "updatedAt": "2026-09-21T14:15:00Z",
            })
        );
        let text = view.to_string();
        assert!(!text.contains(EMAIL) && !text.contains("websearch") && !text.contains("forked"));
    }
}
