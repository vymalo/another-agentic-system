//! Child sessions a person drives (`child-sessions/v1`, ADR 0072, S3): the pure half. A person
//! starts a session from the parent, sends to a child, steers, stops and closes it; the core says
//! what is written and what is ordered, and refuses what the contract refuses.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Working};
use orch_core::*;
use proptest::prelude::*;
use uuid::Uuid;

fn person() -> UserId {
    UserId::new("ada@example.com")
}

fn child_id(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

fn report(
    session: &str,
    state: SessionState,
    keep_open: bool,
    round: Option<RoundReport>,
) -> SessionReport {
    SessionReport {
        session: session.into(),
        context_id: format!("ctx/{session}"),
        agent: "planner".into(),
        description: "Plan the migration".into(),
        keep_open,
        by: Some(SessionBy::Person),
        state,
        closing: false,
        round,
        parent_context_id: None,
    }
}

fn round(number: u32, task: &str, state: AgentTaskState) -> Option<RoundReport> {
    Some(RoundReport {
        number,
        task_id: Some(task.into()),
        state: Some(state),
    })
}

/// A child thread of session `s1` in `state`, kept open or not, whose round 1 was followed.
fn child(state: ThreadState, keep_open: bool) -> Snapshot {
    let (mut snap, _) = start_child(&report(
        "s1",
        SessionState::Idle,
        keep_open,
        round(1, "t1", AgentTaskState::Working),
    ));
    snap.state = state;
    snap.job.child.as_mut().unwrap().started = 1;
    snap
}

fn user_message(text: &str) -> Input {
    Input::UserMessage {
        user: person(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: vec![],
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
    }
}

fn stop_and_send(text: &str) -> Input {
    Input::StopAndSend {
        user: person(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: vec![],
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
    }
}

fn start(request: &str) -> Input {
    Input::StartSession {
        user: person(),
        request: request.into(),
        agent: "planner".into(),
        description: "Find the writers".into(),
        text: "List every place that writes the ledger".into(),
        keep_open: true,
        tier: Some(AccessTier::Edits),
        tier_cap: None,
    }
}

fn close(cancel: bool) -> Input {
    Input::CloseSession {
        user: person(),
        cancel,
    }
}

fn bodies(cmds: &[Command]) -> Vec<EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(d.body.clone()),
            _ => None,
        })
        .collect()
}

fn kinds(cmds: &[Command]) -> Vec<&'static str> {
    bodies(cmds).iter().map(|b| b.kind().as_str()).collect()
}

fn observes(cmds: &[Command]) -> Vec<u32> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Observe { round, .. } => Some(*round),
            _ => None,
        })
        .collect()
}

// --- sending to a child -------------------------------------------------------------------

#[test]
fn a_message_to_an_idle_child_starts_the_next_job_and_delegates() {
    for ended in [Done, Failed, Cancelled] {
        let (next, cmds) = transition(&child(ended, true), &user_message("and then?")).unwrap();
        assert_eq!(next.state, Queued, "{ended:?}");
        assert_eq!(next.job.number, 2);
        assert!(next.job.child.is_some(), "the session stays the thread's");
        assert!(next.job.observed.is_none());
        assert!(matches!(cmds.last(), Some(Command::Delegate { text, .. }) if text == "and then?"));
        assert!(kinds(&cmds).contains(&"user_message"));
    }
}

#[test]
fn a_message_to_a_working_child_is_steered() {
    let (next, cmds) = transition(&child(Working, true), &user_message("also this")).unwrap();
    assert_eq!(next.state, Working);
    assert!(matches!(cmds.last(), Some(Command::Steer { text, .. }) if text == "also this"));
}

#[test]
fn a_message_to_a_blocked_child_answers_the_round() {
    let (next, cmds) = transition(&child(Blocked, true), &user_message("yes")).unwrap();
    assert_eq!(next.state, Queued);
    assert_eq!(next.job.number, 1, "the answer is the same job's");
    assert!(matches!(cmds.last(), Some(Command::Delegate { text, .. }) if text == "yes"));
}

#[test]
fn a_closed_session_takes_nothing_from_the_person() {
    let mut snap = child(Done, true);
    snap.job.child.as_mut().unwrap().state = SessionState::Closed;
    let action = Input::UiAction {
        user: person(),
        action: UiActionData {
            surface_id: "s".into(),
            name: "go".into(),
            source_component_id: "b".into(),
            context: serde_json::Map::new(),
            version: UiVersion::V1_0,
            run_id: None,
            tier: None,
            tier_cap: None,
        },
        catalog: None,
        tier: None,
        tier_cap: None,
    };
    for input in [user_message("hi"), stop_and_send("hi"), action] {
        for state in [Queued, Working, Blocked, Done, Failed, Cancelled] {
            snap.state = state;
            assert_eq!(
                transition(&snap, &input).unwrap_err(),
                TransitionError::SessionClosed,
                "{state:?} {}",
                input.name()
            );
        }
    }
}

#[test]
fn stop_and_send_is_only_for_a_session_kept_open() {
    for state in [Queued, Working, Blocked, Done] {
        let err = transition(&child(state, false), &stop_and_send("now")).unwrap_err();
        assert_eq!(err, TransitionError::SessionClosesOnStop, "{state:?}");
    }
    // kept open: the ordinary Stop & send, the round is canceled and the next one waits for it
    let (next, cmds) = transition(&child(Working, true), &stop_and_send("now")).unwrap();
    assert_eq!(next.state, Working);
    assert!(cmds.contains(&Command::RequestCancel { job: 1 }));
    assert_eq!(next.job.after_stop.as_deref(), Some("now"));
}

#[test]
fn a_message_to_a_child_that_mentions_an_agent_is_refused_and_writes_nothing() {
    let mention = Mention {
        agent_id: AgentId::new("coder"),
        label: "@coder".into(),
        start: 4,
        end: 10,
        card_url: None,
        tier_cap: None,
    };
    for stop in [false, true] {
        for state in [Done, Working, Blocked] {
            let snap = child(state, true);
            let input = if stop {
                Input::StopAndSend {
                    user: person(),
                    text: "ask @coder".into(),
                    message_id: None,
                    run_id: None,
                    origin: Origin::Agui,
                    catalog: None,
                    mentions: vec![mention.clone()],
                    tier: None,
                    tier_cap: None,
                    locale: None,
                    pick: None,
                }
            } else {
                Input::UserMessage {
                    user: person(),
                    text: "ask @coder".into(),
                    message_id: None,
                    run_id: None,
                    origin: Origin::Agui,
                    catalog: None,
                    mentions: vec![mention.clone()],
                    tier: None,
                    tier_cap: None,
                    locale: None,
                    pick: None,
                }
            };
            assert_eq!(
                transition(&snap, &input).unwrap_err(),
                TransitionError::ChildMentions,
                "{state:?} stop={stop}"
            );
        }
    }
    // a message that mentions nobody is as before, and a thread that is no child takes mentions
    assert!(transition(&child(Done, true), &user_message("next")).is_ok());
    let plain = Input::UserMessage {
        user: person(),
        text: "ask @coder".into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: vec![mention],
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
    };
    assert!(transition(&Snapshot::new(Done), &plain).is_ok());
}

#[test]
fn the_refusals_are_classed() {
    assert_eq!(TransitionError::SessionClosed.class(), ErrorClass::Rejected);
    assert_eq!(TransitionError::NotAChild.class(), ErrorClass::Invalid);
    assert_eq!(
        TransitionError::SessionClosesOnStop.class(),
        ErrorClass::Invalid
    );
    assert_eq!(
        TransitionError::ChildSessionRefused(SessionRefusal::TooManyStarts).class(),
        ErrorClass::Rejected
    );
    assert_eq!(
        TransitionError::ChildSessionRefused(SessionRefusal::ChildThread).class(),
        ErrorClass::Invalid
    );
}

// --- closing -----------------------------------------------------------------------------------

#[test]
fn a_close_is_asked_once_and_again_only_when_it_is_stronger() {
    let snap = child(Working, true);
    let (snap, cmds) = transition(&snap, &close(false)).unwrap();
    assert_eq!(kinds(&cmds), ["child_session_close_asked"]);
    assert_eq!(cmds.last(), Some(&Command::CloseSession { cancel: false }));
    let EventBody::ChildSessionCloseAsked(data) = &bodies(&cmds)[0] else {
        panic!("not a close asked")
    };
    assert_eq!(
        (data.session.as_str(), data.cancel),
        ("s1", false),
        "{data:?}"
    );
    let Command::Append(draft) = &cmds[0] else {
        panic!("the event comes first")
    };
    assert_eq!(draft.actor, Actor::user(&person()));
    let ledger = snap.job.child.as_ref().unwrap();
    assert!(ledger.close_asked && !ledger.close_cancel);

    // the same again, nothing
    let (same, cmds) = transition(&snap, &close(false)).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(same, snap);

    // with cancel, it is stronger: asked again, once
    let (snap, cmds) = transition(&snap, &close(true)).unwrap();
    assert_eq!(cmds.last(), Some(&Command::CloseSession { cancel: true }));
    assert!(snap.job.child.as_ref().unwrap().close_cancel);
    let (snap, cmds) = transition(&snap, &close(true)).unwrap();
    assert!(cmds.is_empty());
    // and a plain close is no stronger than what was asked
    let (_, cmds) = transition(&snap, &close(false)).unwrap();
    assert!(cmds.is_empty());
}

#[test]
fn a_close_of_a_closed_session_writes_nothing() {
    let mut snap = child(Done, true);
    snap.job.child.as_mut().unwrap().state = SessionState::Closed;
    for cancel in [false, true] {
        let (after, cmds) = transition(&snap, &close(cancel)).unwrap();
        assert!(cmds.is_empty());
        assert_eq!(after, snap);
    }
}

#[test]
fn a_close_is_valid_in_every_state_and_moves_nothing() {
    for state in [Queued, Working, Blocked, Done, Failed, Cancelled] {
        let (after, cmds) = transition(&child(state, true), &close(false)).unwrap();
        assert_eq!(after.state, state);
        assert_eq!(cmds.len(), 2, "{state:?}");
    }
}

#[test]
fn a_thread_that_is_no_child_cannot_be_closed() {
    let err = transition(&Snapshot::new(Working), &close(false)).unwrap_err();
    assert_eq!(err, TransitionError::NotAChild);
}

#[test]
fn a_refused_close_clears_the_ask_and_says_why_in_the_childs_log() {
    let (asked, _) = transition(&child(Working, true), &close(true)).unwrap();
    let failed = Input::SessionCloseFailed {
        reason: "session s1 is closed and takes no more messages".into(),
    };
    let (after, cmds) = transition(&asked, &failed).unwrap();
    assert_eq!(after.state, Working);
    let ledger = after.job.child.as_ref().unwrap();
    assert!(!ledger.close_asked && !ledger.close_cancel);
    // the error, then the state the session is in with no close asked: what a card reads
    assert_eq!(
        bodies(&cmds),
        [
            EventBody::Error(ErrorData {
                message: "session s1 is closed and takes no more messages".into(),
                retryable: false,
            }),
            EventBody::ChildSessionState(ChildSessionStateData {
                session: ledger.session.clone(),
                state: ledger.state,
                closing: ledger.closing,
                close_asked: false,
            }),
        ]
    );
    // the person may ask again
    let (_, cmds) = transition(&after, &close(false)).unwrap();
    assert_eq!(kinds(&cmds), ["child_session_close_asked"]);
}

#[test]
fn a_close_that_was_not_asked_cannot_fail_and_a_non_child_refuses() {
    let snap = child(Working, true);
    let failed = Input::SessionCloseFailed { reason: "x".into() };
    let (after, cmds) = transition(&snap, &failed).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, snap);
    assert_eq!(
        transition(&Snapshot::new(Working), &failed).unwrap_err(),
        TransitionError::NotAChild
    );
}

#[test]
fn the_words_of_a_refused_close_are_cut() {
    let (asked, _) = transition(&child(Working, true), &close(false)).unwrap();
    let failed = Input::SessionCloseFailed {
        reason: "é".repeat(600),
    };
    let (_, cmds) = transition(&asked, &failed).unwrap();
    let EventBody::Error(e) = &bodies(&cmds)[0] else {
        panic!("not an error")
    };
    assert!(e.message.len() <= 512, "{}", e.message.len());
}

// --- starting from the parent ------------------------------------------------------------

#[test]
fn a_start_is_asked_in_the_parents_log_without_the_text_and_ordered() {
    for state in [Queued, Working, Blocked, Done, Failed, Cancelled] {
        let parent = Snapshot::new(state);
        let (next, cmds) = transition(&parent, &start("r1")).unwrap();
        assert_eq!(next.state, state, "a start never moves the thread");
        assert_eq!(kinds(&cmds), ["child_session_start_asked"]);
        let EventBody::ChildSessionStartAsked(data) = &bodies(&cmds)[0] else {
            panic!("not a start asked")
        };
        assert_eq!(
            serde_json::to_value(data).unwrap(),
            serde_json::json!({
                "request": "r1", "agent": "planner", "description": "Find the writers",
                "keepOpen": true, "tier": "edits"
            }),
            "no text in the log"
        );
        let Command::Append(draft) = &cmds[0] else {
            panic!()
        };
        assert_eq!(draft.actor, Actor::user(&person()));
        assert_eq!(
            cmds[1],
            Command::StartSession {
                request: "r1".into(),
                agent: "planner".into(),
                description: "Find the writers".into(),
                text: "List every place that writes the ledger".into(),
                keep_open: true,
                tier: Some(AccessTier::Edits),
                tier_cap: None,
            }
        );
        assert_eq!(
            next.job.session_starts,
            vec![PendingStart {
                request: "r1".into()
            }]
        );
    }
}

#[test]
fn the_same_start_twice_is_one() {
    let (once, _) = transition(&Snapshot::new(Done), &start("r1")).unwrap();
    let (twice, cmds) = transition(&once, &start("r1")).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(twice, once);
}

#[test]
fn a_parent_has_at_most_four_starts_pending() {
    let mut snap = Snapshot::new(Done);
    for n in 0..MAX_PENDING_STARTS {
        snap = transition(&snap, &start(&format!("r{n}"))).unwrap().0;
    }
    assert_eq!(snap.job.session_starts.len(), MAX_PENDING_STARTS);
    let err = transition(&snap, &start("one-more")).unwrap_err();
    assert_eq!(
        err,
        TransitionError::ChildSessionRefused(SessionRefusal::TooManyStarts)
    );
    // one that ended makes room
    let ended = Input::SessionStartEnded {
        request: "r0".into(),
        outcome: SessionStartOutcome::Failed {
            reason: "no".into(),
        },
    };
    let snap = transition(&snap, &ended).unwrap().0;
    assert!(transition(&snap, &start("one-more")).is_ok());
    // a replay of a pending one is not the fifth
    let full = transition(&snap, &start("one-more")).unwrap().0;
    assert!(transition(&full, &start("r1")).unwrap().1.is_empty());
}

#[test]
fn a_child_cannot_start_a_session() {
    let err = transition(&child(Done, true), &start("r1")).unwrap_err();
    assert_eq!(
        err,
        TransitionError::ChildSessionRefused(SessionRefusal::ChildThread)
    );
}

#[test]
fn a_start_with_a_bad_description_or_request_is_refused_and_writes_nothing() {
    let bad = |description: &str, request: &str| Input::StartSession {
        user: person(),
        request: request.into(),
        agent: "planner".into(),
        description: description.into(),
        text: "go".into(),
        keep_open: false,
        tier: None,
        tier_cap: None,
    };
    let long = "d".repeat(81);
    for input in [
        bad("", "r1"),
        bad("   ", "r1"),
        bad("two\nlines", "r1"),
        bad("bell\u{7}", "r1"),
        bad(&long, "r1"),
        bad("fine", ""),
        bad("fine", &"r".repeat(129)),
    ] {
        let err = transition(&Snapshot::new(Done), &input).unwrap_err();
        assert_eq!(
            err,
            TransitionError::ChildSessionRefused(SessionRefusal::Malformed)
        );
    }
    let exactly = "d".repeat(80);
    assert!(transition(&Snapshot::new(Done), &bad(&exactly, "r1")).is_ok());
}

#[test]
fn a_start_that_ended_drops_its_pending_start_once() {
    let (snap, _) = transition(&Snapshot::new(Working), &start("r1")).unwrap();
    let opened = Input::SessionStartEnded {
        request: "r1".into(),
        outcome: SessionStartOutcome::Opened {
            session: "s9".into(),
            child: child_id(9),
        },
    };
    let (snap, cmds) = transition(&snap, &opened).unwrap();
    assert!(snap.job.session_starts.is_empty());
    assert_eq!(snap.state, Working);
    assert_eq!(kinds(&cmds), ["child_session_start_ended"]);
    let Command::Append(draft) = &cmds[0] else {
        panic!()
    };
    assert_eq!(draft.actor, Actor::system());
    assert_eq!(
        draft.body.data_value(),
        serde_json::json!({
            "request": "r1", "outcome": "opened", "session": "s9",
            "thread": child_id(9).to_string()
        })
    );
    // again, or for a request nobody asked: nothing
    let (same, cmds) = transition(&snap, &opened).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(same, snap);
    let stranger = Input::SessionStartEnded {
        request: "nope".into(),
        outcome: SessionStartOutcome::Failed { reason: "x".into() },
    };
    assert!(transition(&snap, &stranger).unwrap().1.is_empty());
}

#[test]
fn a_start_that_failed_says_the_agents_words_cut_to_512_bytes() {
    let (snap, _) = transition(&Snapshot::new(Done), &start("r1")).unwrap();
    let words = "é".repeat(400); // 800 bytes
    let failed = Input::SessionStartEnded {
        request: "r1".into(),
        outcome: SessionStartOutcome::Failed {
            reason: words.clone(),
        },
    };
    let (_, cmds) = transition(&snap, &failed).unwrap();
    let EventBody::ChildSessionStartEnded(data) = &bodies(&cmds)[0] else {
        panic!()
    };
    assert_eq!(data.outcome, StartOutcomeKind::Failed);
    let reason = data.reason.as_deref().unwrap();
    assert!(reason.len() <= 512 && words.starts_with(reason) && !reason.is_empty());
    assert_eq!((data.session.as_deref(), data.thread), (None, None));
    // a short reason is kept as it is
    let (snap, _) = transition(&Snapshot::new(Done), &start("r2")).unwrap();
    let (_, cmds) = transition(
        &snap,
        &Input::SessionStartEnded {
            request: "r2".into(),
            outcome: SessionStartOutcome::Failed {
                reason: "no helper `x`".into(),
            },
        },
    )
    .unwrap();
    let EventBody::ChildSessionStartEnded(data) = &bodies(&cmds)[0] else {
        panic!()
    };
    assert_eq!(data.reason.as_deref(), Some("no helper `x`"));
}

#[test]
fn the_pending_starts_belong_to_the_conversation() {
    let (snap, _) = transition(&Snapshot::new(Working), &start("r1")).unwrap();
    assert_eq!(snap.job.next().session_starts, snap.job.session_starts);
    // a job stored before the field reads, and a job with none writes none
    let old: Job = serde_json::from_value(serde_json::json!({"number": 1})).unwrap();
    assert!(old.session_starts.is_empty());
    assert!(
        serde_json::to_value(&old)
            .unwrap()
            .get("sessionStarts")
            .is_none()
    );
    let v = serde_json::to_value(&snap.job).unwrap();
    assert_eq!(v["sessionStarts"], serde_json::json!([{"request": "r1"}]));
    let back: Job = serde_json::from_value(v).unwrap();
    assert_eq!(back, snap.job);
}

// --- the round a thread sent ---------------------------------------------------------------------

fn round_sent(number: u32) -> Input {
    Input::RoundSent {
        session: "s1".into(),
        number,
        task_id: format!("t{number}"),
    }
}

fn said(r: SessionReport) -> Input {
    Input::ChildSessionSaid(r)
}

#[test]
fn the_round_a_thread_sent_is_followed_and_not_observed_again() {
    // the person's message started job 2 on an idle child; adam answers with round 2
    let (snap, _) = transition(&child(Done, true), &user_message("next")).unwrap();
    assert_eq!(snap.state, Queued);
    let (snap, cmds) = transition(&snap, &round_sent(2)).unwrap();
    assert!(cmds.is_empty());
    let ledger = snap.job.child.as_ref().unwrap();
    assert_eq!((ledger.followed, ledger.started), (2, 2));
    assert!(snap.job.observed.is_none(), "this job sent it, no Observe");

    // the agent's report of the same round orders nothing
    let (snap, cmds) = transition(
        &snap,
        &said(report(
            "s1",
            SessionState::Working,
            true,
            round(2, "t2", AgentTaskState::Working),
        )),
    )
    .unwrap();
    assert!(observes(&cmds).is_empty());
    // and a late Observe of it is a replay
    let (after, cmds) = transition(
        &snap,
        &Input::RoundStarted {
            session: "s1".into(),
            number: 2,
            task_id: "t2".into(),
            by: Some(SessionBy::Person),
        },
    )
    .unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, snap);
}

#[test]
fn a_round_sent_never_lowers_what_was_followed() {
    let (snap, _) = transition(&child(Working, true), &round_sent(1)).unwrap();
    let ledger = snap.job.child.as_ref().unwrap();
    assert_eq!((ledger.followed, ledger.started), (1, 1));
    let mut high = child(Working, true);
    {
        let l = high.job.child.as_mut().unwrap();
        l.followed = 5;
        l.started = 5;
    }
    let (snap, _) = transition(&high, &round_sent(3)).unwrap();
    let ledger = snap.job.child.as_ref().unwrap();
    assert_eq!((ledger.followed, ledger.started), (5, 5));
}

#[test]
fn a_round_sent_is_refused_by_a_thread_that_is_no_child_or_another_session() {
    assert_eq!(
        transition(&Snapshot::new(Working), &round_sent(2)).unwrap_err(),
        TransitionError::NotAChild
    );
    let other = Input::RoundSent {
        session: "other".into(),
        number: 2,
        task_id: "t".into(),
    };
    let snap = child(Working, true);
    let (after, cmds) = transition(&snap, &other).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, snap);
}

#[test]
fn the_closing_round_is_never_followed() {
    // an idle kept-open session, a close asked: adam answers with the closing round 2
    let (snap, _) = transition(&child(Done, true), &close(false)).unwrap();
    let (snap, cmds) = transition(&snap, &round_sent(2)).unwrap();
    assert!(cmds.is_empty());
    let ledger = snap.job.child.as_ref().unwrap();
    assert_eq!(
        (ledger.followed, ledger.started),
        (2, 1),
        "followed, not started: no job is made for it"
    );
    assert_eq!(snap.state, Done);

    // the report of the closed session with that round terminal shows nothing
    let (snap, cmds) = transition(
        &snap,
        &said(report(
            "s1",
            SessionState::Closed,
            true,
            round(2, "t2", AgentTaskState::Completed),
        )),
    )
    .unwrap();
    assert!(observes(&cmds).is_empty());
    assert_eq!(snap.job.child.unwrap().state, SessionState::Closed);
}

#[test]
fn a_report_of_the_closing_round_orders_nothing_even_before_the_answer() {
    let (snap, _) = transition(&child(Done, true), &close(false)).unwrap();
    let closing = report(
        "s1",
        SessionState::Closed,
        true,
        round(2, "t2", AgentTaskState::Completed),
    );
    let (snap, cmds) = transition(&snap, &said(closing.clone())).unwrap();
    assert!(observes(&cmds).is_empty());
    assert_eq!(snap.job.child.as_ref().unwrap().followed, 2);
    // without a close asked, the same report is a round to follow
    let (_, cmds) = transition(&child(Done, true), &said(closing)).unwrap();
    assert_eq!(observes(&cmds), [2]);
}

#[test]
fn the_closing_round_reported_while_it_works_orders_nothing_before_the_answer() {
    // a close of an idle session makes round 2, which the parent's stream reports working, with the
    // close under way, before the answer to the close reaches the thread
    let (snap, _) = transition(&child(Done, true), &close(false)).unwrap();
    let working = SessionReport {
        closing: true,
        ..report(
            "s1",
            SessionState::Working,
            true,
            round(2, "t2", AgentTaskState::Working),
        )
    };
    let (snap, cmds) = transition(&snap, &said(working)).unwrap();
    assert!(
        observes(&cmds).is_empty(),
        "the closing round is never a job: {cmds:?}"
    );
    assert_eq!(snap.state, Done);
}

#[test]
fn a_round_the_person_sent_is_not_observed_before_the_answer_names_it() {
    let (snap, _) = transition(&child(Done, true), &user_message("next")).unwrap();
    assert_eq!(snap.state, Queued);
    // the parent's stream reports round 2 before the answer to the message is applied
    let (snap, cmds) = transition(
        &snap,
        &said(report(
            "s1",
            SessionState::Working,
            true,
            round(2, "t2", AgentTaskState::Working),
        )),
    )
    .unwrap();
    assert!(observes(&cmds).is_empty(), "{cmds:?}");
    let (snap, _) = transition(&snap, &round_sent(2)).unwrap();
    assert_eq!(snap.job.child.as_ref().unwrap().started, 2);
    assert!(snap.job.observed.is_none());
}

#[test]
fn only_the_round_the_close_made_is_the_closing_round() {
    // the close was asked with round 1 the newest known: it makes round 2
    let (asked, _) = transition(&child(Done, true), &close(false)).unwrap();
    let ended = |n: u32| {
        report(
            "s1",
            SessionState::Closed,
            true,
            round(n, &format!("t{n}"), AgentTaskState::Completed),
        )
    };
    // a later round that ended is a real round that was missed: it is followed
    let (_, cmds) = transition(&asked, &said(ended(3))).unwrap();
    assert_eq!(observes(&cmds), [3]);
    // once the answer names the closing round, it alone is it
    let (confirmed, _) = transition(&asked, &round_sent(3)).unwrap();
    let (after, cmds) = transition(&confirmed, &said(ended(3))).unwrap();
    assert!(observes(&cmds).is_empty());
    let (_, cmds) = transition(&after, &said(ended(2))).unwrap();
    assert!(
        observes(&cmds).is_empty(),
        "round 2 is at or below what was followed"
    );
}

fn started(number: u32) -> Input {
    Input::RoundStarted {
        session: "s1".into(),
        number,
        task_id: format!("t{number}"),
        by: Some(SessionBy::Person),
    }
}

#[test]
fn a_session_that_closes_when_done_takes_a_plain_message_and_only_refuses_the_stop() {
    let (next, cmds) = transition(&child(Working, false), &user_message("also this")).unwrap();
    assert_eq!(next.state, Working);
    assert!(matches!(cmds.last(), Some(Command::Steer { text, .. }) if text == "also this"));
    let (next, cmds) = transition(&child(Blocked, false), &user_message("yes")).unwrap();
    assert_eq!(next.state, Queued);
    assert!(matches!(cmds.last(), Some(Command::Delegate { text, .. }) if text == "yes"));
}

#[test]
fn a_start_says_whether_the_session_is_kept_open_to_the_log_and_to_the_order() {
    for keep_open in [false, true] {
        let input = Input::StartSession {
            user: person(),
            request: "r".into(),
            agent: "planner".into(),
            description: "Find the writers".into(),
            text: "go".into(),
            keep_open,
            tier: None,
            tier_cap: None,
        };
        let (_, cmds) = transition(&Snapshot::new(Done), &input).unwrap();
        match &bodies(&cmds)[0] {
            EventBody::ChildSessionStartAsked(d) => assert_eq!(d.keep_open, keep_open),
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(cmds.last(), Some(Command::StartSession { keep_open: k, .. }) if *k == keep_open)
        );
    }
}

#[test]
fn the_state_a_close_changes_says_the_close_is_asked() {
    let (asked, _) = transition(&child(Working, true), &close(false)).unwrap();
    let working_and_closing = SessionReport {
        closing: true,
        ..report("s1", SessionState::Working, true, None)
    };
    let (_, cmds) = transition(&asked, &said(working_and_closing)).unwrap();
    assert_eq!(
        bodies(&cmds)[0].data_value(),
        serde_json::json!({"session": "s1", "state": "working", "closing": true, "closeAsked": true})
    );
}

#[test]
fn the_closing_round_of_a_blocked_thread_is_not_followed_again_when_it_works() {
    // the thread waits for the person; the close makes round 2, which is reported working
    let (asked, _) = transition(&child(Blocked, true), &close(false)).unwrap();
    let working = SessionReport {
        closing: true,
        ..report(
            "s1",
            SessionState::Working,
            true,
            round(2, "t2", AgentTaskState::Working),
        )
    };
    let (_, cmds) = transition(&asked, &said(working)).unwrap();
    assert!(observes(&cmds).is_empty(), "{cmds:?}");
}

#[test]
fn a_round_sent_while_no_close_is_asked_does_not_name_the_closing_round_later() {
    // the answer to a message arrives after its job ended: the round is followed, nothing else
    let (snap, _) = transition(&child(Done, true), &round_sent(5)).unwrap();
    let (snap, _) = transition(&snap, &close(false)).unwrap();
    // the close makes round 6, the one after the newest known
    let (_, cmds) = transition(
        &snap,
        &said(report(
            "s1",
            SessionState::Closed,
            true,
            round(6, "t6", AgentTaskState::Completed),
        )),
    )
    .unwrap();
    assert!(observes(&cmds).is_empty(), "{cmds:?}");
}

#[test]
fn the_round_the_answer_to_a_close_named_never_starts_a_job() {
    let (asked, _) = transition(&child(Done, true), &close(false)).unwrap();
    let (confirmed, _) = transition(&asked, &round_sent(2)).unwrap();
    let (after, cmds) = transition(&confirmed, &started(2)).unwrap();
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(after, confirmed);
}

#[test]
fn only_the_round_a_close_may_have_made_waits_for_its_answer_to_start() {
    let (asked, _) = transition(&child(Done, true), &close(false)).unwrap();
    // round 2 may be the closing round: refused, the dispatcher asks again
    assert!(transition(&asked, &started(2)).is_err());
    // round 3 is a real round that was missed: it starts a job
    let (next, _) = transition(&asked, &started(3)).unwrap();
    assert_eq!(next.state, Working);
}

#[test]
fn a_stale_report_does_not_move_the_state_back() {
    let say = |snap: &Snapshot, state, round_no: u32, closing: bool| {
        let r = SessionReport {
            closing,
            ..report(
                "s1",
                state,
                true,
                round(round_no, &format!("t{round_no}"), AgentTaskState::Working),
            )
        };
        transition(snap, &said(r)).unwrap()
    };
    let snap = child(Working, true);
    let (snap, cmds) = say(&snap, SessionState::Working, 2, false);
    assert_eq!(kinds(&cmds), ["child_session_state"]);
    let (snap, cmds) = say(&snap, SessionState::Idle, 2, false);
    assert_eq!(kinds(&cmds), ["child_session_state"]);
    // a listing read before the round ended says working again: older news
    let (snap, cmds) = say(&snap, SessionState::Working, 2, false);
    assert!(cmds.is_empty(), "flapped back: {cmds:?}");
    assert_eq!(snap.job.child.as_ref().unwrap().state, SessionState::Idle);
    // and so does a listing of an earlier round
    let (snap, cmds) = say(&snap, SessionState::Working, 1, false);
    assert!(cmds.is_empty(), "{cmds:?}");
    // the next round is news
    let (snap, cmds) = say(&snap, SessionState::Working, 3, false);
    assert_eq!(kinds(&cmds), ["child_session_state"]);
    // a close under way is not undone by an older report of the same round
    let (snap, _) = say(&snap, SessionState::Working, 3, true);
    let (snap, cmds) = say(&snap, SessionState::Working, 3, false);
    assert!(cmds.is_empty(), "{cmds:?}");
    assert!(snap.job.child.as_ref().unwrap().closing);
}

// --- what the child's log says of its session ----------------------------------------------------

#[test]
fn the_state_of_the_session_is_written_when_it_changes() {
    let snap = child(Working, true);
    let with = |snap: &Snapshot, state, closing| {
        let r = SessionReport {
            closing,
            ..report("s1", state, true, None)
        };
        transition(snap, &said(r)).unwrap()
    };
    // `child` made the session idle
    let (snap, cmds) = with(&snap, SessionState::Idle, false);
    assert!(cmds.is_empty(), "no change, nothing written");
    let (snap, cmds) = with(&snap, SessionState::Working, false);
    assert_eq!(kinds(&cmds), ["child_session_state"]);
    let Command::Append(draft) = &cmds[0] else {
        panic!()
    };
    assert_eq!(draft.actor, Actor::system());
    assert_eq!(
        draft.body.data_value(),
        serde_json::json!({"session": "s1", "state": "working", "closing": false})
    );
    let (snap, cmds) = with(&snap, SessionState::Working, true);
    assert_eq!(
        bodies(&cmds)[0].data_value(),
        serde_json::json!({"session": "s1", "state": "working", "closing": true})
    );
    let (snap, cmds) = with(&snap, SessionState::Working, true);
    assert!(cmds.is_empty());
    let (snap, cmds) = with(&snap, SessionState::Closed, false);
    assert_eq!(
        bodies(&cmds)[0].data_value(),
        serde_json::json!({"session": "s1", "state": "closed", "closing": false})
    );
    // closed is final: a late report changes nothing and writes nothing
    let (_, cmds) = with(&snap, SessionState::Working, false);
    assert!(cmds.is_empty());
}

#[test]
fn the_state_of_another_session_writes_nothing() {
    let snap = child(Working, true);
    let (_, cmds) = transition(
        &snap,
        &said(report("other", SessionState::Closed, true, None)),
    )
    .unwrap();
    assert!(cmds.is_empty());
}

// --- a child made for a person's start -----------------------------------------------------------

#[test]
fn a_child_started_by_a_person_holds_their_message_before_round_one() {
    let r = report(
        "s9",
        SessionState::Working,
        true,
        round(1, "t1", AgentTaskState::Working),
    );
    let prompt = PersonPrompt {
        user: person(),
        text: "List every place that writes the ledger".into(),
        message_id: Some("start-r1".into()),
        tier: Some(AccessTier::Edits),
    };
    let (snap, cmds) = start_child_for(&r, Some(prompt));
    assert_eq!(snap.state, Queued);
    assert_eq!(kinds(&cmds), ["user_message"]);
    let EventBody::UserMessage(data) = &bodies(&cmds)[0] else {
        panic!()
    };
    assert_eq!(data.text, "List every place that writes the ledger");
    assert_eq!(data.message_id.as_deref(), Some("start-r1"));
    assert_eq!(data.tier, Some(AccessTier::Edits));
    let Command::Append(draft) = &cmds[0] else {
        panic!()
    };
    assert_eq!(draft.actor, Actor::user(&person()));
    // the message comes first, the Observe of round 1 after it, by the person
    assert!(matches!(
        &cmds[1],
        Command::Observe {
            round: 1,
            by: Some(SessionBy::Person),
            ..
        }
    ));
    assert_eq!(cmds.len(), 2);

    // no prompt: exactly what a child of the agent's own start is
    let (plain, plain_cmds) = start_child_for(&r, None);
    assert_eq!((plain.clone(), plain_cmds), start_child(&r));
    assert_eq!(plain.state, Queued);
}

#[test]
fn a_prompt_with_no_round_to_follow_still_holds_the_message() {
    let r = report("s9", SessionState::Idle, true, None);
    let prompt = PersonPrompt {
        user: person(),
        text: "hello".into(),
        message_id: None,
        tier: None,
    };
    let (snap, cmds) = start_child_for(&r, Some(prompt));
    assert_eq!(kinds(&cmds), ["user_message"]);
    assert_eq!(
        snap.state, Done,
        "at rest, as a child with nothing to follow"
    );
}

// --- the wire ----------------------------------------------------------------------------------------

#[test]
fn the_four_events_have_their_kinds_and_data_and_read_back() {
    let thread = child_id(7);
    let table = [
        (
            EventBody::ChildSessionStartAsked(ChildSessionStartAskedData {
                request: "r1".into(),
                agent: "planner".into(),
                description: "Plan".into(),
                keep_open: false,
                tier: None,
            }),
            "child_session_start_asked",
            serde_json::json!({"request": "r1", "agent": "planner", "description": "Plan", "keepOpen": false}),
        ),
        (
            EventBody::ChildSessionStartEnded(ChildSessionStartEndedData {
                request: "r1".into(),
                outcome: StartOutcomeKind::Opened,
                session: Some("s1".into()),
                thread: Some(thread),
                reason: None,
            }),
            "child_session_start_ended",
            serde_json::json!({"request": "r1", "outcome": "opened", "session": "s1", "thread": thread.to_string()}),
        ),
        (
            EventBody::ChildSessionStartEnded(ChildSessionStartEndedData {
                request: "r2".into(),
                outcome: StartOutcomeKind::Failed,
                session: None,
                thread: None,
                reason: Some("too many".into()),
            }),
            "child_session_start_ended",
            serde_json::json!({"request": "r2", "outcome": "failed", "reason": "too many"}),
        ),
        (
            EventBody::ChildSessionCloseAsked(ChildSessionCloseAskedData {
                session: "s1".into(),
                cancel: true,
            }),
            "child_session_close_asked",
            serde_json::json!({"session": "s1", "cancel": true}),
        ),
        (
            EventBody::ChildSessionState(ChildSessionStateData {
                session: "s1".into(),
                state: SessionState::Idle,
                closing: false,
                close_asked: false,
            }),
            "child_session_state",
            serde_json::json!({"session": "s1", "state": "idle", "closing": false}),
        ),
    ];
    for (body, kind, data) in table {
        assert_eq!(body.kind().as_str(), kind);
        assert_eq!(body.data_value(), data, "{kind}");
        let back = EventBody::from_parts(body.kind(), data).unwrap();
        assert_eq!(back, body, "{kind}");
    }
}

#[test]
fn the_start_asked_has_no_place_for_the_text() {
    let v = serde_json::to_value(ChildSessionStartAskedData {
        request: "r".into(),
        agent: "a".into(),
        description: "d".into(),
        keep_open: true,
        tier: Some(AccessTier::Plan),
    })
    .unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert!(!keys.contains(&"text"), "{keys:?}");
}

#[test]
fn the_ledger_of_a_close_is_written_and_read_back_and_old_ones_read() {
    let (snap, _) = transition(&child(Working, true), &close(true)).unwrap();
    let ledger = snap.job.child.clone().unwrap();
    let v = serde_json::to_value(&ledger).unwrap();
    assert_eq!(v["closeAsked"], true);
    assert_eq!(v["closeCancel"], true);
    assert_eq!(serde_json::from_value::<ChildLedger>(v).unwrap(), ledger);
    let bare: ChildLedger = serde_json::from_value(serde_json::json!({
        "session": "s1", "agent": "a", "keepOpen": true, "state": "working"
    }))
    .unwrap();
    assert!(!bare.close_asked && !bare.close_cancel);
    let v = serde_json::to_value(&bare).unwrap();
    assert!(v.get("closeAsked").is_none() && v.get("closeCancel").is_none());
}

#[test]
fn a_report_keeps_the_context_of_the_parent_only_when_it_has_one() {
    let mut r = report("s1", SessionState::Idle, true, None);
    assert!(
        serde_json::to_value(&r)
            .unwrap()
            .get("parentContextId")
            .is_none()
    );
    r.parent_context_id = Some("ctx-parent".into());
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["parentContextId"], "ctx-parent");
    assert_eq!(serde_json::from_value::<SessionReport>(v).unwrap(), r);
    let old: SessionReport = serde_json::from_value(serde_json::json!({
        "session": "s", "contextId": "c", "agent": "a", "description": "", "keepOpen": false,
        "state": "idle"
    }))
    .unwrap();
    assert_eq!(old.parent_context_id, None);
}

#[test]
fn the_child_says_what_the_composer_reads() {
    let (snap, _) = transition(&child(Working, true), &close(false)).unwrap();
    let record = ThreadRecord {
        id: child_id(2),
        owner: person(),
        title: "Plan".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: snap.state,
        job: snap.job,
        version: 1,
        forked_from: None,
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: Some(child_id(1)),
        child_of: Some(ChildOf {
            thread: child_id(1),
            session: "s1".into(),
        }),
        rail_rank: "i".to_owned(),
        last_seq: 0,
        created_at: "2026-11-19T10:00:00Z".parse().unwrap(),
        updated_at: "2026-11-19T10:00:00Z".parse().unwrap(),
    };
    let wire = serde_json::to_value(&record).unwrap();
    assert_eq!(
        wire["childOf"],
        serde_json::json!({
            "threadId": child_id(1).to_string(),
            "session": "s1",
            "agent": "planner",
            "state": "idle",
            "keepOpen": true,
            "closeAsked": true
        })
    );
}

// --- properties ------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Sent(u32),
    Said(u32, bool),
    Started(u32),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u32..5).prop_map(Op::Sent),
        (1u32..5, any::<bool>()).prop_map(|(n, done)| Op::Said(n, done)),
        (1u32..5).prop_map(Op::Started),
    ]
}

fn apply_op(snap: &Snapshot, op: &Op) -> (Snapshot, Vec<Command>) {
    let input = match op {
        Op::Sent(n) => round_sent(*n),
        Op::Said(n, done) => said(report(
            "s1",
            SessionState::Working,
            true,
            round(
                *n,
                &format!("t{n}"),
                if *done {
                    AgentTaskState::Completed
                } else {
                    AgentTaskState::Working
                },
            ),
        )),
        Op::Started(n) => Input::RoundStarted {
            session: "s1".into(),
            number: *n,
            task_id: format!("t{n}"),
            by: Some(SessionBy::Person),
        },
    };
    // a refusal (a busy thread) changes nothing: the dispatcher tries again
    transition(snap, &input).unwrap_or_else(|_| (snap.clone(), Vec::new()))
}

proptest! {
    /// However `RoundSent`, the agent's reports and `RoundStarted` interleave, a round is ordered
    /// followed at most once.
    #[test]
    fn every_round_is_followed_at_most_once(ops in proptest::collection::vec(arb_op(), 0..40)) {
        let (mut snap, _) = start_child_for(&report("s1", SessionState::Idle, true, None), None);
        let mut ordered = std::collections::HashMap::<u32, u32>::new();
        for op in &ops {
            let (next, cmds) = apply_op(&snap, op);
            for n in observes(&cmds) {
                *ordered.entry(n).or_default() += 1;
            }
            snap = next;
        }
        for (n, times) in &ordered {
            prop_assert!(*times <= 1, "round {n} ordered {times} times");
        }
        let followed = snap.job.child.as_ref().unwrap().followed;
        for n in ordered.keys() {
            prop_assert!(*n <= followed);
        }
    }

    /// The pending starts are a set of at most four, whatever is asked and ended.
    #[test]
    fn the_pending_starts_are_a_bounded_set(
        ops in proptest::collection::vec((0u8..8, any::<bool>()), 0..60)
    ) {
        let mut snap = Snapshot::new(Done);
        for (n, end) in ops {
            let request = format!("r{n}");
            let input = if end {
                Input::SessionStartEnded {
                    request,
                    outcome: SessionStartOutcome::Failed { reason: "x".into() },
                }
            } else {
                start(&request)
            };
            if let Ok((next, _)) = transition(&snap, &input) {
                snap = next;
            }
            let pending: Vec<&str> =
                snap.job.session_starts.iter().map(|p| p.request.as_str()).collect();
            let unique: std::collections::HashSet<&&str> = pending.iter().collect();
            prop_assert!(pending.len() <= MAX_PENDING_STARTS);
            prop_assert_eq!(unique.len(), pending.len());
        }
    }
}
