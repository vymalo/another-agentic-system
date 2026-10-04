//! Stop & send (ADR 0036): a person sends a message while a job runs and asks the agent to stop
//! it. One test per row of the `after_stop` table, then what the gate must not do for the job
//! that was abandoned, and the wire compatibility of the ledgers written before.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;
use serde_json::json;

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};

const S1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RUNNING: [ThreadState; 2] = [Queued, Working];
const NOT_RUNNING: [ThreadState; 5] = [Blocked, Verifying, Done, Failed, Cancelled];

fn agent() -> AgentId {
    AgentId::new("coder")
}
fn user() -> UserId {
    UserId::new("me@example.com")
}
fn said(text: &str) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
    }
}
fn stop(text: &str) -> Input {
    Input::StopAndSend {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
    }
}
fn agent_input(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update,
    }
}
fn task(state: AgentTaskState) -> Input {
    agent_input(AgentUpdate::Status {
        state,
        detail: None,
    })
}
fn artifact(name: &str, data: serde_json::Value) -> Input {
    agent_input(AgentUpdate::Artifact {
        name: name.into(),
        mime_type: Some("application/json".into()),
        uri: None,
        text: Some(data.to_string()),
    })
}
fn branch(sha: &str) -> Input {
    artifact(
        "branch",
        json!({"repository": "https://GitHub.com/Vymalo/Repo.git", "branch": "agent/x", "commit": sha}),
    )
}
fn cancel_rejected(retryable: bool) -> Input {
    Input::CancelRejected {
        agent: agent(),
        reason: "the agent said no".into(),
        retryable,
    }
}

/// A thread whose job runs under `state`, with no gate.
fn running(state: ThreadState) -> Snapshot {
    Snapshot::new(state)
}
/// A thread under a gate that requires the agent's checks, CI and a verifier.
fn gated(state: ThreadState) -> Snapshot {
    let mut gate = GatePolicy::requiring([
        CheckSource::AgentChecks,
        CheckSource::Ci,
        CheckSource::Verifier,
    ]);
    gate.ci.required = ["build".to_owned()].into();
    gate.verifier = Some(AgentId::new("reviewer"));
    let mut snap = Snapshot::queued(gate);
    snap.state = state;
    snap
}
/// The same thread with the person's first message in the job, as a gated job always has.
fn gated_with_task(state: ThreadState) -> Snapshot {
    let (snap, _) = transition(&gated(state), &said("first")).unwrap();
    Snapshot { state, ..snap }
}

fn step(snap: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snap, input).unwrap()
}
fn feed(mut snap: Snapshot, inputs: &[Input]) -> (Snapshot, Vec<Command>) {
    let mut all = Vec::new();
    for input in inputs {
        let (next, cmds) = step(&snap, input);
        snap = next;
        all.extend(cmds);
    }
    (snap, all)
}
fn bodies(cmds: &[Command]) -> Vec<&EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(&d.body),
            _ => None,
        })
        .collect()
}
/// The commands that are not events.
fn orders(cmds: &[Command]) -> Vec<&Command> {
    cmds.iter()
        .filter(|c| !matches!(c, Command::Append(_)))
        .collect()
}
fn kinds(cmds: &[Command]) -> Vec<&'static str> {
    bodies(cmds)
        .iter()
        .map(|b| match b {
            EventBody::UserMessage(_) => "user_message",
            EventBody::AgentStatus(_) => "agent_status",
            EventBody::ThreadState(_) => "thread_state",
            EventBody::JobStarted(_) => "job_started",
            EventBody::Error(_) => "error",
            EventBody::UiCatalog(_) => "ui_catalog",
            EventBody::CheckResult(_) => "check_result",
            EventBody::Rework(_) => "rework",
            _ => "other",
        })
        .collect()
}
fn message_data(cmds: &[Command]) -> &UserMessageData {
    bodies(cmds)
        .into_iter()
        .find_map(|b| match b {
            EventBody::UserMessage(m) => Some(m),
            _ => None,
        })
        .expect("a user_message")
}
fn error_message(cmds: &[Command]) -> String {
    bodies(cmds)
        .into_iter()
        .find_map(|b| match b {
            EventBody::Error(e) => Some(e.message.clone()),
            _ => None,
        })
        .expect("an error event")
}
fn status_event(status: AgentStatus, detail: Option<&str>) -> EventBody {
    EventBody::AgentStatus(AgentStatusData {
        status,
        detail: detail.map(Into::into),
    })
}
/// The commands that start the next job from `text`, in the order the ADR gives them.
fn next_job(text: &str, abandoned: u32) -> [Command; 2] {
    [
        Command::DropQueued { job: abandoned },
        Command::Delegate {
            text: text.into(),
            catalog: None,
            mentions: Vec::new(),
        },
    ]
}
/// The thread has been stopped: `after_stop` is set and the job is `state`.
fn stopping(state: ThreadState, text: &str) -> Snapshot {
    step(&running(state), &stop(text)).0
}

// ---- row 1: the first Stop & send ------------------------------------------------------------

#[test]
fn row1_a_stop_and_send_logs_the_message_asks_for_the_cancel_and_holds_the_text() {
    for s in RUNNING {
        let (next, cmds) = step(&running(s), &stop("do X instead"));
        assert_eq!(next.state, s, "stopping is not a state");
        assert_eq!(next.job.after_stop.as_deref(), Some("do X instead"));
        assert_eq!(
            next.job.number, 1,
            "the job is the same until its task ends"
        );
        assert_eq!(
            message_data(&cmds),
            &UserMessageData {
                text: "do X instead".into(),
                message_id: Some("m-1".into()),
                run_id: Some("r-1".into()),
                origin: Origin::Agui,
                delivery: Some(Delivery::Interrupt),
                mentions: Vec::new(),
            }
        );
        // the message and the cancel: nothing is sent to the agent (no delegation, no steer)
        assert_eq!(kinds(&cmds), ["user_message"]);
        assert_eq!(orders(&cmds), [&Command::RequestCancel { job: 1 }], "{s:?}");
    }
}

#[test]
fn row1_the_cancel_is_for_the_job_that_runs() {
    let mut snap = running(Working);
    snap.job.number = 4;
    let (_, cmds) = step(&snap, &stop("again"));
    assert_eq!(orders(&cmds), [&Command::RequestCancel { job: 4 }]);
}

#[test]
fn row1_the_catalog_the_screen_sent_is_recorded_first_and_the_next_job_is_told_a_reference() {
    let id = "https://agents.vymalo.com/a2ui/catalogs/chat";
    let json = json!({"catalogId": id, "components": {"Note": {"type": "object"}}});
    let catalog = UiCatalogData {
        catalog_id: id.into(),
        version: 1,
        digest: catalog_digest(&json).unwrap(),
        catalog: json,
    };
    let input = Input::StopAndSend {
        user: user(),
        text: "go".into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: Some(catalog.clone()),
        mentions: Vec::new(),
    };
    let (held, cmds) = step(&running(Working), &input);
    assert_eq!(kinds(&cmds), ["ui_catalog", "user_message"]);
    let (_, cmds) = step(&held, &task(AgentTaskState::Canceled));
    assert!(cmds.contains(&Command::Delegate {
        text: "go".into(),
        catalog: Some(UiDelivery::Ref(catalog.reference())),
        mentions: Vec::new(),
    }));
}

// ---- row 2: while the stop is on its way -----------------------------------------------------

#[test]
fn row2_two_stops_are_joined_by_a_blank_line_and_ask_for_one_cancel() {
    for s in RUNNING {
        let first = stopping(s, "do X");
        let (next, cmds) = step(&first, &stop("and then Y"));
        assert_eq!(next.state, s);
        assert_eq!(next.job.after_stop.as_deref(), Some("do X\n\nand then Y"));
        assert_eq!(message_data(&cmds).delivery, Some(Delivery::Interrupt));
        assert!(
            orders(&cmds).is_empty(),
            "the cancel is on its way: {cmds:?}"
        );
    }
}

#[test]
fn row2_a_plain_message_while_the_stop_is_on_its_way_joins_it_too() {
    for s in RUNNING {
        let (next, cmds) = step(&stopping(s, "do X"), &said("also Y"));
        assert_eq!(next.job.after_stop.as_deref(), Some("do X\n\nalso Y"));
        // not a steer: a task that is stopping would be read by nobody
        assert_eq!(message_data(&cmds).delivery, Some(Delivery::Interrupt));
        assert!(orders(&cmds).is_empty(), "{cmds:?}");
    }
}

#[test]
fn row2_the_joined_text_starts_the_next_job_once() {
    let (snap, cmds) = feed(
        running(Working),
        &[
            stop("one"),
            stop("two"),
            said("three"),
            task(AgentTaskState::Canceled),
        ],
    );
    assert_eq!(snap.state, Queued);
    assert_eq!(snap.job.number, 2);
    assert_eq!(snap.job.after_stop, None);
    let cancels = cmds
        .iter()
        .filter(|c| matches!(c, Command::RequestCancel { .. }))
        .count();
    assert_eq!(cancels, 1, "one cancel for two stops");
    let delegations: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Delegate { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(delegations, ["one\n\ntwo\n\nthree"]);
}

#[test]
fn row2_a_joined_text_over_64_kib_is_refused_and_changes_nothing() {
    let first = "a".repeat(MAX_AFTER_STOP_BYTES - 10);
    let held = stopping(Working, &first);
    let refused = transition(&held, &stop(&"b".repeat(20)));
    assert_eq!(
        refused,
        Err(TransitionError::TextTooLong {
            max: MAX_AFTER_STOP_BYTES
        })
    );
    let refused = transition(&held, &said(&"b".repeat(20)));
    assert!(matches!(refused, Err(TransitionError::TextTooLong { .. })));
    // exactly at the limit is accepted: "\n\n" and one byte more would not be
    let (next, _) = step(&held, &stop("b"));
    assert_eq!(
        next.job.after_stop.as_ref().map(String::len),
        Some(first.len() + 3)
    );
    // the first message is held under the same limit
    let long = "c".repeat(MAX_AFTER_STOP_BYTES + 1);
    assert!(matches!(
        transition(&running(Working), &stop(&long)),
        Err(TransitionError::TextTooLong { .. })
    ));
}

// ---- row 3: Send is a steer ------------------------------------------------------------------

#[test]
fn row3_a_message_to_a_running_job_is_a_steer_and_the_job_goes_on() {
    for s in RUNNING {
        let (next, cmds) = step(&running(s), &said("you were wrong since line 1"));
        assert_eq!(next.state, s);
        assert_eq!(next.job.after_stop, None);
        assert_eq!(message_data(&cmds).delivery, Some(Delivery::Steer));
        assert_eq!(
            orders(&cmds),
            [&Command::Steer {
                text: "you were wrong since line 1".into(),
                catalog: None,
                mentions: Vec::new(),
            }]
        );
    }
}

#[test]
fn row3_a_steer_is_part_of_the_same_job_and_the_same_attempt() {
    let before = gated_with_task(Working);
    let (after, _) = step(&before, &said("and use tabs"));
    assert_eq!(after.job.number, before.job.number);
    assert_eq!(after.job.attempt, before.job.attempt);
    assert_eq!(after.job.verification, before.job.verification);
    assert!(
        after.job.task.as_deref().unwrap().ends_with("and use tabs"),
        "the gate's prompts quote what the person said"
    );
}

#[test]
fn the_first_message_of_a_thread_is_delegated_with_no_delivery() {
    // nothing runs yet that it could steer or stop: a message to a `queued` thread says steer,
    // the first message of a thread says nothing
    for input in [said("hi"), stop("hi")] {
        let (snap, cmds) = start_thread(GatePolicy::default(), &input).unwrap();
        assert_eq!((snap.state, snap.job.number), (Queued, 1));
        assert_eq!(message_data(&cmds).delivery, None);
        assert_eq!(kinds(&cmds), ["user_message"]);
        assert_eq!(
            orders(&cmds),
            [&Command::Delegate {
                text: "hi".into(),
                catalog: None,
                mentions: Vec::new(),
            }]
        );
        assert_eq!(snap.job.after_stop, None);
    }
    // under a gate the message is the job's task, as it is for every message
    let (snap, _) = start_thread(gated(Queued).job.gate, &said("fix it")).unwrap();
    assert_eq!(snap.job.task.as_deref(), Some("fix it"));
}

// ---- row 4: nothing runs, so there is nothing to stop ----------------------------------------

#[test]
fn row4_a_stop_and_send_when_nothing_runs_is_exactly_a_message() {
    for s in NOT_RUNNING {
        let plain = step(&running(s), &said("next please"));
        let stopped = step(&running(s), &stop("next please"));
        assert_eq!(plain, stopped, "{s:?}");
        assert_eq!(message_data(&stopped.1).delivery, None, "{s:?}");
        assert!(
            !stopped
                .1
                .iter()
                .any(|c| matches!(c, Command::RequestCancel { .. })),
            "{s:?}"
        );
        assert_eq!(stopped.0.job.after_stop, None);
    }
}

#[test]
fn row4_a_message_on_a_finished_thread_still_starts_the_next_job() {
    for s in [Done, Failed, Cancelled] {
        let (next, cmds) = step(&running(s), &stop("again"));
        assert_eq!(next.state, Queued);
        assert_eq!(next.job.number, 2);
        assert_eq!(kinds(&cmds), ["user_message", "job_started"]);
    }
}

// ---- row 5: the task ends, and the job is not judged -----------------------------------------

/// What ends a job, as inputs: the task's four terminal states, and a dead-lettered delegation.
fn endings() -> Vec<(&'static str, Input, EventBody)> {
    vec![
        (
            "completed",
            task(AgentTaskState::Completed),
            status_event(AgentStatus::Completed, None),
        ),
        (
            "failed",
            task(AgentTaskState::Failed),
            status_event(AgentStatus::Failed, None),
        ),
        (
            "canceled",
            task(AgentTaskState::Canceled),
            status_event(AgentStatus::Canceled, None),
        ),
        (
            "rejected",
            task(AgentTaskState::Rejected),
            status_event(AgentStatus::Failed, Some("rejected")),
        ),
        (
            "delivery failed",
            Input::DeliveryFailed {
                reason: "gave up".into(),
                retryable: false,
            },
            EventBody::Error(ErrorData {
                message: "gave up".into(),
                retryable: false,
            }),
        ),
        (
            "delivery failed, retryable",
            Input::DeliveryFailed {
                reason: "gave up".into(),
                retryable: true,
            },
            EventBody::Error(ErrorData {
                message: "gave up".into(),
                retryable: true,
            }),
        ),
    ]
}

#[test]
fn row5_every_ending_logs_what_the_agent_said_and_starts_the_next_job_with_no_thread_state() {
    for s in RUNNING {
        for (name, ending, logged) in endings() {
            let (next, cmds) = step(&stopping(s, "do X instead"), &ending);
            assert_eq!(next.state, Queued, "{name} in {s:?}");
            assert_eq!(next.job.number, 2, "{name}");
            assert_eq!(next.job.after_stop, None, "{name}");
            assert_eq!(
                bodies(&cmds),
                [&logged, &EventBody::JobStarted(JobStartedData { job: 2 })],
                "{name} in {s:?}: the agent's word, then the boundary, and no `thread_state`"
            );
            assert_eq!(
                orders(&cmds),
                next_job("do X instead", 1).iter().collect::<Vec<_>>(),
                "{name}"
            );
        }
    }
}

#[test]
fn row5_the_next_job_starts_from_attempt_one_and_keeps_what_the_conversation_has() {
    let mut snap = gated_with_task(Working);
    snap.job.attempt = 2;
    snap.job.tools = vec!["search".into()];
    let (held, _) = step(&snap, &stop("do X"));
    let (next, _) = step(&held, &task(AgentTaskState::Canceled));
    assert_eq!(next.job.number, 2);
    assert_eq!(next.job.attempt, 1);
    assert_eq!(next.job.gate, snap.job.gate);
    assert_eq!(next.job.tools, ["search"]);
    assert_eq!(next.job.pushed, None);
    assert!(next.job.results.is_empty());
    // the gate's prompts for the new job quote the new message, and only it
    assert_eq!(next.job.task.as_deref(), Some("do X"));
}

#[test]
fn row5_an_abandoned_job_is_never_verified_never_reworked_and_spends_no_attempt() {
    for ending in [
        task(AgentTaskState::Completed),
        task(AgentTaskState::Canceled),
        task(AgentTaskState::Failed),
    ] {
        // the agent had pushed, its checks were on their way, and then the person stopped it
        let (held, _) = feed(
            gated_with_task(Working),
            &[branch(S1), stop("do X instead")],
        );
        let verification = held.job.verification;
        let (next, cmds) = step(&held, &ending);
        assert_eq!(next.state, Queued);
        assert_eq!(next.job.attempt, 1);
        assert_eq!(
            next.job.verification, verification,
            "no verification started"
        );
        assert_eq!(
            next.job.pushed, None,
            "its pushed commit is not the next job's"
        );
        for c in &cmds {
            assert!(
                !matches!(
                    c,
                    Command::RequestVerification { .. }
                        | Command::Watch { .. }
                        | Command::Schedule { .. }
                        | Command::RequestDescription { .. }
                ),
                "{c:?}"
            );
        }
        let kinds = kinds(&cmds);
        assert!(
            !kinds.contains(&"rework")
                && !kinds.contains(&"check_result")
                && !kinds.contains(&"thread_state"),
            "{kinds:?}"
        );
    }
}

#[test]
fn row5_the_next_job_runs_under_the_gate_from_attempt_one() {
    let mut gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    gate.max_attempts = 3;
    let mut snap = Snapshot::queued(gate);
    snap.state = Working;
    snap.job.attempt = 2; // the abandoned job was already sent back once
    let failing = artifact(
        "checks",
        json!({"passed": false, "commit": S1, "summary": "cargo test", "findings": ["x"]}),
    );
    let (snap, cmds) = feed(
        snap,
        &[
            stop("do X instead"),
            task(AgentTaskState::Canceled),
            // the new task's work: it pushes, its checks fail, and it finishes
            task(AgentTaskState::Working),
            branch(S1),
            failing,
            task(AgentTaskState::Completed),
        ],
    );
    assert_eq!(snap.job.number, 2);
    // the gate judged the next job, and sent it back as its first attempt fails: attempt 2
    assert_eq!(snap.job.attempt, 2);
    assert!(kinds(&cmds).contains(&"rework"), "{:?}", kinds(&cmds));
}

#[test]
fn row5_what_was_still_coming_for_the_abandoned_job_is_stale() {
    // job 1 was verified once and sent back; the person then stopped the rework
    let (snap, _) = feed(
        gated_with_task(Working),
        &[
            branch(S1),                       // it pushed, so the gate applies
            task(AgentTaskState::Completed),  // verification 1 starts
            said("a word while it verifies"), // abandons it: Verifying -> Queued
            stop("do X instead"),
            task(AgentTaskState::Canceled),
        ],
    );
    assert_eq!(snap.job.number, 2);
    assert_eq!(
        snap.job.verification, 1,
        "the counter is the thread's, never reset"
    );
    // the verifier answers the verification of job 1: stale by the comparison the core makes
    let late = Input::VerifierReported {
        attempt: 1,
        verification: 1,
        verdict: Verdict {
            passed: false,
            findings: vec!["x".into()],
        },
    };
    let (after, cmds) = step(&snap, &late);
    assert_eq!(after, snap, "nothing about the thread changes");
    assert_eq!(
        bodies(&cmds)
            .iter()
            .filter(|b| matches!(b, EventBody::CheckResult(r) if r.stale))
            .count(),
        1
    );
    // and so is its timer
    let timer = Input::TimerFired(Timer::VerifierDeadline {
        attempt: 1,
        verification: 1,
    });
    assert_eq!(step(&snap, &timer), (snap.clone(), vec![]));
}

#[test]
fn row5_a_job_without_a_gate_is_the_same() {
    let (next, cmds) = step(&stopping(Working, "do X"), &task(AgentTaskState::Completed));
    assert_eq!(next.state, Queued);
    assert_eq!(next.job.number, 2);
    assert_eq!(
        kinds(&cmds),
        ["agent_status", "job_started"],
        "the thread never shows done for the abandoned job"
    );
}

#[test]
fn row5_a_working_status_while_stopping_is_logged_as_it_is_without_a_stop() {
    let held = stopping(Queued, "do X");
    let (next, cmds) = step(
        &held,
        &agent_input(AgentUpdate::Status {
            state: AgentTaskState::Working,
            detail: Some("still going".into()),
        }),
    );
    assert_eq!(next.state, Working);
    assert_eq!(next.job.after_stop.as_deref(), Some("do X"));
    assert_eq!(
        bodies(&cmds),
        [&status_event(AgentStatus::Working, Some("still going"))]
    );
}

// ---- row 6: the agent asks while the job is being abandoned ----------------------------------

#[test]
fn row6_a_question_is_logged_and_the_thread_stays_working_with_the_stop_on_its_way() {
    for s in RUNNING {
        for (state, status) in [
            (AgentTaskState::InputRequired, AgentStatus::InputRequired),
            (AgentTaskState::AuthRequired, AgentStatus::AuthRequired),
        ] {
            let held = stopping(s, "do X");
            let (next, cmds) = step(
                &held,
                &agent_input(AgentUpdate::Status {
                    state,
                    detail: Some("which branch?".into()),
                }),
            );
            assert_eq!(next.state, s, "no `blocked`");
            assert_eq!(next.job.after_stop.as_deref(), Some("do X"));
            assert_eq!(
                bodies(&cmds),
                [&status_event(status, Some("which branch?"))]
            );
            assert!(
                !cmds.iter().any(|c| matches!(
                    c,
                    Command::Delegate { .. }
                        | Command::Steer { .. }
                        | Command::RequestCancel { .. }
                )),
                "{cmds:?}"
            );
        }
    }
}

#[test]
fn row6_the_cancel_that_follows_still_starts_the_next_job() {
    let (snap, cmds) = feed(
        stopping(Working, "do X"),
        &[
            agent_input(AgentUpdate::Status {
                state: AgentTaskState::InputRequired,
                detail: None,
            }),
            task(AgentTaskState::Canceled),
        ],
    );
    assert_eq!((snap.state, snap.job.number), (Queued, 2));
    assert!(cmds.contains(&Command::Delegate {
        text: "do X".into(),
        catalog: None,
        mentions: Vec::new(),
    }));
}

// ---- row 7: the agent could not be stopped ---------------------------------------------------

#[test]
fn row7_a_cancel_refused_for_good_sends_the_text_to_the_agent_as_a_steer() {
    for s in RUNNING {
        let (next, cmds) = step(&stopping(s, "do X instead"), &cancel_rejected(false));
        assert_eq!(next.state, s);
        assert_eq!(next.job.number, 1, "the job goes on");
        assert_eq!(next.job.after_stop, None);
        assert_eq!(
            error_message(&cmds),
            "coder could not be stopped; your message was sent to it instead"
        );
        assert_eq!(
            orders(&cmds),
            [&Command::Steer {
                text: "do X instead".into(),
                catalog: None,
                mentions: Vec::new(),
            }]
        );
        assert_eq!(
            kinds(&cmds),
            ["error"],
            "no second user_message: it is in the log"
        );
    }
}

#[test]
fn row7_the_text_becomes_part_of_the_job_the_gate_judges() {
    let (held, _) = step(&gated_with_task(Working), &stop("do X instead"));
    let (next, _) = step(&held, &cancel_rejected(false));
    assert!(next.job.task.as_deref().unwrap().ends_with("do X instead"));
}

#[test]
fn row7_a_cancel_that_may_succeed_later_is_logged_and_the_stop_keeps_waiting() {
    let held = stopping(Working, "do X");
    let (next, cmds) = step(&held, &cancel_rejected(true));
    assert_eq!(next, held);
    assert_eq!(
        bodies(&cmds),
        [&EventBody::Error(ErrorData {
            message: "the agent said no".into(),
            retryable: true
        })]
    );
    assert!(orders(&cmds).is_empty());
}

#[test]
fn row7_without_a_stop_a_refused_cancel_is_logged_as_before() {
    let plain = running(Working);
    let (next, cmds) = step(&plain, &cancel_rejected(false));
    assert_eq!(next, plain);
    assert_eq!(kinds(&cmds), ["error"]);
    assert_eq!(error_message(&cmds), "the agent said no");
}

// ---- row 8: the agent never got the job ------------------------------------------------------

#[test]
fn row8_a_stop_before_the_agent_was_sent_the_job_starts_the_next_one() {
    for s in RUNNING {
        let (next, cmds) = step(&stopping(s, "do X"), &Input::CancelledBeforeStart);
        assert_eq!((next.state, next.job.number), (Queued, 2), "{s:?}");
        assert_eq!(kinds(&cmds), ["job_started"], "not cancelled");
        assert_eq!(
            orders(&cmds),
            next_job("do X", 1).iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn row8_without_a_stop_the_same_input_still_cancels_the_thread() {
    let (next, cmds) = step(&running(Working), &Input::CancelledBeforeStart);
    assert_eq!(next.state, Cancelled);
    assert_eq!(kinds(&cmds), ["thread_state"]);
}

// ---- row 9: Stop ends it ---------------------------------------------------------------------

#[test]
fn row9_a_cancel_clears_the_held_text_and_the_thread_ends_cancelled() {
    for s in RUNNING {
        let (held, cmds) = step(&stopping(s, "do X"), &Input::Cancel { user: user() });
        assert_eq!(held.job.after_stop, None, "the text is never sent");
        assert_eq!(orders(&cmds), [&Command::RequestCancel { job: 1 }]);
        let (end, cmds) = step(&held, &task(AgentTaskState::Canceled));
        assert_eq!(end.state, Cancelled);
        assert_eq!(end.job.number, 1, "no next job");
        assert_eq!(kinds(&cmds), ["agent_status", "thread_state"]);
        assert!(!cmds.iter().any(|c| matches!(c, Command::Delegate { .. })));
    }
}

#[test]
fn row9_a_cancel_before_start_after_a_stop_cancels_the_thread() {
    let (held, _) = step(&stopping(Working, "do X"), &Input::Cancel { user: user() });
    let (end, _) = step(&held, &Input::CancelledBeforeStart);
    assert_eq!(end.state, Cancelled);
}

// ---- what else a stopping job does -----------------------------------------------------------

#[test]
fn a_message_redelivered_for_the_abandoned_job_is_superseded() {
    let held = stopping(Working, "do X");
    let (next, cmds) = step(
        &held,
        &Input::Redeliver {
            text: "old".into(),
            sent: false,
            mentions: Vec::new(),
        },
    );
    assert_eq!(next, held);
    assert!(cmds.is_empty());
}

#[test]
fn an_action_on_a_job_being_abandoned_is_refused() {
    let held = stopping(Working, "do X");
    let action = UiActionData {
        surface_id: "s".into(),
        name: "go".into(),
        source_component_id: "b".into(),
        context: serde_json::Map::new(),
        version: UiVersion::V0_9_1,
        run_id: None,
    };
    let refused = transition(
        &held,
        &Input::UiAction {
            user: user(),
            action,
            catalog: None,
        },
    );
    assert!(matches!(
        refused,
        Err(TransitionError::InvalidInState { .. })
    ));
}

#[test]
fn what_the_agent_says_while_it_stops_is_logged_as_always() {
    let held = stopping(Working, "do X");
    let (next, cmds) = step(
        &held,
        &agent_input(AgentUpdate::Message {
            message_id: "a-1".into(),
            text: "one moment".into(),
            is_final: true,
            purpose: None,
        }),
    );
    assert_eq!(next.state, held.state);
    assert_eq!(next.job.after_stop, held.job.after_stop);
    assert_eq!(bodies(&cmds).len(), 1);
}

#[test]
fn stopping_is_not_a_state_and_a_stopped_thread_always_leaves_it_through_a_job_boundary_or_a_cancel()
 {
    // from every way the stop can be answered, the thread is `queued` with job 2, or still
    // running, or cancelled: never `done`, `failed` or `blocked` for the abandoned job
    for s in RUNNING {
        let held = stopping(s, "do X");
        let inputs = [
            task(AgentTaskState::Completed),
            task(AgentTaskState::Failed),
            task(AgentTaskState::Rejected),
            task(AgentTaskState::Canceled),
            task(AgentTaskState::InputRequired),
            task(AgentTaskState::AuthRequired),
            task(AgentTaskState::Working),
            Input::DeliveryFailed {
                reason: "x".into(),
                retryable: false,
            },
            Input::DeliveryFailed {
                reason: "x".into(),
                retryable: true,
            },
            Input::CancelledBeforeStart,
            cancel_rejected(true),
            cancel_rejected(false),
        ];
        for input in inputs {
            let (next, cmds) = step(&held, &input);
            assert!(
                matches!(next.state, Queued | Working),
                "{input:?} left the thread {:?}",
                next.state
            );
            assert!(
                !kinds(&cmds).contains(&"thread_state"),
                "{input:?} wrote a thread_state"
            );
        }
    }
}

// ---- wire compatibility ----------------------------------------------------------------------

/// A ledger and a log written before this change read and behave as they did: a message in a
/// running job was a delegation then, and is a steer now, which the application writes as the
/// same delegation row.
#[test]
fn an_old_ledger_and_an_old_event_decode_and_transition_as_before() {
    let old_job: Job =
        serde_json::from_value(json!({"number": 3, "attempt": 1, "verification": 2})).unwrap();
    assert_eq!(old_job.after_stop, None);
    let old_event: Event = serde_json::from_value(json!({
        "seq": 7, "threadId": "00000000-0000-7000-8000-000000000001",
        "at": "2026-10-01T10:00:00Z", "kind": "user_message",
        "actor": {"type": "user", "name": "me@example.com"},
        "data": {"text": "hi", "messageId": "m", "runId": "r"}
    }))
    .unwrap();
    let EventBody::UserMessage(data) = &old_event.body else {
        panic!("{old_event:?}");
    };
    assert_eq!(data.delivery, None);
    // the same event written back is the same JSON: no `delivery`, no null
    let back = serde_json::to_value(&old_event).unwrap();
    assert!(back["data"].get("delivery").is_none());

    // the ledger drives the transitions it always did
    let snap = Snapshot {
        state: Working,
        job: old_job,
    };
    let (after, cmds) = step(&snap, &task(AgentTaskState::Completed));
    assert_eq!(after.state, Done);
    assert_eq!(kinds(&cmds), ["agent_status", "thread_state"]);
    let (after, cmds) = step(
        &Snapshot {
            state: Done,
            ..snap
        },
        &said("again"),
    );
    assert_eq!((after.state, after.job.number), (Queued, 4));
    assert_eq!(kinds(&cmds), ["user_message", "job_started"]);
}

#[test]
fn a_stopping_ledger_survives_the_store_and_is_stopping_afterwards() {
    let held = stopping(Working, "do X");
    let stored = serde_json::to_value(&held.job).unwrap();
    assert_eq!(stored["afterStop"], "do X");
    let read: Job = serde_json::from_value(stored).unwrap();
    let (next, _) = step(
        &Snapshot {
            state: Working,
            job: read,
        },
        &task(AgentTaskState::Canceled),
    );
    assert_eq!((next.state, next.job.number), (Queued, 2));
}
