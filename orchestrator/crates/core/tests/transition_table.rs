//! One test per row of the transition table in the plan (§2.3).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Working};

const ALL: [ThreadState; 6] = [Queued, Working, Blocked, Done, Failed, Cancelled];
const OPEN: [ThreadState; 3] = [Queued, Working, Blocked];
const TERMINAL: [ThreadState; 3] = [Done, Failed, Cancelled];

fn user() -> UserId {
    UserId::new("Me@Example.com")
}
fn agent() -> AgentId {
    AgentId::new("coder")
}
fn um(text: &str) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
    }
}
fn status(state: AgentTaskState, detail: Option<&str>) -> Input {
    Input::Agent {
        agent: agent(),
        revision: Some("rev-1".into()),
        update: AgentUpdate::Status {
            state,
            detail: detail.map(Into::into),
        },
    }
}
fn bodies(cmds: &[Command]) -> Vec<&EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(&d.body),
            Command::Delegate { .. } | Command::RequestCancel => None,
        })
        .collect()
}
fn run(state: ThreadState, input: &Input) -> (ThreadState, Vec<Command>) {
    transition(&state, input).unwrap()
}
fn status_body(s: AgentStatus, detail: Option<&str>) -> EventBody {
    EventBody::AgentStatus(AgentStatusData {
        status: s,
        detail: detail.map(Into::into),
    })
}
fn state_body(s: ThreadState) -> EventBody {
    EventBody::ThreadState(ThreadStateData { state: s })
}

#[test]
fn row1_user_message_in_queued_or_working() {
    for s in [Queued, Working] {
        let (next, cmds) = run(s, &um("hi"));
        assert_eq!(next, s);
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            Command::Append(d) => {
                assert_eq!(d.actor, Actor::user(&user()));
                assert_eq!(d.actor.name, "me@example.com");
                assert_eq!(
                    d.body,
                    EventBody::UserMessage(UserMessageData { text: "hi".into() })
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(cmds[1], Command::Delegate { text: "hi".into() });
    }
}

#[test]
fn row2_user_message_in_blocked_requeues() {
    let (next, cmds) = run(Blocked, &um("main"));
    assert_eq!(next, Queued);
    assert_eq!(cmds.len(), 2);
    assert_eq!(
        cmds[1],
        Command::Delegate {
            text: "main".into()
        }
    );
}

#[test]
fn row3_user_message_in_terminal_is_finished() {
    for s in TERMINAL {
        assert_eq!(
            transition(&s, &um("x")),
            Err(TransitionError::Finished { state: s })
        );
    }
}

#[test]
fn row4_cancel_requests_cancel_when_open() {
    for s in OPEN {
        assert_eq!(
            run(s, &Input::Cancel { user: user() }),
            (s, vec![Command::RequestCancel])
        );
    }
}

#[test]
fn row5_cancel_when_terminal_is_a_noop() {
    for s in TERMINAL {
        assert_eq!(run(s, &Input::Cancel { user: user() }), (s, vec![]));
    }
}

#[test]
fn row6_working_from_queued_or_blocked() {
    for s in [Queued, Blocked] {
        let (next, cmds) = run(s, &status(AgentTaskState::Working, None));
        assert_eq!(next, Working);
        assert_eq!(bodies(&cmds), [&status_body(AgentStatus::Working, None)]);
        match &cmds[0] {
            Command::Append(d) => {
                assert_eq!(d.actor, Actor::agent(&agent(), Some("rev-1".into())));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn row6b_submitted_is_never_shown() {
    for s in OPEN {
        assert_eq!(
            run(s, &status(AgentTaskState::Submitted, None)),
            (s, vec![])
        );
    }
}

#[test]
fn row7_working_in_working_is_deduplicated_unless_detailed() {
    assert_eq!(
        run(Working, &status(AgentTaskState::Working, None)),
        (Working, vec![])
    );
    let (next, cmds) = run(Working, &status(AgentTaskState::Working, Some("compiling")));
    assert_eq!(next, Working);
    assert_eq!(
        bodies(&cmds),
        [&status_body(AgentStatus::Working, Some("compiling"))]
    );
}

#[test]
fn row8_input_required_blocks() {
    for s in [Queued, Working] {
        let (next, cmds) = run(
            s,
            &status(AgentTaskState::InputRequired, Some("Which branch?")),
        );
        assert_eq!(next, Blocked);
        assert_eq!(
            bodies(&cmds),
            [
                &status_body(AgentStatus::InputRequired, Some("Which branch?")),
                &state_body(Blocked)
            ]
        );
    }
}

#[test]
fn row8_auth_required_blocks_with_prefixed_detail() {
    let (next, cmds) = run(
        Working,
        &status(AgentTaskState::AuthRequired, Some("github")),
    );
    assert_eq!(next, Blocked);
    assert_eq!(
        bodies(&cmds)[0],
        &status_body(
            AgentStatus::InputRequired,
            Some("authentication required: github")
        )
    );
    let (_, cmds) = run(Working, &status(AgentTaskState::AuthRequired, None));
    assert_eq!(
        bodies(&cmds)[0],
        &status_body(AgentStatus::InputRequired, Some("authentication required"))
    );
}

#[test]
fn row9_input_required_in_blocked() {
    assert_eq!(
        run(Blocked, &status(AgentTaskState::InputRequired, None)),
        (Blocked, vec![])
    );
    let (next, cmds) = run(
        Blocked,
        &status(AgentTaskState::InputRequired, Some("still?")),
    );
    assert_eq!(next, Blocked);
    assert_eq!(
        bodies(&cmds),
        [&status_body(AgentStatus::InputRequired, Some("still?"))]
    );
}

#[test]
fn row10_completed_finishes_the_thread() {
    for s in OPEN {
        let (next, cmds) = run(s, &status(AgentTaskState::Completed, None));
        assert_eq!(next, Done);
        assert_eq!(
            bodies(&cmds),
            [
                &status_body(AgentStatus::Completed, None),
                &state_body(Done)
            ]
        );
    }
}

#[test]
fn row11_failed_and_rejected_fail_the_thread() {
    for s in OPEN {
        let (next, cmds) = run(s, &status(AgentTaskState::Failed, Some("boom")));
        assert_eq!(next, Failed);
        assert_eq!(
            bodies(&cmds),
            [
                &status_body(AgentStatus::Failed, Some("boom")),
                &state_body(Failed)
            ]
        );
        let (next, cmds) = run(s, &status(AgentTaskState::Rejected, Some("no capacity")));
        assert_eq!(next, Failed);
        assert_eq!(
            bodies(&cmds)[0],
            &status_body(AgentStatus::Failed, Some("rejected: no capacity"))
        );
        let (_, cmds) = run(s, &status(AgentTaskState::Rejected, None));
        assert_eq!(
            bodies(&cmds)[0],
            &status_body(AgentStatus::Failed, Some("rejected"))
        );
    }
}

#[test]
fn row12_canceled_cancels_the_thread() {
    for s in OPEN {
        let (next, cmds) = run(s, &status(AgentTaskState::Canceled, None));
        assert_eq!(next, Cancelled);
        assert_eq!(
            bodies(&cmds),
            [
                &status_body(AgentStatus::Canceled, None),
                &state_body(Cancelled)
            ]
        );
    }
}

#[test]
fn row13_artifact_keeps_state() {
    let input = Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Artifact {
            name: "pr".into(),
            mime_type: None,
            uri: Some("https://github.com/acme/demo/pull/1".into()),
            text: None,
        },
    };
    for s in OPEN {
        let (next, cmds) = run(s, &input);
        assert_eq!(next, s);
        assert_eq!(
            bodies(&cmds),
            [&EventBody::Artifact(ArtifactData {
                name: "pr".into(),
                mime_type: None,
                uri: Some("https://github.com/acme/demo/pull/1".into()),
                text: None
            })]
        );
    }
}

#[test]
fn row14_message_keeps_state() {
    let input = Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Message {
            message_id: "m1".into(),
            text: "hello".into(),
            is_final: true,
        },
    };
    for s in OPEN {
        let (next, cmds) = run(s, &input);
        assert_eq!(next, s);
        assert_eq!(
            bodies(&cmds),
            [&EventBody::AgentMessage(AgentMessageData {
                text: "hello".into(),
                message_id: "m1".into(),
                is_final: true
            })]
        );
    }
}

#[test]
fn row15_agent_input_in_terminal_is_invalid() {
    for s in TERMINAL {
        for st in [AgentTaskState::Working, AgentTaskState::Completed] {
            assert_eq!(
                transition(&s, &status(st, None)),
                Err(TransitionError::InvalidInState {
                    state: s,
                    input: "agent update"
                })
            );
        }
    }
}

#[test]
fn row16_retryable_delivery_failure_blocks() {
    let input = Input::DeliveryFailed {
        reason: "agent unreachable".into(),
        retryable: true,
    };
    for s in [Queued, Working] {
        let (next, cmds) = run(s, &input);
        assert_eq!(next, Blocked);
        assert_eq!(
            bodies(&cmds),
            [
                &EventBody::Error(ErrorData {
                    message: "agent unreachable".into(),
                    retryable: true
                }),
                &state_body(Blocked)
            ]
        );
    }
    // Already blocked: no second `thread_state` (it only marks entering a state).
    let (next, cmds) = run(Blocked, &input);
    assert_eq!(next, Blocked);
    assert_eq!(cmds.len(), 1);
}

#[test]
fn row17_permanent_delivery_failure_fails() {
    let input = Input::DeliveryFailed {
        reason: "rejected".into(),
        retryable: false,
    };
    for s in OPEN {
        let (next, cmds) = run(s, &input);
        assert_eq!(next, Failed);
        assert_eq!(
            bodies(&cmds),
            [
                &EventBody::Error(ErrorData {
                    message: "rejected".into(),
                    retryable: false
                }),
                &state_body(Failed)
            ]
        );
    }
}

#[test]
fn row18_delivery_failure_in_terminal_only_records_an_error() {
    for s in TERMINAL {
        for retryable in [true, false] {
            let (next, cmds) = run(
                s,
                &Input::DeliveryFailed {
                    reason: "late".into(),
                    retryable,
                },
            );
            assert_eq!(next, s);
            assert_eq!(
                bodies(&cmds),
                [&EventBody::Error(ErrorData {
                    message: "late".into(),
                    retryable: false
                })]
            );
        }
    }
}

#[test]
fn row19_cancelled_before_start() {
    for s in OPEN {
        let (next, cmds) = run(s, &Input::CancelledBeforeStart);
        assert_eq!(next, Cancelled);
        assert_eq!(bodies(&cmds), [&state_body(Cancelled)]);
    }
}

#[test]
fn row20_cancel_rejected_records_an_error() {
    for s in OPEN {
        let (next, cmds) = run(
            s,
            &Input::CancelRejected {
                reason: "not cancelable".into(),
                retryable: true,
            },
        );
        assert_eq!(next, s);
        assert_eq!(
            bodies(&cmds),
            [&EventBody::Error(ErrorData {
                message: "not cancelable".into(),
                retryable: true
            })]
        );
    }
}

#[test]
fn row21_cancel_outcomes_in_terminal_are_noops() {
    for s in TERMINAL {
        assert_eq!(run(s, &Input::CancelledBeforeStart), (s, vec![]));
        assert_eq!(
            run(
                s,
                &Input::CancelRejected {
                    reason: "x".into(),
                    retryable: false
                }
            ),
            (s, vec![])
        );
    }
}

#[test]
fn every_state_is_covered_by_the_lists() {
    assert_eq!(ALL.len(), OPEN.len() + TERMINAL.len());
    for s in ALL {
        assert_eq!(s.is_terminal(), TERMINAL.contains(&s));
    }
}

#[test]
fn replay_is_deterministic() {
    let inputs = vec![
        um("go"),
        status(AgentTaskState::Working, None),
        status(AgentTaskState::InputRequired, Some("q?")),
        um("a"),
        status(AgentTaskState::Working, None),
        status(AgentTaskState::Completed, None),
    ];
    let fold = |inputs: &[Input]| {
        let mut state = Queued;
        let mut out = Vec::new();
        for i in inputs {
            let (next, cmds) = transition(&state, i).unwrap();
            state = next;
            out.push(cmds);
        }
        (state, out)
    };
    let a = fold(&inputs);
    let b = fold(&inputs);
    assert_eq!(a, b);
    assert_eq!(a.0, Done);
}
