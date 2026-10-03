//! One test per row of the transition table in the plan (§2.3).
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

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};

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
        message_id: None,
        run_id: None,
        origin: orch_core::Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
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
            Command::Delegate { .. }
            | Command::DelegateAction { .. }
            | Command::Steer { .. }
            | Command::DropQueued { .. }
            | Command::RequestCancel { .. }
            | Command::Watch { .. }
            | Command::Schedule { .. }
            | Command::SetTitle(_)
            | Command::RequestTitle { .. }
            | Command::SetDescription(_)
            | Command::RequestDescription { .. }
            | Command::Ask { .. }
            | Command::RequestVerification { .. } => None,
        })
        .collect()
}
/// The `user_message` data of a message sent to a thread in `state`: while a job runs the core
/// says it was steered (ADR 0036), otherwise nothing.
fn heard(state: ThreadState, text: &str) -> UserMessageData {
    UserMessageData {
        delivery: matches!(state, Queued | Working).then_some(Delivery::Steer),
        ..UserMessageData::new(text)
    }
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
                assert_eq!(d.body, EventBody::UserMessage(heard(s, "hi")));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            cmds[1],
            Command::Steer {
                text: "hi".into(),
                catalog: None,
                mentions: Vec::new(),
            }
        );
    }
}

#[test]
fn row1b_a_surface_names_the_message_and_the_run_and_the_log_records_both() {
    let input = Input::UserMessage {
        user: user(),
        text: "hi".into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: orch_core::Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
    };
    for s in [Queued, Working, Blocked] {
        let (_, cmds) = run(s, &input);
        match &cmds[0] {
            Command::Append(d) => assert_eq!(
                d.body,
                EventBody::UserMessage(UserMessageData {
                    text: "hi".into(),
                    message_id: Some("m-1".into()),
                    run_id: Some("r-1".into()),
                    origin: orch_core::Origin::Agui,
                    // sent while the job runs: the core says how it reaches the agent (ADR 0036)
                    delivery: matches!(s, Queued | Working).then_some(Delivery::Steer),
                    mentions: Vec::new(),
                })
            ),
            other => panic!("unexpected {other:?}"),
        }
        // The delivery carries the text only: the agent never sees surface ids. A message to a
        // running job is a steer; one that answers a blocked job is a delegation.
        let text = "hi".to_owned();
        assert_eq!(
            cmds[1],
            if matches!(s, Queued | Working) {
                Command::Steer {
                    text,
                    catalog: None,
                    mentions: Vec::new(),
                }
            } else {
                Command::Delegate {
                    text,
                    catalog: None,
                    mentions: Vec::new(),
                }
            }
        );
    }
}

#[test]
fn row1c_the_origin_of_a_message_is_recorded_in_the_log() {
    let input = Input::UserMessage {
        user: user(),
        text: "hi".into(),
        message_id: None,
        run_id: None,
        origin: orch_core::Origin::Mcp,
        catalog: None,
        mentions: Vec::new(),
    };
    for s in [Queued, Working, Blocked] {
        let (_, cmds) = run(s, &input);
        match &cmds[0] {
            Command::Append(d) => assert_eq!(
                d.body,
                EventBody::UserMessage(UserMessageData {
                    origin: orch_core::Origin::Mcp,
                    ..heard(s, "hi")
                })
            ),
            other => panic!("unexpected {other:?}"),
        }
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
            text: "main".into(),
            catalog: None,
            mentions: Vec::new(),
        }
    );
}

#[test]
fn row3_user_message_in_terminal_starts_the_next_job() {
    for s in TERMINAL {
        let (next, cmds) = run(s, &um("again"));
        assert_eq!(next, Queued, "{s:?}");
        assert_eq!(cmds.len(), 3, "{cmds:?}");
        assert_eq!(
            bodies(&cmds),
            [
                &EventBody::UserMessage(UserMessageData::new("again")),
                &EventBody::JobStarted(JobStartedData { job: 2 }),
            ]
        );
        // The job starts after the message it answers, and is the system's word, not the user's.
        match (&cmds[0], &cmds[1]) {
            (Command::Append(m), Command::Append(j)) => {
                assert_eq!(m.actor, Actor::user(&user()));
                assert_eq!(j.actor, Actor::system());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            cmds[2],
            Command::Delegate {
                text: "again".into(),
                catalog: None,
                mentions: Vec::new(),
            }
        );
        // No `thread_state`: entering `queued` is implied by the message.
        assert!(
            !bodies(&cmds)
                .iter()
                .any(|b| matches!(b, EventBody::ThreadState(_)))
        );
    }
}

#[test]
fn row3b_the_next_job_keeps_the_gate_and_the_verification_count_and_clears_the_rest() {
    let mut gate = GatePolicy::requiring([CheckSource::AgentChecks, CheckSource::Verifier]);
    gate.max_attempts = 2;
    let sha = "a".repeat(40);
    // the catalogs the conversation has seen belong to it, not to the job
    let mut catalog = UiCatalogLedger::default();
    catalog.observe(&UiCatalogRef {
        catalog_id: "https://agents.vymalo.com/a2ui/catalogs/chat".into(),
        version: 2,
        digest: format!("sha256:{}", "b".repeat(64)),
    });
    // a step the old job left open is the old job's
    let mut steps_job = Job::default();
    orch_core::record_step(
        Working,
        &mut steps_job,
        Actor::system(),
        &StepReport {
            id: "t/s1".into(),
            parent: None,
            kind: StepKind::Tool,
            label: "tool".into(),
            state: StepState::Running,
            icon: None,
            detail: None,
            input: None,
            output: None,
        },
        StepSource::Agent,
    );
    assert_eq!(steps_job.steps.open_count(), 1);
    // a turn that announced its answer: the next job does not inherit it
    let announced = orch_core::transition(
        &Snapshot::new(Working),
        &Input::Answer {
            actor: Actor::agent(&agent(), None),
            text: "The result.".into(),
            job: 1,
            token: "j".into(),
        },
    )
    .unwrap()
    .0
    .job
    .answer;
    assert!(announced.is_announced());
    for s in TERMINAL {
        let before = Snapshot {
            state: s,
            job: Job {
                after_stop: None,
                number: 3,
                gate: gate.clone(),
                attempt: 2,
                verification: 5,
                task: Some("first".into()),
                branch_problem: Some("why".into()),
                summary: Some("did things".into()),
                pushed: Some(PushedRef {
                    repository: "https://github.com/o/r".into(),
                    branch: "agent/x".into(),
                    commit: sha.clone(),
                }),
                results: vec![CheckResult {
                    source: CheckSource::AgentChecks,
                    name: None,
                    attempt: 2,
                    commit: Some(sha.clone()),
                    status: CheckStatus::Passed,
                    summary: None,
                    stale: false,
                    findings: vec![],
                }],
                hold: None,
                catalog: catalog.clone(),
                steps: steps_job.steps.clone(),
                answer: announced.clone(),
                title: TitleLedger::default(),
                description: DescriptionLedger::default(),
                tools: vec!["docs".into(), "websearch".into()],
                // the agents of the finished job's messages are not the next job's
                mentioned: [AgentId::new("researcher")].into(),
                after_stop_mentions: Vec::new(),
                // an ask of the finished job (ended with its task): the next job numbers its own
                asks: vec![Ask {
                    n: 1,
                    by: Caller::Main,
                    agent: AgentId::new("researcher"),
                    depth: 1,
                    call_key: None,
                    task_id: Some("t".into()),
                    outcome: Some(AskOutcome::Canceled),
                }],
            },
        };
        let (after, cmds) = orch_core::transition(&before, &um("next")).unwrap();
        assert_eq!(after.state, Queued);
        assert_eq!(
            after.job,
            Job {
                number: 4,
                gate: gate.clone(),
                attempt: 1,
                verification: 5,
                task: Some("next".into()),
                catalog: catalog.clone(),
                // the servers attached to the conversation go with it into the next job
                tools: vec!["docs".into(), "websearch".into()],
                ..Job::default()
            }
        );
        assert!(
            bodies(&cmds)
                .iter()
                .any(|b| **b == EventBody::JobStarted(JobStartedData { job: 4 }))
        );
    }
}

/// Open question 33 (ADR 0036): a message the agent took as a task of its own while the thread was
/// ending is applied as a redelivery that was **sent**: the same transition, and no delegation.
#[test]
fn a_redelivery_that_was_sent_changes_the_thread_as_one_that_was_not_and_delegates_nothing() {
    let sent = |text: &str| Input::Redeliver {
        text: text.into(),
        sent: true,
        mentions: Vec::new(),
    };
    for s in [Done, Failed] {
        let (next, cmds) = run(s, &sent("x"));
        assert_eq!(next, Queued, "{s:?}");
        // the next job starts, and nothing is delegated: the task is the agent's already
        assert_eq!(
            bodies(&cmds),
            [&EventBody::JobStarted(JobStartedData { job: 2 })],
            "{s:?}"
        );
        assert_eq!(cmds.len(), 1, "{s:?}");
    }
    // the same states as an unsent one reaches, minus the command
    for (s, expected) in [
        (Queued, Queued),
        (Working, Working),
        (Blocked, Queued),
        (Verifying, Queued),
    ] {
        assert_eq!(run(s, &sent("x")), (expected, vec![]), "{s:?}");
    }
    // the person asked to stop: nothing
    assert_eq!(run(Cancelled, &sent("x")), (Cancelled, vec![]));
    // an open job has the message in its task already (what a rework or a verifier is told), from
    // when it was written: it is not added a second time; a job it starts is told it
    let before = Snapshot {
        state: Verifying,
        job: Job {
            task: Some("first".into()),
            ..Job::with_gate(GatePolicy::requiring([CheckSource::AgentChecks]))
        },
    };
    let (after, _) = orch_core::transition(&before, &sent("second thoughts")).unwrap();
    assert_eq!(after.state, Queued);
    assert_eq!(after.job.task.as_deref(), Some("first"));
    assert_eq!(after.job.number, before.job.number);
    assert!(after.job.hold.is_none());
}

#[test]
fn row3c_redelivery_starts_the_next_job_unless_the_person_stopped() {
    for s in [Done, Failed] {
        let (next, cmds) = run(
            s,
            &Input::Redeliver {
                text: "x".into(),
                sent: false,
                mentions: Vec::new(),
            },
        );
        assert_eq!(next, Queued);
        // The `user_message` is in the log already: only the boundary and the delegation.
        assert_eq!(
            bodies(&cmds),
            [&EventBody::JobStarted(JobStartedData { job: 2 })]
        );
        assert_eq!(
            cmds[1],
            Command::Delegate {
                text: "x".into(),
                catalog: None,
                mentions: Vec::new(),
            }
        );
    }
    assert_eq!(
        run(
            Cancelled,
            &Input::Redeliver {
                text: "x".into(),
                sent: false,
                mentions: Vec::new()
            }
        ),
        (Cancelled, vec![])
    );
    // Still open: it only delegates, and a blocked thread is answered.
    for (s, expected) in [(Queued, Queued), (Working, Working), (Blocked, Queued)] {
        assert_eq!(
            run(
                s,
                &Input::Redeliver {
                    text: "x".into(),
                    sent: false,
                    mentions: Vec::new()
                }
            ),
            (
                expected,
                vec![Command::Delegate {
                    text: "x".into(),
                    catalog: None,
                    mentions: Vec::new(),
                }]
            )
        );
    }
}

#[test]
fn row4_cancel_requests_cancel_when_open() {
    for s in OPEN {
        assert_eq!(
            run(s, &Input::Cancel { user: user() }),
            (s, vec![Command::RequestCancel { job: 1 }])
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
fn row8_auth_required_blocks_with_its_own_status() {
    // Same thread behaviour as input-required (blocked, resumable), but the status says what
    // it is and the detail is the agent's own text, not a prefixed sentence.
    let (next, cmds) = run(
        Working,
        &status(AgentTaskState::AuthRequired, Some("github")),
    );
    assert_eq!(next, Blocked);
    assert_eq!(
        bodies(&cmds),
        [
            &status_body(AgentStatus::AuthRequired, Some("github")),
            &state_body(Blocked)
        ]
    );
    let (next, cmds) = run(Queued, &status(AgentTaskState::AuthRequired, None));
    assert_eq!(next, Blocked);
    assert_eq!(
        bodies(&cmds),
        [
            &status_body(AgentStatus::AuthRequired, None),
            &state_body(Blocked)
        ]
    );
}

#[test]
fn row8b_auth_required_in_blocked_repeats_only_with_detail() {
    assert_eq!(
        run(Blocked, &status(AgentTaskState::AuthRequired, None)),
        (Blocked, vec![])
    );
    let (next, cmds) = run(
        Blocked,
        &status(AgentTaskState::AuthRequired, Some("gitlab")),
    );
    assert_eq!(next, Blocked);
    assert_eq!(
        bodies(&cmds),
        [&status_body(AgentStatus::AuthRequired, Some("gitlab"))]
    );
}

#[test]
fn row8c_auth_required_is_resumable_by_a_user_message() {
    let (next, cmds) = run(Blocked, &um("signed in"));
    assert_eq!(next, Queued);
    assert!(cmds.iter().any(|c| matches!(c, Command::Delegate { .. })));
}

#[test]
fn row8e_auth_and_input_required_take_the_same_thread_path() {
    // The new status changes what the event says, never what the thread does.
    for s in ALL {
        for detail in [None, Some("x")] {
            let auth = transition(&s, &status(AgentTaskState::AuthRequired, detail));
            let input = transition(&s, &status(AgentTaskState::InputRequired, detail));
            match (auth, input) {
                (Ok((a_next, a_cmds)), Ok((i_next, i_cmds))) => {
                    assert_eq!(a_next, i_next, "{s:?}/{detail:?}");
                    assert_eq!(a_cmds.len(), i_cmds.len(), "{s:?}/{detail:?}");
                    for (a, i) in a_cmds.iter().zip(&i_cmds) {
                        match (a, i) {
                            (Command::Append(a), Command::Append(i)) => {
                                assert_eq!(a.body.kind(), i.body.kind());
                                assert_eq!(a.actor, i.actor);
                            }
                            (a, i) => assert_eq!(a, i),
                        }
                    }
                }
                (a, i) => assert_eq!(a, i, "{s:?}/{detail:?}"),
            }
        }
    }
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
                text: None,
                file: None
            })]
        );
    }
}

fn digest() -> String {
    "ab".repeat(32)
}

fn file_input(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update,
    }
}

/// ADR 0032: a file the worker kept is an artifact that holds the reference.
#[test]
fn row13a_a_kept_file_is_an_artifact_with_its_reference() {
    let file = FileRef {
        sha256: digest(),
        size: 4,
        filename: Some("chart.png".into()),
    };
    let input = file_input(AgentUpdate::FileKept {
        name: "chart".into(),
        mime_type: "image/png".into(),
        file: file.clone(),
    });
    for s in OPEN {
        let (next, cmds) = run(s, &input);
        assert_eq!(next, s);
        assert_eq!(
            bodies(&cmds),
            [&EventBody::Artifact(ArtifactData {
                name: "chart".into(),
                mime_type: Some("image/png".into()),
                uri: None,
                text: None,
                file: Some(file.clone()),
            })]
        );
    }
}

/// A file that was not kept is an artifact without a file and an error that says why, whatever
/// the reason; the turn goes on (the state does not change).
#[test]
fn row13b_a_refused_file_is_an_entry_without_a_file_and_an_error() {
    for (reason, words) in [
        (FileRefusal::TooLarge, "the file is too large to keep"),
        (FileRefusal::NotKept, "the file could not be kept"),
        (
            FileRefusal::JobLimit,
            "this job has reached its limit of files, so the file is not kept",
        ),
    ] {
        let input = file_input(AgentUpdate::FileRefused {
            name: "dump".into(),
            mime_type: Some("application/zip".into()),
            reason,
        });
        for s in OPEN {
            let (next, cmds) = run(s, &input);
            assert_eq!(next, s);
            assert_eq!(
                bodies(&cmds),
                [
                    &EventBody::Artifact(ArtifactData {
                        name: "dump".into(),
                        mime_type: Some("application/zip".into()),
                        uri: None,
                        text: None,
                        file: None,
                    }),
                    &EventBody::Error(ErrorData {
                        message: words.into(),
                        retryable: false,
                    }),
                ]
            );
        }
    }
}

/// Bytes that reach the core were put nowhere: never logged, logged as not kept.
#[test]
fn row13c_bytes_that_reach_the_core_are_never_logged() {
    let input = file_input(AgentUpdate::File {
        name: "raw".into(),
        media_type: None,
        filename: Some("raw.bin".into()),
        bytes: vec![1, 2, 3],
    });
    let (next, cmds) = run(Working, &input);
    assert_eq!(next, Working);
    let shown = format!("{:?}", bodies(&cmds));
    assert!(shown.contains("the file could not be kept"), "{shown}");
    assert!(!shown.contains("[1, 2, 3]"), "{shown}");
    for s in TERMINAL {
        assert!(transition(&s, &input).is_err());
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
            purpose: None,
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
                is_final: true,
                purpose: None,
                via: None,
            })]
        );
    }
}

#[test]
fn row14_message_copies_what_its_words_are_for_to_the_event() {
    for purpose in [MessagePurpose::Working, MessagePurpose::Answer] {
        let input = Input::Agent {
            agent: agent(),
            revision: None,
            update: AgentUpdate::Message {
                message_id: "m1".into(),
                text: "hello".into(),
                is_final: true,
                purpose: Some(purpose),
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
                    is_final: true,
                    purpose: Some(purpose),
                    // reserved: nothing in the core says how an answer was announced yet
                    via: None,
                })]
            );
        }
    }
}

#[test]
fn row15_agent_input_in_terminal_is_invalid() {
    for s in TERMINAL {
        for st in [
            AgentTaskState::Working,
            AgentTaskState::Completed,
            AgentTaskState::AuthRequired,
        ] {
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

fn step_report(state: StepState) -> StepReport {
    StepReport {
        id: "t/s1".into(),
        parent: None,
        kind: StepKind::Command,
        label: "npm test".into(),
        state,
        icon: Some("execute".into()),
        detail: None,
        input: None,
        output: None,
    }
}

fn step_body(phase: StepPhase, state: StepState) -> EventBody {
    EventBody::AgentStep(AgentStepData {
        id: "t/s1".into(),
        path: vec![],
        kind: StepKind::Command,
        label: "npm test".into(),
        state,
        phase,
        icon: Some("execute".into()),
        detail: None,
        input: None,
        output: None,
        io_dropped: false,
    })
}

#[test]
fn row14b_a_step_is_the_sign_of_work_and_keeps_the_thread_working() {
    let input = Input::Agent {
        agent: agent(),
        revision: Some("rev-1".into()),
        update: AgentUpdate::Step(step_report(StepState::Running)),
    };
    for s in [Queued, Working] {
        let (next, cmds) = run(s, &input);
        assert_eq!(next, Working);
        assert_eq!(
            bodies(&cmds),
            [&step_body(StepPhase::Start, StepState::Running)]
        );
        let Command::Append(d) = &cmds[0] else {
            panic!("{cmds:?}")
        };
        assert_eq!(d.actor, Actor::agent(&agent(), Some("rev-1".into())));
    }
    // the work is not going on while the thread waits for the user or is verified: dropped
    for s in [Blocked, Verifying] {
        let (next, cmds) = orch_core::transition(&Snapshot::new(s), &input).unwrap();
        assert_eq!(next.state, s);
        assert!(cmds.is_empty(), "{s:?}");
    }
}

#[test]
fn row15b_a_step_for_a_finished_thread_is_invalid() {
    let inputs = [
        Input::Agent {
            agent: agent(),
            revision: None,
            update: AgentUpdate::Step(step_report(StepState::Running)),
        },
        Input::Step {
            actor: Actor::system(),
            report: step_report(StepState::Running),
        },
    ];
    for s in TERMINAL {
        for input in &inputs {
            assert_eq!(
                transition(&s, input),
                Err(TransitionError::InvalidInState {
                    state: s,
                    input: input.name()
                })
            );
        }
    }
    assert_eq!(inputs[0].name(), "agent update");
    assert_eq!(inputs[1].name(), "step");
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
                agent: AgentId::new("coder"),
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
                    agent: AgentId::new("coder"),
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

// ---------------------------------------------------------------- A2UI (ADR 0013)

fn surface_op() -> serde_json::Value {
    serde_json::json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s1"}})
}
fn ui(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: agent(),
        revision: Some("rev-1".into()),
        update,
    }
}
fn act() -> UiActionData {
    UiActionData {
        surface_id: "s1".into(),
        name: "submit".into(),
        source_component_id: "btn".into(),
        context: serde_json::Map::new(),
        version: UiVersion::V0_9_1,
        run_id: Some("run-2".into()),
    }
}
fn ui_action(action: UiActionData) -> Input {
    Input::UiAction {
        user: user(),
        action,
        catalog: None,
    }
}

#[test]
fn row_ui1_a_surface_is_recorded_without_moving_the_thread() {
    let update = AgentUpdate::Ui {
        operations: vec![surface_op()],
    };
    for s in OPEN {
        let (next, cmds) = run(s, &ui(update.clone()));
        assert_eq!(next, s);
        let want = EventBody::UiSurface(UiSurfaceData {
            operations: vec![surface_op()],
        });
        assert_eq!(bodies(&cmds), [&want]);
        let Command::Append(draft) = &cmds[0] else {
            panic!("not an append");
        };
        assert_eq!(draft.actor, Actor::agent(&agent(), Some("rev-1".into())));
    }
}

#[test]
fn row_ui2_a_surface_for_a_finished_thread_is_a_late_update() {
    let update = AgentUpdate::Ui {
        operations: vec![surface_op()],
    };
    for s in TERMINAL {
        assert!(matches!(
            transition(&s, &ui(update.clone())),
            Err(TransitionError::InvalidInState { .. })
        ));
    }
}

#[test]
fn row_ui3_a_refused_part_is_an_error_event_and_nothing_else() {
    let update = AgentUpdate::UiRejected {
        reason: "message 0: no version".into(),
    };
    for s in OPEN {
        let (next, cmds) = run(s, &ui(update.clone()));
        assert_eq!(next, s, "the turn goes on");
        let [EventBody::Error(e)] = bodies(&cmds)[..] else {
            panic!("{cmds:?}");
        };
        assert!(!e.retryable);
        assert!(e.message.contains("message 0: no version"), "{}", e.message);
    }
}

#[test]
fn row_ui4_an_action_is_a_user_event_and_a_delegation() {
    for s in [Queued, Working] {
        let (next, cmds) = run(s, &ui_action(act()));
        assert_eq!(next, s);
        assert_eq!(bodies(&cmds), [&EventBody::UiAction(act())]);
        let Command::Append(draft) = &cmds[0] else {
            panic!("not an append");
        };
        assert_eq!(draft.actor, Actor::user(&user()));
        assert_eq!(
            cmds[1],
            Command::DelegateAction {
                action: act(),
                catalog: None
            }
        );
    }
}

#[test]
fn row_ui5_an_action_answers_a_blocked_thread_like_a_message() {
    let (next, cmds) = run(Blocked, &ui_action(act()));
    assert_eq!(next, Queued);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::DelegateAction { .. }))
    );
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::Delegate { .. })),
        "an action carries no text"
    );
}

#[test]
fn row_ui6_an_action_on_a_finished_thread_is_refused() {
    for s in TERMINAL {
        assert_eq!(
            transition(&s, &ui_action(act())),
            Err(TransitionError::Finished { state: s })
        );
    }
}

#[test]
fn row_ui7_an_unchecked_payload_never_reaches_the_log() {
    // An adapter that forgot the envelope check: the core does it again.
    let junk = [
        vec![],
        vec![serde_json::json!({"nonsense": true})],
        vec![serde_json::json!({"version": "v0.8", "createSurface": {"surfaceId": "s"}})],
        vec![serde_json::json!("x"); orch_core::MAX_OPERATIONS + 1],
    ];
    for operations in junk {
        for s in OPEN {
            let (next, cmds) = run(
                s,
                &ui(AgentUpdate::Ui {
                    operations: operations.clone(),
                }),
            );
            assert_eq!(next, s);
            let [EventBody::Error(e)] = bodies(&cmds)[..] else {
                panic!("{cmds:?}");
            };
            assert!(!e.retryable);
            assert!(e.message.contains("refused"), "{}", e.message);
        }
    }
}

// ---- the UI catalog (ADR 0023) -------------------------------------------------------------

/// A version of the UI's catalog with its real digest; two of the same version differ by `tag`.
fn catalog(version: u32, tag: &str) -> UiCatalogData {
    let id = "https://agents.vymalo.com/a2ui/catalogs/chat";
    let catalog = serde_json::json!({
        "catalogId": id,
        "components": {"Note": {"type": "object", "title": format!("{tag}-{version}")}},
    });
    UiCatalogData {
        catalog_id: id.into(),
        version,
        digest: catalog_digest(&catalog).unwrap(),
        catalog,
    }
}
fn um_with(text: &str, catalog: &UiCatalogData) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: orch_core::Origin::Agui,
        catalog: Some(catalog.clone()),
        mentions: Vec::new(),
    }
}
fn action_with(catalog: &UiCatalogData) -> Input {
    Input::UiAction {
        user: user(),
        action: act(),
        catalog: Some(catalog.clone()),
    }
}
/// The thread's snapshot after it has been shown `catalogs`, in order.
fn after_catalogs(state: ThreadState, catalogs: &[UiCatalogData]) -> Snapshot {
    let mut snap = Snapshot::new(state);
    for c in catalogs {
        snap.job.catalog.accept(Some(c));
    }
    snap
}
/// What the commands tell the agent of the catalog: the delivery of the only delegation.
fn delivery(cmds: &[Command]) -> Option<&UiDelivery> {
    let mut found = cmds.iter().filter_map(|c| match c {
        Command::Delegate { catalog, .. }
        | Command::Steer { catalog, .. }
        | Command::DelegateAction { catalog, .. } => Some(catalog.as_ref()),
        Command::Append(_)
        | Command::DropQueued { .. }
        | Command::RequestCancel { .. }
        | Command::Watch { .. }
        | Command::Schedule { .. }
        | Command::SetTitle(_)
        | Command::RequestTitle { .. }
        | Command::SetDescription(_)
        | Command::RequestDescription { .. }
        | Command::Ask { .. }
        | Command::RequestVerification { .. } => None,
    });
    let only = found.next().expect("a delegation");
    assert!(found.next().is_none(), "one delegation: {cmds:?}");
    only
}

#[test]
fn row_cat1_the_first_message_with_a_catalog_records_it_first_and_delivers_it_inline() {
    let v1 = catalog(1, "a");
    for s in [Queued, Working, Blocked, ThreadState::Verifying] {
        let (after, cmds) = orch_core::transition(&Snapshot::new(s), &um_with("hi", &v1)).unwrap();
        assert_eq!(
            bodies(&cmds),
            [
                &EventBody::UiCatalog(v1.clone()),
                &EventBody::UserMessage(heard(s, "hi")),
            ],
            "{s:?}"
        );
        // the person's screen sent it: the event is the user's, and first in the commit
        let Command::Append(first) = &cmds[0] else {
            panic!("not an append: {cmds:?}");
        };
        assert_eq!(first.actor, Actor::user(&user()));
        assert_eq!(
            delivery(&cmds),
            Some(&UiDelivery::Inline(v1.clone())),
            "{s:?}"
        );
        assert_eq!(after.job.catalog.current(), Some(&v1.reference()));
        assert_eq!(after.job.catalog.seen(), std::slice::from_ref(&v1.digest));
    }
}

#[test]
fn row_cat2_the_same_digest_again_writes_no_event_and_comes_as_a_reference() {
    let v1 = catalog(1, "a");
    let before = after_catalogs(Working, std::slice::from_ref(&v1));
    let (after, cmds) = orch_core::transition(&before, &um_with("again", &v1)).unwrap();
    assert_eq!(
        bodies(&cmds),
        [&EventBody::UserMessage(heard(Working, "again"))]
    );
    assert_eq!(delivery(&cmds), Some(&UiDelivery::Ref(v1.reference())));
    assert_eq!(after.job.catalog, before.job.catalog);
}

#[test]
fn row_cat3_a_newer_version_is_recorded_and_delivered_inline() {
    let (v1, v2) = (catalog(1, "a"), catalog(2, "a"));
    let before = after_catalogs(Working, &[v1]);
    let (after, cmds) = orch_core::transition(&before, &um_with("newer", &v2)).unwrap();
    assert_eq!(
        bodies(&cmds),
        [
            &EventBody::UiCatalog(v2.clone()),
            &EventBody::UserMessage(heard(Working, "newer")),
        ]
    );
    assert_eq!(delivery(&cmds), Some(&UiDelivery::Inline(v2.clone())));
    assert_eq!(after.job.catalog.current(), Some(&v2.reference()));
}

#[test]
fn row_cat4_an_older_version_is_recorded_once_and_the_agent_is_told_the_newest() {
    let (v1, v2) = (catalog(1, "a"), catalog(2, "a"));
    let before = after_catalogs(Working, std::slice::from_ref(&v2));
    let (after, cmds) = orch_core::transition(&before, &um_with("older", &v1)).unwrap();
    assert_eq!(
        bodies(&cmds),
        [
            &EventBody::UiCatalog(v1.clone()),
            &EventBody::UserMessage(heard(Working, "older")),
        ]
    );
    assert_eq!(delivery(&cmds), Some(&UiDelivery::Ref(v2.reference())));
    assert_eq!(after.job.catalog.current(), Some(&v2.reference()));
    // a second time, it is known
    let (_, cmds) = orch_core::transition(&after, &um_with("older again", &v1)).unwrap();
    assert_eq!(
        bodies(&cmds),
        [&EventBody::UserMessage(heard(Working, "older again"))]
    );
}

#[test]
fn row_cat5_a_message_without_a_catalog_carries_a_reference_to_the_current_one() {
    let v1 = catalog(1, "a");
    let with = after_catalogs(Working, std::slice::from_ref(&v1));
    let (after, cmds) = orch_core::transition(&with, &um("plain")).unwrap();
    assert_eq!(
        bodies(&cmds),
        [&EventBody::UserMessage(heard(Working, "plain"))]
    );
    assert_eq!(delivery(&cmds), Some(&UiDelivery::Ref(v1.reference())));
    assert_eq!(after.job, with.job, "nothing about the screen changed");
    // a thread that was never shown one carries none, as before
    let (_, cmds) = orch_core::transition(&Snapshot::new(Working), &um("plain")).unwrap();
    assert_eq!(delivery(&cmds), None);
}

#[test]
fn row_cat6_an_action_with_a_newer_catalog_answers_a_blocked_thread_and_delivers_it() {
    let (v1, v2) = (catalog(1, "a"), catalog(2, "a"));
    let before = after_catalogs(Blocked, &[v1]);
    let (after, cmds) = orch_core::transition(&before, &action_with(&v2)).unwrap();
    assert_eq!(after.state, Queued);
    assert_eq!(
        bodies(&cmds),
        [
            &EventBody::UiCatalog(v2.clone()),
            &EventBody::UiAction(act())
        ]
    );
    assert_eq!(delivery(&cmds), Some(&UiDelivery::Inline(v2.clone())));
    assert!(matches!(cmds[2], Command::DelegateAction { .. }));
    assert_eq!(after.job.catalog.current(), Some(&v2.reference()));
}

#[test]
fn row_cat7_an_action_on_a_finished_thread_records_nothing_even_with_a_catalog() {
    let v1 = catalog(1, "a");
    for s in TERMINAL {
        let before = after_catalogs(s, &[]);
        assert_eq!(
            orch_core::transition(&before, &action_with(&v1)),
            Err(TransitionError::Finished { state: s })
        );
    }
}

#[test]
fn row_cat8_a_message_that_starts_the_next_job_records_its_catalog_before_the_boundary() {
    let (v1, v2) = (catalog(1, "a"), catalog(2, "a"));
    for s in TERMINAL {
        let before = after_catalogs(s, std::slice::from_ref(&v1));
        let (after, cmds) = orch_core::transition(&before, &um_with("next", &v2)).unwrap();
        assert_eq!(after.state, Queued, "{s:?}");
        assert_eq!(
            bodies(&cmds),
            [
                &EventBody::UiCatalog(v2.clone()),
                &EventBody::UserMessage(UserMessageData::new("next")),
                &EventBody::JobStarted(JobStartedData { job: 2 }),
            ],
            "{s:?}"
        );
        assert!(matches!(cmds[3], Command::Delegate { .. }));
        assert_eq!(delivery(&cmds), Some(&UiDelivery::Inline(v2.clone())));
        assert_eq!(after.job.catalog.current(), Some(&v2.reference()));
        assert_eq!(after.job.catalog.seen().len(), 2);
    }
}

#[test]
fn row_cat9_a_new_job_without_a_catalog_keeps_the_conversations_and_a_redelivery_names_it() {
    let v1 = catalog(1, "a");
    for s in [Done, Failed] {
        let before = after_catalogs(s, std::slice::from_ref(&v1));
        let (after, cmds) = orch_core::transition(&before, &um("next")).unwrap();
        assert_eq!(
            delivery(&cmds),
            Some(&UiDelivery::Ref(v1.reference())),
            "{s:?}"
        );
        assert_eq!(after.job.catalog, before.job.catalog, "{s:?}");
        let (after, cmds) = orch_core::transition(
            &before,
            &Input::Redeliver {
                text: "x".into(),
                sent: false,
                mentions: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(
            delivery(&cmds),
            Some(&UiDelivery::Ref(v1.reference())),
            "{s:?}"
        );
        assert_eq!(after.job.catalog, before.job.catalog, "{s:?}");
    }
    for s in [Queued, Working, Blocked] {
        let before = after_catalogs(s, std::slice::from_ref(&v1));
        let (_, cmds) = orch_core::transition(
            &before,
            &Input::Redeliver {
                text: "x".into(),
                sent: false,
                mentions: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(
            delivery(&cmds),
            Some(&UiDelivery::Ref(v1.reference())),
            "{s:?}"
        );
    }
}

#[test]
fn row16_a_rename_is_valid_in_every_state_and_changes_nothing_but_the_title() {
    let rename = Input::Rename {
        user: user(),
        title: "Fix the build".into(),
    };
    for s in ALL {
        let (next, cmds) = run(s, &rename);
        assert_eq!(next, s, "{s:?}");
        assert_eq!(
            cmds,
            vec![
                Command::Append(EventDraft {
                    actor: Actor::user(&user()),
                    body: EventBody::ThreadTitled(ThreadTitledData {
                        title: "Fix the build".into(),
                        source: TitledBy::User,
                    }),
                }),
                Command::SetTitle("Fix the build".into()),
            ],
            "{s:?}"
        );
    }
    // being verified is a state of the thread as well
    let (next, cmds) = run(Verifying, &rename);
    assert_eq!(next, Verifying);
    assert_eq!(cmds.len(), 2);
    assert_eq!(rename.name(), "rename");
}
