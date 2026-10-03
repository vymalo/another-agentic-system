//! The MCP servers attached to a thread (ADR 0024): the transition rows of `Input::SetTools`, the
//! set carried from job to job, the events' wire shape, and what a fork starts with.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::{
    Actor, AttachedServer, Command, Event, EventBody, EventDraft, EventKind, GatePolicy, Input,
    Job, MAX_ATTACHED_SERVERS, Snapshot, ThreadId, ThreadState, ToolsData, ToolsError, ToolsGrant,
    UserId, attached_by, check_servers, transition,
};

const EVERY_STATE: [ThreadState; 7] =
    [Queued, Working, Verifying, Blocked, Done, Failed, Cancelled];

fn user() -> UserId {
    UserId::new("me@example.com")
}

fn ids(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| (*i).to_owned()).collect()
}

fn set(servers: &[&str]) -> Input {
    Input::SetTools {
        user: user(),
        servers: ids(servers),
    }
}

fn with_tools(state: ThreadState, tools: &[&str]) -> Snapshot {
    Snapshot {
        state,
        job: Job {
            tools: ids(tools),
            ..Job::default()
        },
    }
}

fn attached(servers: &[&str]) -> Command {
    Command::Append(EventDraft {
        actor: Actor::user(&user()),
        body: EventBody::ToolsAttached(ToolsData {
            servers: ids(servers),
        }),
    })
}

fn detached(servers: &[&str]) -> Command {
    Command::Append(EventDraft {
        actor: Actor::user(&user()),
        body: EventBody::ToolsDetached(ToolsData {
            servers: ids(servers),
        }),
    })
}

#[test]
fn a_set_is_accepted_in_every_state_and_changes_nothing_but_the_set() {
    for state in EVERY_STATE {
        let (next, cmds) = transition(&with_tools(state, &[]), &set(&["websearch"])).unwrap();
        assert_eq!(next.state, state, "{state:?}: the state is the thread's");
        assert_eq!(next.job.tools, ids(&["websearch"]), "{state:?}");
        assert_eq!(cmds, vec![attached(&["websearch"])], "{state:?}");
        // nothing else of the job moves: it is the same job but for the set
        assert_eq!(
            next.job,
            Job {
                tools: ids(&["websearch"]),
                ..Job::default()
            },
            "{state:?}"
        );
    }
}

#[test]
fn the_same_set_is_no_event_and_no_change() {
    for state in EVERY_STATE {
        let before = with_tools(state, &["docs", "websearch"]);
        let (next, cmds) = transition(&before, &set(&["docs", "websearch"])).unwrap();
        assert_eq!((next, cmds), (before.clone(), vec![]), "{state:?}");
        // the order and the repeats of what the person sent do not make a change
        let (next, cmds) = transition(&before, &set(&["websearch", "docs", "websearch"])).unwrap();
        assert_eq!((next, cmds), (before, vec![]), "{state:?}");
    }
}

#[test]
fn a_new_set_is_the_difference_as_one_event_each_way() {
    let before = with_tools(Working, &["docs", "files"]);
    let (next, cmds) = transition(&before, &set(&["files", "websearch"])).unwrap();
    assert_eq!(next.job.tools, ids(&["files", "websearch"]));
    assert_eq!(cmds, vec![attached(&["websearch"]), detached(&["docs"])]);

    let (next, cmds) = transition(&before, &set(&[])).unwrap();
    assert!(next.job.tools.is_empty());
    assert_eq!(cmds, vec![detached(&["docs", "files"])]);

    let (next, cmds) = transition(&before, &set(&["docs", "files", "a", "z"])).unwrap();
    assert_eq!(next.job.tools, ids(&["a", "docs", "files", "z"]));
    assert_eq!(cmds, vec![attached(&["a", "z"])]);
}

#[test]
fn a_set_does_not_delegate_or_touch_the_titles_or_the_steps() {
    let (_, cmds) = transition(&with_tools(Working, &[]), &set(&["websearch"])).unwrap();
    assert!(
        cmds.iter().all(|c| matches!(c, Command::Append(_))),
        "attaching writes events and nothing else: {cmds:?}"
    );
}

#[test]
fn the_set_is_carried_to_the_next_job() {
    let before = with_tools(Done, &["docs", "websearch"]);
    let (next, cmds) = transition(
        &before,
        &Input::UserMessage {
            user: user(),
            text: "again".into(),
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Agui,
            catalog: None,
            mentions: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(next.job.number, 2);
    assert_eq!(next.job.tools, ids(&["docs", "websearch"]));
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Append(EventDraft {
                body: EventBody::ToolsAttached(_) | EventBody::ToolsDetached(_),
                ..
            })
        )),
        "a new job re-announces nothing: the thread's set did not change"
    );
    assert_eq!(before.job.next().tools, ids(&["docs", "websearch"]));
}

#[test]
fn the_set_is_carried_through_the_gate_and_the_other_ledgers() {
    // under an active gate, the set is still the thread's
    let gate = GatePolicy::requiring([orch_core::CheckSource::Ci]);
    let before = Snapshot {
        state: Working,
        job: Job {
            tools: ids(&["docs"]),
            ..Job::with_gate(gate)
        },
    };
    let (next, _) = transition(&before, &set(&["docs", "files"])).unwrap();
    assert_eq!(next.job.tools, ids(&["docs", "files"]));
    assert!(next.job.gate.is_active());
}

#[test]
fn a_job_without_servers_leaves_them_out_of_its_ledger_and_reads_an_old_one() {
    let json = serde_json::to_value(Job::default()).unwrap();
    assert!(json.get("tools").is_none(), "{json}");
    let with = serde_json::to_value(with_tools(Queued, &["docs"]).job).unwrap();
    assert_eq!(with["tools"], serde_json::json!(["docs"]));
    // a ledger stored before the field existed
    let old: Job = serde_json::from_value(serde_json::json!({"number": 2})).unwrap();
    assert!(old.tools.is_empty());
    assert_eq!(old.number, 2);
}

#[test]
fn the_events_are_ids_and_nothing_else_on_the_wire() {
    for (body, kind, name) in [
        (
            EventBody::ToolsAttached(ToolsData {
                servers: ids(&["docs", "websearch"]),
            }),
            EventKind::ToolsAttached,
            "tools_attached",
        ),
        (
            EventBody::ToolsDetached(ToolsData {
                servers: ids(&["docs"]),
            }),
            EventKind::ToolsDetached,
            "tools_detached",
        ),
    ] {
        assert_eq!(body.kind(), kind);
        assert_eq!(kind.as_str(), name);
        let event = Event {
            seq: 3,
            thread_id: ThreadId(uuid::Uuid::nil()),
            at: orch_core::Timestamp::UNIX_EPOCH,
            actor: Actor::user(&user()),
            body: body.clone(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], name);
        assert!(json["data"]["servers"].is_array(), "{json}");
        assert_eq!(json["data"].as_object().unwrap().len(), 1, "{json}");
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }
}

#[test]
fn a_set_is_checked_by_shape_and_size() {
    assert_eq!(check_servers(&ids(&["b", "a", "b"])), Ok(ids(&["a", "b"])));
    assert_eq!(
        check_servers(&ids(&["web search"])),
        Err(ToolsError::BadId("web search".into()))
    );
    let many: Vec<String> = (0..=MAX_ATTACHED_SERVERS)
        .map(|n| format!("s{n}"))
        .collect();
    assert_eq!(check_servers(&many), Err(ToolsError::TooMany));
}

fn logged(seq: i64, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: ThreadId(uuid::Uuid::nil()),
        at: orch_core::Timestamp::UNIX_EPOCH,
        actor: Actor::user(&user()),
        body,
    }
}

#[test]
fn what_the_log_leaves_attached_is_what_a_copy_of_it_starts_with() {
    let log = vec![
        logged(
            1,
            EventBody::ToolsAttached(ToolsData {
                servers: ids(&["docs", "websearch"]),
            }),
        ),
        logged(
            2,
            EventBody::ToolsDetached(ToolsData {
                servers: ids(&["docs"]),
            }),
        ),
        logged(
            3,
            EventBody::ToolsAttached(ToolsData {
                servers: ids(&["files"]),
            }),
        ),
    ];
    assert_eq!(attached_by(&log), ids(&["files", "websearch"]));
    assert_eq!(attached_by(&log[..2]), ids(&["websearch"]));
    assert_eq!(attached_by(&log[..1]), ids(&["docs", "websearch"]));
    assert!(attached_by(&[]).is_empty());

    let snapshot = orch_core::forked_snapshot(
        &log,
        GatePolicy::default(),
        orch_core::TitleLedger::default(),
        orch_core::DescriptionLedger::default(),
    );
    assert_eq!(snapshot.job.tools, ids(&["files", "websearch"]));
    assert_eq!(snapshot.state, Done);
}

#[test]
fn a_grant_is_told_the_attached_servers_and_nothing_secret() {
    let grant = ToolsGrant::main(
        ThreadId(uuid::Uuid::nil()),
        1,
        orch_core::AgentId::new("chat"),
    )
    .with_attached(vec![AttachedServer {
        id: "websearch".into(),
        name: "Web search".into(),
        description: Some("Search the web.".into()),
    }]);
    assert!(grant.is_consistent());
    assert_eq!(grant.attached[0].id, "websearch");
    let json = serde_json::to_value(&grant.attached).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{"id": "websearch", "name": "Web search", "description": "Search the web."}])
    );
}

#[test]
fn a_thread_serialises_its_servers_whatever_its_gate() {
    use orch_core::{AgentTarget, ThreadRecord};
    let record = |tools: &[&str], gate: GatePolicy| ThreadRecord {
        id: ThreadId(uuid::Uuid::nil()),
        owner: user(),
        title: "t".into(),
        description: None,
        target: AgentTarget {
            agent_id: orch_core::AgentId::new("chat"),
            release: None,
        },
        state: Queued,
        job: Job {
            tools: ids(tools),
            ..Job::with_gate(gate)
        },
        version: 1,
        forked_from: None,
        share: None,
        last_seq: 2,
        created_at: orch_core::Timestamp::UNIX_EPOCH,
        updated_at: orch_core::Timestamp::UNIX_EPOCH,
    };
    let plain = serde_json::to_value(record(&["docs"], GatePolicy::default())).unwrap();
    assert_eq!(plain["tools"], serde_json::json!(["docs"]));
    assert!(plain.get("job").is_none(), "no gate, no job view: {plain}");
    let gated = serde_json::to_value(record(
        &["docs", "files"],
        GatePolicy::requiring([orch_core::CheckSource::Ci]),
    ))
    .unwrap();
    assert_eq!(gated["tools"], serde_json::json!(["docs", "files"]));
    assert!(gated.get("job").is_some());
    let none = serde_json::to_value(record(&[], GatePolicy::default())).unwrap();
    assert!(none.get("tools").is_none(), "omitted when empty: {none}");
    assert_eq!(none["id"], "00000000-0000-0000-0000-000000000000");
    assert_eq!(none["lastSeq"], 2);
}
