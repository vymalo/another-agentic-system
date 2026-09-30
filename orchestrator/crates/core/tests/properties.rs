//! Property tests over random input sequences.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

/// The gate is off in these tests (the default job), so the state alone decides: the shim keeps
/// every case below as it was before the job existed.
fn transition(
    state: &ThreadState,
    input: &Input,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
    orch_core::transition(&Snapshot::new(*state), input).map(|(next, cmds)| (next.state, cmds))
}
use proptest::prelude::*;

fn arb_task_state() -> impl Strategy<Value = AgentTaskState> {
    prop_oneof![
        Just(AgentTaskState::Submitted),
        Just(AgentTaskState::Working),
        Just(AgentTaskState::InputRequired),
        Just(AgentTaskState::AuthRequired),
        Just(AgentTaskState::Completed),
        Just(AgentTaskState::Failed),
        Just(AgentTaskState::Canceled),
        Just(AgentTaskState::Rejected),
    ]
}

fn arb_input() -> impl Strategy<Value = Input> {
    let detail = proptest::option::of("[a-z ]{0,8}");
    prop_oneof![
        "[a-z]{1,8}".prop_map(|text| Input::UserMessage {
            user: UserId::new("u@x.io"),
            text,
            message_id: None,
            run_id: None,
        }),
        Just(Input::Cancel {
            user: UserId::new("u@x.io")
        }),
        (arb_task_state(), detail.clone()).prop_map(|(state, detail)| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Status { state, detail }
        }),
        Just(Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Artifact {
                name: "n".into(),
                mime_type: None,
                uri: None,
                text: None
            }
        }),
        Just(Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Message {
                message_id: "m".into(),
                text: "t".into(),
                is_final: true
            }
        }),
        (any::<bool>(), "[a-z]{1,5}")
            .prop_map(|(retryable, reason)| Input::DeliveryFailed { reason, retryable }),
        Just(Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Ui {
                operations: vec![
                    serde_json::json!({"version": "v0.9.1", "deleteSurface": {"surfaceId": "s"}})
                ]
            }
        }),
        "[a-z]{1,8}".prop_map(|reason| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::UiRejected { reason }
        }),
        "[a-z]{1,8}".prop_map(|name| Input::UiAction {
            user: UserId::new("u@x.io"),
            action: UiActionData {
                surface_id: "s".into(),
                name,
                source_component_id: "b".into(),
                context: serde_json::Map::new(),
                version: UiVersion::V0_9_1,
                run_id: None,
            },
        }),
        Just(Input::CancelledBeforeStart),
        (any::<bool>(), "[a-z]{1,5}")
            .prop_map(|(retryable, reason)| Input::CancelRejected { reason, retryable }),
    ]
}

fn complete() -> Input {
    Input::Agent {
        agent: AgentId::new("a"),
        revision: None,
        update: AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: None,
        },
    }
}

proptest! {
    #[test]
    fn invariants_hold_over_random_sequences(inputs in proptest::collection::vec(arb_input(), 0..40)) {
        let mut state = ThreadState::Queued;
        for input in inputs {
            match transition(&state, &input) {
                Ok((next, cmds)) => {
                    // (a) terminal states are absorbing.
                    if state.is_terminal() {
                        prop_assert_eq!(next, state);
                    }
                    // (b) thread_state events name the resulting state and only mark entry.
                    for cmd in &cmds {
                        if let Command::Append(d) = cmd
                            && let EventBody::ThreadState(t) = &d.body
                        {
                            prop_assert_eq!(t.state, next);
                            prop_assert!(next != state, "thread_state without a state change");
                            prop_assert!(matches!(
                                next,
                                ThreadState::Blocked | ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled
                            ));
                        }
                    }
                    // Conversely: entering blocked/done/failed/cancelled always says so.
                    if next != state && !matches!(next, ThreadState::Queued | ThreadState::Working) {
                        let announced = cmds.iter().any(|c| matches!(c,
                            Command::Append(d) if matches!(&d.body, EventBody::ThreadState(t) if t.state == next)));
                        prop_assert!(announced);
                    }
                    state = next;
                }
                Err(TransitionError::Finished { state: s }) => {
                    // (c) only a user message (or an action, which answers like one) on a
                    // finished thread.
                    prop_assert!(s.is_terminal() && s == state);
                    let is_user_message =
                        matches!(input, Input::UserMessage { .. } | Input::UiAction { .. });
                    prop_assert!(is_user_message);
                }
                Err(TransitionError::InvalidInState { state: s, .. }) => {
                    prop_assert!(s.is_terminal() && s == state);
                    let is_agent = matches!(input, Input::Agent { .. });
                    prop_assert!(is_agent);
                }
                Err(other) => prop_assert!(false, "unexpected transition error: {other}"),
            }
        }
        // (d) from every non-terminal state, `Completed` reaches Done.
        if !state.is_terminal() {
            let (next, _) = transition(&state, &complete()).unwrap();
            prop_assert_eq!(next, ThreadState::Done);
        }
    }
}
