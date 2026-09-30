//! Legal logs, made by the core's own `transition`: whatever sequence of inputs, the events it
//! appends are a log the orchestrator could have written.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{
    AgentId, AgentTarget, AgentTaskState, AgentUpdate, CheckSource, Command, Event, GatePolicy,
    Input, ThreadId, ThreadState, Timestamp, UiActionData, UiVersion, UserId,
};
use proptest::prelude::*;
use serde_json::json;

pub const THREAD: &str = "00000000-0000-7000-8000-000000000001";

pub fn thread_id() -> ThreadId {
    THREAD.parse().unwrap()
}

pub fn meta() -> orch_agui_projection::ThreadMeta {
    meta_under(GatePolicy::default())
}

/// The thread of [`meta`], its job running under `gate`.
pub fn meta_under(gate: GatePolicy) -> orch_agui_projection::ThreadMeta {
    orch_agui_projection::ThreadMeta {
        thread_id: thread_id(),
        title: "a thread".to_owned(),
        target: AgentTarget {
            agent_id: AgentId::new("plain"),
            release: None,
        },
        gate,
    }
}

/// A gate that requires the agent's own checks, three attempts.
pub fn gate() -> GatePolicy {
    GatePolicy::requiring([CheckSource::AgentChecks])
}

/// The events a legal log of `actions` makes, and the thread they belong to: with the gate when
/// `gated`, without it otherwise.
pub fn world(gated: bool, actions: &[Action]) -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let gate = if gated { gate() } else { GatePolicy::default() };
    (build_under(actions, &gate), meta_under(gate))
}

/// One thing that happens to a thread.
#[derive(Debug, Clone)]
pub enum Action {
    /// A user message (with the surface's ids when `ids`).
    User {
        text: String,
        ids: bool,
    },
    Cancel,
    Status(AgentTaskState, Option<String>),
    Artifact,
    /// The agent says `more` in message slot `slot`, and finishes it when `fin`.
    Say {
        slot: u8,
        more: String,
        fin: bool,
    },
    /// The agent rewrites the message in `slot` with text that does not extend it.
    Rewrite {
        slot: u8,
    },
    Delivery {
        retryable: bool,
    },
    CancelledBeforeStart,
    CancelRejected {
        retryable: bool,
    },
    /// The agent sends one A2UI operation for surface `s<surface>`: `op` 0 creates, 1 updates
    /// components, 2 updates data, 3 deletes.
    Surface {
        surface: u8,
        op: u8,
    },
    /// The agent's A2UI payload failed the envelope check.
    SurfaceRefused,
    /// The user acts on surface `s0`.
    UiAct {
        ids: bool,
    },
    /// The agent reports the branch it pushed (a commit named by `commit`).
    Branch {
        commit: u8,
    },
    /// The agent reports the result of its own checks on the commit named by `commit`.
    Checks {
        passed: bool,
        commit: u8,
    },
}

/// One operation of surface `s<surface>`, in the shapes of the A2UI spec.
pub fn surface_op(surface: u8, op: u8) -> serde_json::Value {
    let id = format!("s{surface}");
    match op % 4 {
        0 => json!({"version": "v0.9.1", "createSurface": {"surfaceId": id, "catalogId": "c"}}),
        1 => json!({"version": "v0.9.1", "updateComponents": {"surfaceId": id, "components": [
            {"id": "root", "component": "Text", "text": "hi"}]}}),
        2 => json!({"version": "v0.9.1", "updateDataModel": {"surfaceId": id, "value": {"n": op}}}),
        _ => json!({"version": "v0.9.1", "deleteSurface": {"surfaceId": id}}),
    }
}

pub fn arb_action() -> impl Strategy<Value = Action> {
    let task_state = prop_oneof![
        4 => Just(AgentTaskState::Working),
        3 => Just(AgentTaskState::InputRequired),
        1 => Just(AgentTaskState::AuthRequired),
        2 => Just(AgentTaskState::Completed),
        1 => Just(AgentTaskState::Failed),
        1 => Just(AgentTaskState::Canceled),
        1 => Just(AgentTaskState::Rejected),
        1 => Just(AgentTaskState::Submitted),
    ];
    let detail = proptest::option::of("[a-z ]{1,8}");
    prop_oneof![
        5 => ("[a-z]{1,8}", any::<bool>()).prop_map(|(text, ids)| Action::User { text, ids }),
        1 => Just(Action::Cancel),
        8 => (task_state, detail).prop_map(|(s, d)| Action::Status(s, d)),
        3 => Just(Action::Artifact),
        5 => (0u8..2, "[a-z]{1,4}", any::<bool>())
            .prop_map(|(slot, more, fin)| Action::Say { slot, more, fin }),
        1 => (0u8..2).prop_map(|slot| Action::Rewrite { slot }),
        2 => any::<bool>().prop_map(|retryable| Action::Delivery { retryable }),
        1 => Just(Action::CancelledBeforeStart),
        2 => any::<bool>().prop_map(|retryable| Action::CancelRejected { retryable }),
        4 => (0u8..2, 0u8..4).prop_map(|(surface, op)| Action::Surface { surface, op }),
        1 => Just(Action::SurfaceRefused),
        2 => any::<bool>().prop_map(|ids| Action::UiAct { ids }),
        3 => (0u8..3).prop_map(|commit| Action::Branch { commit }),
        3 => (any::<bool>(), 0u8..3).prop_map(|(passed, commit)| Action::Checks { passed, commit }),
    ]
}

/// A log that mostly starts the way a real one does, with the user speaking.
pub fn arb_actions() -> impl Strategy<Value = Vec<Action>> {
    (
        proptest::bool::weighted(0.9),
        proptest::collection::vec(arb_action(), 0..40),
    )
        .prop_map(|(lead, mut actions)| {
            if lead {
                actions.insert(
                    0,
                    Action::User {
                        text: "go".to_owned(),
                        ids: false,
                    },
                );
            }
            actions
        })
}

#[derive(Default, Clone)]
struct Slot {
    generation: u32,
    text: String,
}

/// Runs `actions` through `transition` and collects the events it appends, numbered from 1.
/// Actions the state refuses (a message on a finished thread) are skipped, as the service does.
pub fn build(actions: &[Action]) -> Vec<Event> {
    build_under(actions, &GatePolicy::default())
}

/// [`build`] for a thread whose job runs under `gate`.
pub fn build_under(actions: &[Action], gate: &GatePolicy) -> Vec<Event> {
    let user = UserId::new("alice@example.com");
    let agent = AgentId::new("plain");
    let mut state = orch_core::Snapshot {
        state: ThreadState::Queued,
        job: orch_core::Job::with_gate(gate.clone()),
    };
    let mut events: Vec<Event> = Vec::new();
    let mut slots = [Slot::default(), Slot::default()];
    let mut users = 0u32;
    for (n, action) in actions.iter().enumerate() {
        let revision = (n % 3 == 0).then(|| "rev-1".to_owned());
        let agent_input = |update: AgentUpdate| Input::Agent {
            agent: agent.clone(),
            revision: revision.clone(),
            update,
        };
        let mut next_slots = slots.clone();
        let input = match action {
            Action::User { text, ids } => {
                users += 1;
                Input::UserMessage {
                    user: user.clone(),
                    text: text.clone(),
                    message_id: ids.then(|| format!("m-{users}")),
                    run_id: ids.then(|| format!("r-{users}")),
                }
            }
            Action::Cancel => Input::Cancel { user: user.clone() },
            Action::Status(task, detail) => agent_input(AgentUpdate::Status {
                state: *task,
                detail: detail.clone(),
            }),
            Action::Artifact => agent_input(AgentUpdate::Artifact {
                name: "result".to_owned(),
                mime_type: None,
                uri: Some("https://example.com/pr/1".to_owned()),
                text: None,
            }),
            Action::Say { slot, more, fin } => {
                let s = &mut next_slots[usize::from(*slot)];
                s.text.push_str(more);
                let update = AgentUpdate::Message {
                    message_id: format!("a{slot}-{}", s.generation),
                    text: s.text.clone(),
                    is_final: *fin,
                };
                if *fin {
                    s.generation += 1;
                    s.text.clear();
                }
                agent_input(update)
            }
            Action::Rewrite { slot } => {
                let s = &mut next_slots[usize::from(*slot)];
                s.text = "REWRITTEN".to_owned();
                agent_input(AgentUpdate::Message {
                    message_id: format!("a{slot}-{}", s.generation),
                    text: s.text.clone(),
                    is_final: false,
                })
            }
            Action::Delivery { retryable } => Input::DeliveryFailed {
                reason: "delivery failed".to_owned(),
                retryable: *retryable,
            },
            Action::CancelledBeforeStart => Input::CancelledBeforeStart,
            Action::CancelRejected { retryable } => Input::CancelRejected {
                reason: "cancel refused".to_owned(),
                retryable: *retryable,
            },
            Action::Surface { surface, op } => agent_input(AgentUpdate::Ui {
                operations: vec![surface_op(*surface, *op)],
            }),
            Action::SurfaceRefused => agent_input(AgentUpdate::UiRejected {
                reason: "message 0: no version".to_owned(),
            }),
            Action::Branch { commit } => agent_input(AgentUpdate::Artifact {
                name: "branch".to_owned(),
                mime_type: None,
                uri: None,
                text: Some(
                    json!({"repository": "https://github.com/acme/demo.git", "branch": "agent/x",
                           "commit": format!("{commit:040x}")})
                    .to_string(),
                ),
            }),
            Action::Checks { passed, commit } => agent_input(AgentUpdate::Artifact {
                name: "checks".to_owned(),
                mime_type: None,
                uri: None,
                text: Some(
                    json!({"passed": passed, "commit": format!("{commit:040x}"),
                           "findings": if *passed { json!([]) } else { json!(["it fails"]) }})
                    .to_string(),
                ),
            }),
            Action::UiAct { ids } => {
                users += 1;
                Input::UiAction {
                    user: user.clone(),
                    action: UiActionData {
                        surface_id: "s0".to_owned(),
                        name: "go".to_owned(),
                        source_component_id: "btn".to_owned(),
                        context: serde_json::Map::new(),
                        version: UiVersion::V0_9_1,
                        run_id: ids.then(|| format!("r-{users}")),
                    },
                }
            }
        };
        let Ok((next, commands)) = orch_core::transition(&state, &input) else {
            continue;
        };
        state = next;
        slots = next_slots;
        for command in commands {
            if let Command::Append(draft) = command {
                let seq = i64::try_from(events.len()).unwrap() + 1;
                events.push(Event {
                    seq,
                    thread_id: thread_id(),
                    at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
                    actor: draft.actor,
                    body: draft.body,
                });
            }
        }
    }
    events
}
