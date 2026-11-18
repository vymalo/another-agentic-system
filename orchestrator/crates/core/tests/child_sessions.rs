//! Child sessions (`child-sessions/v1`, ADR 0072), the pure half: what the parent's log records
//! and when, what a child thread starts as, how an observed round begins, and what its usage is.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::*;
use proptest::prelude::*;
use uuid::Uuid;

fn parent_agent() -> Actor {
    Actor::agent(&AgentId::new("coder"), None)
}

fn child_id(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

fn round(number: u32, task: Option<&str>, state: Option<AgentTaskState>) -> RoundReport {
    RoundReport {
        number,
        task_id: task.map(Into::into),
        state,
    }
}

fn report(session: &str, state: SessionState, round: Option<RoundReport>) -> SessionReport {
    SessionReport {
        session: session.into(),
        context_id: format!("ctx/{session}"),
        agent: "planner".into(),
        description: "Plan the migration".into(),
        keep_open: true,
        by: Some(SessionBy::Agent),
        state,
        closing: false,
        round,
    }
}

fn tell(
    snapshot: &Snapshot,
    report: &SessionReport,
    child: ThreadId,
) -> Result<(Snapshot, Vec<EventBody>), TransitionError> {
    let (next, cmds) = transition(
        snapshot,
        &Input::ChildSession {
            actor: parent_agent(),
            report: report.clone(),
            child,
        },
    )?;
    Ok((next, bodies(&cmds)))
}

fn bodies(cmds: &[Command]) -> Vec<EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(d.body.clone()),
            _ => None,
        })
        .collect()
}

fn kinds(events: &[EventBody]) -> Vec<&'static str> {
    events.iter().map(|b| b.kind().as_str()).collect()
}

fn snapshot(state: ThreadState) -> Snapshot {
    Snapshot::new(state)
}

const ALL_STATES: [ThreadState; 6] = [Queued, Working, Blocked, Verifying, Done, Failed];

// --- the wire -------------------------------------------------------------------------------

#[test]
fn report_of_says_how_a_round_ended() {
    use AgentTaskState as T;
    let table = [
        (T::Completed, Some(SessionOutcome::Done)),
        (T::Failed, Some(SessionOutcome::Failed)),
        (T::Rejected, Some(SessionOutcome::Failed)),
        (T::Canceled, Some(SessionOutcome::Canceled)),
        (T::InputRequired, Some(SessionOutcome::Blocked)),
        (T::AuthRequired, Some(SessionOutcome::Blocked)),
        (T::Submitted, None),
        (T::Working, None),
    ];
    for (state, expected) in table {
        assert_eq!(report_of(state), expected, "{state:?}");
    }
}

#[test]
fn the_three_events_have_their_kinds_and_data() {
    let thread = child_id(7);
    let opened = EventBody::ChildSessionOpened(ChildSessionOpenedData {
        session: "s1".into(),
        thread,
        agent: "planner".into(),
        description: "Plan".into(),
        keep_open: true,
        by: Some(SessionBy::Person),
        round: Some(1),
    });
    let reported = EventBody::ChildSessionReported(ChildSessionReportedData {
        session: "s1".into(),
        thread,
        round: 2,
        report: SessionOutcome::Blocked,
    });
    let closed = EventBody::ChildSessionClosed(ChildSessionClosedData {
        session: "s1".into(),
        thread,
    });
    let id = thread.to_string();
    let table = [
        (
            opened,
            "child_session_opened",
            serde_json::json!({"session": "s1", "thread": id, "agent": "planner",
                "description": "Plan", "keepOpen": true, "by": "person", "round": 1}),
        ),
        (
            reported,
            "child_session_reported",
            serde_json::json!({"session": "s1", "thread": id, "round": 2, "report": "blocked"}),
        ),
        (
            closed,
            "child_session_closed",
            serde_json::json!({"session": "s1", "thread": id}),
        ),
    ];
    for (body, kind, data) in table {
        assert_eq!(body.kind().as_str(), kind);
        assert_eq!(body.data_value(), data);
        let wire = serde_json::to_value(Event {
            seq: 1,
            thread_id: child_id(1),
            at: "2026-11-18T10:00:00Z".parse().unwrap(),
            actor: parent_agent(),
            body: body.clone(),
        })
        .unwrap();
        assert_eq!(wire["kind"], kind);
        let back: Event = serde_json::from_value(wire).unwrap();
        assert_eq!(back.body, body);
    }
}

#[test]
fn an_opened_session_without_a_round_or_a_by_leaves_them_out() {
    let body = EventBody::ChildSessionOpened(ChildSessionOpenedData {
        session: "s1".into(),
        thread: child_id(7),
        agent: String::new(),
        description: String::new(),
        keep_open: false,
        by: None,
        round: None,
    });
    let data = body.data_value();
    assert!(data.get("by").is_none() && data.get("round").is_none());
}

#[test]
fn job_started_names_the_round_only_when_there_is_one() {
    let plain = serde_json::to_value(JobStartedData {
        job: 2,
        round: None,
    })
    .unwrap();
    assert_eq!(plain, serde_json::json!({"job": 2}));
    let round = serde_json::to_value(JobStartedData {
        job: 1,
        round: Some(JobRound {
            number: 3,
            by: Some(SessionBy::Person),
        }),
    })
    .unwrap();
    assert_eq!(
        round,
        serde_json::json!({"job": 1, "round": {"number": 3, "by": "person"}})
    );
    // an event written before the member existed reads as it always did
    let old: JobStartedData = serde_json::from_value(serde_json::json!({"job": 2})).unwrap();
    assert_eq!(old.round, None);
}

#[test]
fn the_door_checks_every_bound() {
    let good = report("s1", SessionState::Working, Some(round(1, Some("t"), None)));
    assert_eq!(good.check(), Ok(()));
    let long = |n: usize| "x".repeat(n);
    let table: Vec<(&str, SessionReport, SessionInvalid)> = vec![
        (
            "empty session",
            SessionReport {
                session: String::new(),
                ..good.clone()
            },
            SessionInvalid::BadSession,
        ),
        (
            "session too long",
            SessionReport {
                session: long(MAX_SESSION_ID_BYTES + 1),
                ..good.clone()
            },
            SessionInvalid::BadSession,
        ),
        (
            "session with a control character",
            SessionReport {
                session: "a\nb".into(),
                ..good.clone()
            },
            SessionInvalid::BadSession,
        ),
        (
            "empty context",
            SessionReport {
                context_id: String::new(),
                ..good.clone()
            },
            SessionInvalid::BadContext,
        ),
        (
            "context too long",
            SessionReport {
                context_id: long(MAX_SESSION_CONTEXT_BYTES + 1),
                ..good.clone()
            },
            SessionInvalid::BadContext,
        ),
        (
            "agent too long",
            SessionReport {
                agent: long(MAX_SESSION_AGENT_BYTES + 1),
                ..good.clone()
            },
            SessionInvalid::BadAgent,
        ),
        (
            "description too long",
            SessionReport {
                description: long(MAX_SESSION_DESCRIPTION_CHARS + 1),
                ..good.clone()
            },
            SessionInvalid::BadDescription,
        ),
        (
            "round zero",
            SessionReport {
                round: Some(round(0, None, None)),
                ..good.clone()
            },
            SessionInvalid::BadRound,
        ),
        (
            "task too long",
            SessionReport {
                round: Some(round(1, Some(&long(MAX_SESSION_TASK_BYTES + 1)), None)),
                ..good.clone()
            },
            SessionInvalid::BadRound,
        ),
    ];
    for (why, bad, expected) in table {
        assert_eq!(bad.check(), Err(expected), "{why}");
    }
    // the limits themselves are fine
    let edge = SessionReport {
        session: long(MAX_SESSION_ID_BYTES),
        context_id: long(MAX_SESSION_CONTEXT_BYTES),
        agent: long(MAX_SESSION_AGENT_BYTES),
        description: long(MAX_SESSION_DESCRIPTION_CHARS),
        ..good
    };
    assert_eq!(edge.check(), Ok(()));
}

// --- the parent's events --------------------------------------------------------------------

#[test]
fn a_session_met_for_the_first_time_writes_what_it_already_is() {
    use AgentTaskState as T;
    let ended = |state| Some(round(1, Some("t1"), state));
    let table: Vec<(&str, SessionState, Option<RoundReport>, Vec<&str>)> = vec![
        (
            "no round, working",
            SessionState::Working,
            None,
            vec!["child_session_opened"],
        ),
        (
            "round running",
            SessionState::Working,
            ended(Some(T::Working)),
            vec!["child_session_opened"],
        ),
        (
            "round state unknown",
            SessionState::Working,
            ended(None),
            vec!["child_session_opened"],
        ),
        (
            "round done, idle",
            SessionState::Idle,
            ended(Some(T::Completed)),
            vec!["child_session_opened", "child_session_reported"],
        ),
        (
            "round failed, idle",
            SessionState::Idle,
            ended(Some(T::Failed)),
            vec!["child_session_opened", "child_session_reported"],
        ),
        (
            "round blocked, working",
            SessionState::Working,
            ended(Some(T::InputRequired)),
            vec!["child_session_opened", "child_session_reported"],
        ),
        (
            "round done, closed",
            SessionState::Closed,
            ended(Some(T::Completed)),
            vec![
                "child_session_opened",
                "child_session_reported",
                "child_session_closed",
            ],
        ),
        (
            "closed, no round",
            SessionState::Closed,
            None,
            vec!["child_session_opened", "child_session_closed"],
        ),
        (
            "closed, round running",
            SessionState::Closed,
            ended(Some(T::Working)),
            vec!["child_session_opened", "child_session_closed"],
        ),
    ];
    for (why, state, round, expected) in table {
        let (next, events) = tell(&snapshot(Working), &report("s1", state, round), child_id(1))
            .unwrap_or_else(|e| panic!("{why}: {e}"));
        assert_eq!(kinds(&events), expected, "{why}");
        assert_eq!(next.state, Working, "{why}: the parent does not move");
        assert_eq!(next.job.sessions.len(), 1, "{why}");
    }
}

#[test]
fn the_opened_event_says_who_and_which() {
    let r = report(
        "s1",
        SessionState::Working,
        Some(round(1, Some("t1"), Some(AgentTaskState::Working))),
    );
    let (_, events) = tell(&snapshot(Working), &r, child_id(9)).unwrap();
    assert_eq!(
        events,
        vec![EventBody::ChildSessionOpened(ChildSessionOpenedData {
            session: "s1".into(),
            thread: child_id(9),
            agent: "planner".into(),
            description: "Plan the migration".into(),
            keep_open: true,
            by: Some(SessionBy::Agent),
            round: Some(1),
        })]
    );
}

#[test]
fn the_events_are_the_parents_agent_words() {
    let r = report("s1", SessionState::Working, None);
    let (_, cmds) = transition(
        &snapshot(Working),
        &Input::ChildSession {
            actor: parent_agent(),
            report: r,
            child: child_id(1),
        },
    )
    .unwrap();
    let Command::Append(d) = &cmds[0] else {
        panic!("an event")
    };
    assert_eq!(d.actor, parent_agent());
}

#[test]
fn the_parent_hears_of_a_session_in_every_state_and_never_moves() {
    use AgentTaskState as T;
    let r = report(
        "s1",
        SessionState::Closed,
        Some(round(1, Some("t"), Some(T::Completed))),
    );
    for state in ALL_STATES.into_iter().chain([Cancelled]) {
        let (next, events) = tell(&snapshot(state), &r, child_id(1)).unwrap();
        assert_eq!(next.state, state, "{state:?}");
        assert_eq!(events.len(), 3, "{state:?}");
    }
}

#[test]
fn a_session_known_writes_only_what_is_new() {
    use AgentTaskState as T;
    let c = child_id(1);
    let (s, _) = tell(
        &snapshot(Working),
        &report(
            "s1",
            SessionState::Working,
            Some(round(1, Some("t1"), Some(T::Working))),
        ),
        c,
    )
    .unwrap();

    // the same report again: nothing
    let (s, events) = tell(
        &s,
        &report(
            "s1",
            SessionState::Working,
            Some(round(1, Some("t1"), Some(T::Working))),
        ),
        c,
    )
    .unwrap();
    assert!(events.is_empty());

    // round 1 ends: reported once
    let ended = report(
        "s1",
        SessionState::Idle,
        Some(round(1, Some("t1"), Some(T::Completed))),
    );
    let (s, events) = tell(&s, &ended, c).unwrap();
    assert_eq!(
        events,
        vec![EventBody::ChildSessionReported(ChildSessionReportedData {
            session: "s1".into(),
            thread: c,
            round: 1,
            report: SessionOutcome::Done,
        })]
    );
    let (s, events) = tell(&s, &ended, c).unwrap();
    assert!(events.is_empty(), "a replay writes nothing");

    // a stale report of an earlier round writes nothing, and does not lower the note
    let stale = report(
        "s1",
        SessionState::Idle,
        Some(round(1, Some("t1"), Some(T::Failed))),
    );
    let (s, events) = tell(&s, &stale, c).unwrap();
    assert!(events.is_empty());
    assert_eq!(s.job.sessions.note("s1").unwrap().reported, 1);

    // round 2 runs, then ends failed: reported once more
    let (s, events) = tell(
        &s,
        &report(
            "s1",
            SessionState::Working,
            Some(round(2, Some("t2"), Some(T::Working))),
        ),
        c,
    )
    .unwrap();
    assert!(events.is_empty());
    let (s, events) = tell(
        &s,
        &report(
            "s1",
            SessionState::Idle,
            Some(round(2, Some("t2"), Some(T::Failed))),
        ),
        c,
    )
    .unwrap();
    assert_eq!(kinds(&events), ["child_session_reported"]);

    // closed once
    let closed = report(
        "s1",
        SessionState::Closed,
        Some(round(2, Some("t2"), Some(T::Failed))),
    );
    let (s, events) = tell(&s, &closed, c).unwrap();
    assert_eq!(kinds(&events), ["child_session_closed"]);
    let (s, events) = tell(&s, &closed, c).unwrap();
    assert!(events.is_empty());
    // and a later report that still says working does not reopen it
    let (_, events) = tell(&s, &report("s1", SessionState::Working, None), child_id(99)).unwrap();
    assert!(events.is_empty());
}

#[test]
fn a_known_session_keeps_the_child_it_was_first_given() {
    let (s, _) = tell(
        &snapshot(Working),
        &report("s1", SessionState::Working, None),
        child_id(1),
    )
    .unwrap();
    let (_, events) = tell(&s, &report("s1", SessionState::Closed, None), child_id(2)).unwrap();
    let EventBody::ChildSessionClosed(d) = &events[0] else {
        panic!("closed")
    };
    assert_eq!(d.thread, child_id(1));
}

#[test]
fn a_parent_notes_at_most_256_sessions() {
    let mut s = snapshot(Working);
    for n in 0..MAX_CHILD_SESSIONS {
        let (next, events) = tell(
            &s,
            &report(&format!("s{n}"), SessionState::Working, None),
            child_id(n as u128),
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        s = next;
    }
    assert_eq!(s.job.sessions.len(), MAX_CHILD_SESSIONS);
    let refused = tell(
        &s,
        &report("one-more", SessionState::Working, None),
        child_id(999),
    )
    .unwrap_err();
    assert_eq!(
        refused,
        TransitionError::ChildSessionRefused(SessionRefusal::LedgerFull)
    );
    assert_eq!(refused.class(), ErrorClass::Rejected);
    // a session already noted is still heard when the ledger is full
    let (_, events) = tell(&s, &report("s0", SessionState::Closed, None), child_id(0)).unwrap();
    assert_eq!(kinds(&events), ["child_session_closed"]);
}

#[test]
fn a_malformed_report_is_refused_and_writes_nothing() {
    let bad = SessionReport {
        session: String::new(),
        ..report("s1", SessionState::Working, None)
    };
    for input in [
        Input::ChildSession {
            actor: parent_agent(),
            report: bad.clone(),
            child: child_id(1),
        },
        Input::ChildSessionSaid(bad.clone()),
    ] {
        let err = transition(&snapshot(Working), &input).unwrap_err();
        assert_eq!(
            err,
            TransitionError::ChildSessionRefused(SessionRefusal::Malformed)
        );
        assert_eq!(err.class(), ErrorClass::Invalid);
    }
}

// --- the child thread -----------------------------------------------------------------------

#[test]
fn a_child_thread_starts_queued_with_no_event_and_follows_its_round() {
    let r = report(
        "s1",
        SessionState::Working,
        Some(round(1, Some("t1"), Some(AgentTaskState::Working))),
    );
    let (snap, cmds) = start_child(&r);
    assert_eq!(snap.state, Queued);
    assert_eq!(
        cmds,
        vec![Command::Observe {
            session: "s1".into(),
            round: 1,
            task_id: "t1".into()
        }]
    );
    assert_eq!(
        snap.job.child,
        Some(ChildLedger {
            session: "s1".into(),
            agent: "planner".into(),
            keep_open: true,
            state: SessionState::Working,
            closing: false,
            followed: 1,
            started: 0,
        })
    );
    // the model is never asked to title or describe it
    assert!(!snap.job.title.may_ask());
    assert!(!snap.job.description.may_ask(1));
    assert_eq!(snap.job.observed, None);
}

#[test]
fn a_child_with_no_round_to_follow_orders_nothing() {
    for round in [None, Some(round(1, None, None))] {
        let (snap, cmds) = start_child(&report("s1", SessionState::Idle, round));
        assert!(cmds.is_empty());
        assert_eq!(snap.job.child.unwrap().followed, 0);
    }
}

#[test]
fn the_title_is_the_description_else_the_helper_else_a_word() {
    let with = |description: &str, agent: &str| {
        child_title(&SessionReport {
            description: description.into(),
            agent: agent.into(),
            ..report("s1", SessionState::Idle, None)
        })
    };
    assert_eq!(
        with("  Plan the migration ", "planner"),
        "Plan the migration"
    );
    assert_eq!(with("", "planner"), "planner");
    assert_eq!(with("line\nbreak", "planner"), "planner");
    assert_eq!(with(&"d".repeat(MAX_TITLE_CHARS + 1), "planner"), "planner");
    assert_eq!(with("", ""), "Helper");
}

#[test]
fn a_child_is_told_of_a_new_round_once() {
    use AgentTaskState as T;
    let (snap, _) = start_child(&report(
        "s1",
        SessionState::Working,
        Some(round(1, Some("t1"), Some(T::Working))),
    ));
    let said =
        |snap: &Snapshot, r: SessionReport| transition(snap, &Input::ChildSessionSaid(r)).unwrap();

    // round 1 again: nothing to order, but the ledger follows the state
    let (snap, cmds) = said(
        &snap,
        SessionReport {
            closing: true,
            ..report(
                "s1",
                SessionState::Working,
                Some(round(1, Some("t1"), Some(T::Working))),
            )
        },
    );
    assert!(cmds.is_empty());
    assert!(snap.job.child.as_ref().unwrap().closing);

    // round 2 with a task: ordered once
    let two = report(
        "s1",
        SessionState::Working,
        Some(round(2, Some("t2"), Some(T::Working))),
    );
    let (snap, cmds) = said(&snap, two.clone());
    assert_eq!(
        cmds,
        vec![Command::Observe {
            session: "s1".into(),
            round: 2,
            task_id: "t2".into()
        }]
    );
    let (snap, cmds) = said(&snap, two);
    assert!(cmds.is_empty(), "the same round is not ordered twice");

    // round 3 without a task cannot be followed yet, and is not marked followed
    let (snap, cmds) = said(
        &snap,
        report("s1", SessionState::Working, Some(round(3, None, None))),
    );
    assert!(cmds.is_empty());
    assert_eq!(snap.job.child.as_ref().unwrap().followed, 2);

    // the session closes
    let (snap, cmds) = said(&snap, report("s1", SessionState::Closed, None));
    assert!(cmds.is_empty());
    assert_eq!(snap.job.child.unwrap().state, SessionState::Closed);
    assert!(snap.job.sessions.is_empty());
}

#[test]
fn what_is_said_of_another_session_or_to_a_thread_that_is_no_child_changes_nothing() {
    let (child, _) = start_child(&report("s1", SessionState::Working, None));
    let (after, cmds) = transition(
        &child,
        &Input::ChildSessionSaid(report("other", SessionState::Closed, None)),
    )
    .unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, child);

    let plain = snapshot(Working);
    let (after, cmds) = transition(
        &plain,
        &Input::ChildSessionSaid(report("s1", SessionState::Closed, None)),
    )
    .unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, plain);
}

#[test]
fn what_is_said_is_heard_in_every_state_and_moves_nothing() {
    let r = report(
        "s1",
        SessionState::Idle,
        Some(round(2, Some("t2"), Some(AgentTaskState::Completed))),
    );
    for state in ALL_STATES.into_iter().chain([Cancelled]) {
        let (mut snap, _) = start_child(&report("s1", SessionState::Working, None));
        snap.state = state;
        let (next, cmds) = transition(&snap, &Input::ChildSessionSaid(r.clone())).unwrap();
        assert_eq!(next.state, state);
        assert_eq!(cmds.len(), 1, "{state:?}");
    }
}

fn started(
    snap: &Snapshot,
    number: u32,
    by: Option<SessionBy>,
) -> Result<(Snapshot, Vec<Command>), TransitionError> {
    transition(
        snap,
        &Input::RoundStarted {
            session: "s1".into(),
            number,
            task_id: format!("t{number}"),
            by,
        },
    )
}

#[test]
fn a_fresh_child_starts_its_first_job_in_place() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let (next, cmds) = started(&snap, 1, Some(SessionBy::Agent)).unwrap();
    assert_eq!(next.state, Working);
    assert_eq!(next.job.number, 1);
    assert_eq!(
        next.job.observed,
        Some(ObservedRound {
            number: 1,
            task_id: "t1".into(),
            by: Some(SessionBy::Agent)
        })
    );
    assert_eq!(
        bodies(&cmds),
        vec![EventBody::JobStarted(JobStartedData {
            job: 1,
            round: Some(JobRound {
                number: 1,
                by: Some(SessionBy::Agent)
            })
        })]
    );
}

#[test]
fn a_round_starts_in_every_state() {
    let (fresh, _) = start_child(&report("s1", SessionState::Working, None));
    for state in [Blocked, Done, Failed, Cancelled] {
        let mut snap = fresh.clone();
        snap.state = state;
        snap.job.observed = Some(ObservedRound {
            number: 1,
            task_id: "t1".into(),
            by: None,
        });
        let (next, cmds) = started(&snap, 2, Some(SessionBy::Person)).unwrap();
        assert_eq!(next.state, Working, "{state:?}");
        assert_eq!(next.job.number, 2, "{state:?}");
        assert_eq!(next.job.observed.as_ref().unwrap().number, 2);
        assert_eq!(
            bodies(&cmds),
            vec![EventBody::JobStarted(JobStartedData {
                job: 2,
                round: Some(JobRound {
                    number: 2,
                    by: Some(SessionBy::Person)
                })
            })],
            "{state:?}"
        );
        // the ledgers of the conversation come along
        assert_eq!(next.job.child.as_ref().unwrap().session, "s1");
        assert_eq!(next.job.child.as_ref().unwrap().followed, 2);
    }
    // a thread that works, or is verified, or that is queued after a round, refuses
    for state in [Working, Verifying] {
        let mut snap = fresh.clone();
        snap.state = state;
        snap.job.observed = Some(ObservedRound {
            number: 1,
            task_id: "t1".into(),
            by: None,
        });
        let err = started(&snap, 2, None).unwrap_err();
        assert!(
            matches!(err, TransitionError::InvalidInState { .. }),
            "{state:?}"
        );
        assert_eq!(err.class(), ErrorClass::Rejected);
    }
    let mut busy = fresh.clone();
    busy.job = busy.job.next();
    let err = started(&busy, 2, None).unwrap_err();
    assert!(matches!(err, TransitionError::InvalidInState { .. }));
}

#[test]
fn the_same_round_twice_changes_nothing() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let (once, _) = started(&snap, 1, None).unwrap();
    for state in [Working, Done, Blocked] {
        let mut again = once.clone();
        again.state = state;
        let (next, cmds) = started(&again, 1, None).unwrap();
        assert_eq!(next, again, "{state:?}");
        assert!(cmds.is_empty());
    }
}

#[test]
fn the_ledgers_of_the_conversation_stay_and_the_round_does_not() {
    let (parent, _) = tell(
        &snapshot(Done),
        &report("s1", SessionState::Working, None),
        child_id(1),
    )
    .unwrap();
    let (mut child, _) = start_child(&report("s1", SessionState::Working, None));
    child.job.sessions = parent.job.sessions.clone();
    child.job.observed = Some(ObservedRound {
        number: 1,
        task_id: "t1".into(),
        by: None,
    });
    let next = child.job.next();
    assert_eq!(next.sessions, parent.job.sessions);
    assert_eq!(next.child, child.job.child);
    assert_eq!(next.observed, None);
}

#[test]
fn a_fork_has_no_sessions_and_is_no_child() {
    let (parent, _) = tell(
        &snapshot(Done),
        &report("s1", SessionState::Closed, None),
        child_id(1),
    )
    .unwrap();
    assert_eq!(parent.job.sessions.len(), 1);
    let forked = forked_snapshot(
        &[],
        GatePolicy::default(),
        parent.job.title,
        parent.job.description,
    );
    assert!(forked.job.sessions.is_empty());
    assert_eq!(forked.job.child, None);
    assert_eq!(forked.job.observed, None);
}

#[test]
fn the_child_of_names_its_parent_and_session() {
    let of = ChildOf {
        thread: child_id(1),
        session: "s1".into(),
    };
    assert_eq!(
        serde_json::to_value(&of).unwrap(),
        serde_json::json!({"thread": child_id(1).to_string(), "session": "s1"})
    );
}

// --- usage ----------------------------------------------------------------------------------

fn usage_inputs() -> Vec<Input> {
    let tokens = TokenCounts {
        input_tokens: 5,
        output_tokens: 2,
        total_tokens: 7,
        ..TokenCounts::default()
    };
    vec![
        Input::Agent {
            agent: AgentId::new("planner"),
            revision: None,
            update: AgentUpdate::Usage(UsageUpdate::Call(UsageCall {
                task: "t1".into(),
                call: "c1".into(),
                step: None,
                provider: None,
                model: "m".into(),
                tokens,
                context_window: None,
            })),
        },
        Input::Agent {
            agent: AgentId::new("planner"),
            revision: None,
            update: AgentUpdate::Usage(UsageUpdate::Total(UsageTotals {
                task: "t1".into(),
                totals: vec![ModelTokens {
                    provider: None,
                    model: "m".into(),
                    tokens,
                }],
                sandbox: None,
                sandbox_rejected: None,
            })),
        },
    ]
}

fn counted(body: &EventBody) -> bool {
    match body {
        EventBody::ModelUsage(d) => d.counted,
        EventBody::ModelUsageTotal(d) => d.counted,
        other => panic!("not usage: {other:?}"),
    }
}

#[test]
fn a_round_only_followed_is_not_counted_unless_a_person_started_it() {
    let table = [
        (None, false),
        (Some(SessionBy::Agent), false),
        (Some(SessionBy::Person), true),
    ];
    for (by, expected) in table {
        let (snap, _) = start_child(&report("s1", SessionState::Working, None));
        let (snap, _) = started(&snap, 1, by).unwrap();
        for input in usage_inputs() {
            let (_, cmds) = transition(&snap, &input).unwrap();
            let events = bodies(&cmds);
            assert_eq!(events.len(), 1);
            assert_eq!(counted(&events[0]), expected, "by {by:?}");
        }
    }
}

#[test]
fn a_thread_that_follows_no_round_counts_as_it_always_did() {
    for input in usage_inputs() {
        let (_, cmds) = transition(&snapshot(Working), &input).unwrap();
        assert!(counted(&bodies(&cmds)[0]));
    }
}

#[test]
fn an_asked_agents_usage_in_a_followed_round_is_not_counted_either() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let (snap, _) = started(&snap, 1, Some(SessionBy::Agent)).unwrap();
    let usage = usage_inputs()
        .into_iter()
        .find_map(|i| match i {
            Input::Agent {
                update: AgentUpdate::Usage(u),
                ..
            } => Some(u),
            _ => None,
        })
        .unwrap();
    let mut with_ask = snap.clone();
    with_ask.job.asks = Vec::new();
    // no ask 1 exists in this job: the report is dropped, never counted by accident
    let (_, cmds) = transition(
        &with_ask,
        &Input::AskUsage {
            job: 1,
            ask: 1,
            revision: None,
            usage,
        },
    )
    .unwrap();
    assert!(bodies(&cmds).is_empty());
}

#[test]
fn uncounted_is_written_only_when_false_and_read_back() {
    let data = |counted| ModelUsageData {
        job: 1,
        agent: "a".into(),
        task: "t".into(),
        call: "c".into(),
        path: vec![],
        provider: None,
        model: "m".into(),
        tokens: TokenCounts::default(),
        context_window: None,
        counted,
    };
    assert!(
        serde_json::to_value(data(true))
            .unwrap()
            .get("counted")
            .is_none()
    );
    let off = serde_json::to_value(data(false)).unwrap();
    assert_eq!(off["counted"], false);
    assert!(
        !serde_json::from_value::<ModelUsageData>(off)
            .unwrap()
            .counted
    );
    // an event written before the member existed was counted
    let mut old = serde_json::to_value(data(true)).unwrap();
    old.as_object_mut().unwrap().remove("counted");
    assert!(
        serde_json::from_value::<ModelUsageData>(old)
            .unwrap()
            .counted
    );

    let total = |counted| ModelUsageTotalData {
        job: 1,
        agent: "a".into(),
        task: "t".into(),
        path: vec![],
        totals: vec![],
        sandbox: None,
        counted,
    };
    assert!(
        serde_json::to_value(total(true))
            .unwrap()
            .get("counted")
            .is_none()
    );
    let off = serde_json::to_value(total(false)).unwrap();
    assert!(
        !serde_json::from_value::<ModelUsageTotalData>(off)
            .unwrap()
            .counted
    );
}

#[test]
fn the_ledger_skips_what_is_not_counted() {
    let event = |counted| Event {
        seq: 1,
        thread_id: child_id(1),
        at: "2026-11-18T10:00:00Z".parse().unwrap(),
        actor: Actor::agent(&AgentId::new("planner"), None),
        body: EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: "planner".into(),
            task: "t".into(),
            call: "c".into(),
            path: vec![],
            provider: None,
            model: "m".into(),
            tokens: TokenCounts::default(),
            context_window: None,
            counted,
        })),
    };
    let owner = UserId::new("a@example.com");
    assert!(usage_change(&owner, &event(true)).is_some());
    assert!(usage_change(&owner, &event(false)).is_none());
    let total = Event {
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "planner".into(),
            task: "t".into(),
            path: vec![],
            totals: vec![],
            sandbox: None,
            counted: false,
        }),
        ..event(true)
    };
    assert!(usage_change(&owner, &total).is_none());
}

// --- the property ---------------------------------------------------------------------------

fn arb_report() -> impl Strategy<Value = SessionReport> {
    let session = prop_oneof![Just("a"), Just("b"), Just("c")];
    let state = prop_oneof![
        Just(SessionState::Working),
        Just(SessionState::Idle),
        Just(SessionState::Closed)
    ];
    let round_state = proptest::option::of(prop_oneof![
        Just(AgentTaskState::Working),
        Just(AgentTaskState::Completed),
        Just(AgentTaskState::Failed),
        Just(AgentTaskState::Canceled),
        Just(AgentTaskState::InputRequired),
    ]);
    let round = proptest::option::of((1u32..4, round_state).prop_map(|(n, s)| RoundReport {
        number: n,
        task_id: Some(format!("t{n}")),
        state: s,
    }));
    (session, state, round).prop_map(|(s, state, round)| report(s, state, round))
}

fn run(reports: &[SessionReport]) -> (Snapshot, Vec<EventBody>) {
    let mut snap = snapshot(Working);
    let mut events = Vec::new();
    for r in reports {
        let child = child_id(u128::from(r.session.as_bytes()[0]));
        let (next, mut e) = tell(&snap, r, child).unwrap();
        snap = next;
        events.append(&mut e);
    }
    (snap, events)
}

proptest! {
    /// However the reports come, in whatever order and however often, a session is opened once,
    /// closed once, and its rounds are reported in strictly increasing order, once each.
    #[test]
    fn the_parents_events_are_once_each(reports in proptest::collection::vec(arb_report(), 0..24), repeat in 1usize..4) {
        let (_, once) = run(&reports);
        let doubled: Vec<SessionReport> = reports.iter().flat_map(|r| std::iter::repeat_n(r.clone(), repeat)).collect();
        let (_, repeated) = run(&doubled);
        prop_assert_eq!(&once, &repeated, "repeating a report changes nothing");

        for session in ["a", "b", "c"] {
            let mut opened = 0;
            let mut closed = 0;
            let mut last = 0;
            for event in &once {
                match event {
                    EventBody::ChildSessionOpened(d) if d.session == session => opened += 1,
                    EventBody::ChildSessionClosed(d) if d.session == session => {
                        closed += 1;
                        prop_assert!(opened == 1, "closed before opened");
                    }
                    EventBody::ChildSessionReported(d) if d.session == session => {
                        prop_assert!(opened == 1, "reported before opened");
                        prop_assert!(d.round > last, "rounds are reported in increasing order");
                        last = d.round;
                    }
                    _ => {}
                }
            }
            prop_assert!(opened <= 1 && closed <= 1);
        }

        // the whole story again says nothing new
        let (snap, _) = run(&reports);
        let mut replay = snap;
        for r in &reports {
            let child = child_id(u128::from(r.session.as_bytes()[0]));
            let (next, e) = tell(&replay, r, child).unwrap();
            prop_assert!(e.is_empty());
            replay = next;
        }
    }
}

#[test]
fn a_stale_round_after_a_newer_one_is_a_no_op() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let (r1, _) = started(&snap, 1, None).unwrap();
    for end in [Done, Blocked] {
        let mut two = started(
            &{
                let mut s = r1.clone();
                s.state = Done;
                s
            },
            2,
            Some(SessionBy::Person),
        )
        .unwrap()
        .0;
        two.state = end;
        let (next, cmds) = started(&two, 1, None).unwrap();
        assert_eq!(next, two, "{end:?}");
        assert!(cmds.is_empty(), "{end:?}");
        assert_eq!(next.job.number, 2);
        assert_eq!(next.job.observed.as_ref().unwrap().number, 2);
    }
}

#[test]
fn a_round_is_refused_by_a_thread_that_is_not_that_sessions_child_or_with_number_zero() {
    let (child, _) = start_child(&report("s1", SessionState::Working, None));
    let err = started(&child, 0, None).unwrap_err();
    assert!(matches!(err, TransitionError::InvalidInState { .. }));
    let other = transition(
        &child,
        &Input::RoundStarted {
            session: "other".into(),
            number: 1,
            task_id: "t1".into(),
            by: None,
        },
    )
    .unwrap_err();
    assert!(matches!(other, TransitionError::InvalidInState { .. }));
    let plain = snapshot(Done);
    let err = started(&plain, 1, None).unwrap_err();
    assert!(matches!(err, TransitionError::InvalidInState { .. }));
    assert_eq!(err.class(), ErrorClass::Rejected);
}

#[test]
fn a_round_with_no_by_is_the_agents_and_not_counted_in_the_child() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let (next, _) = started(&snap, 1, None).unwrap();
    assert!(!next.job.observed.as_ref().unwrap().counts_usage());
    let (next, _) = started(&snap, 1, Some(SessionBy::Person)).unwrap();
    assert!(next.job.observed.as_ref().unwrap().counts_usage());
}

#[test]
fn a_closed_session_is_never_reported_afterwards() {
    let (parent, cmds) = tell(
        &snapshot(Done),
        &report("s1", SessionState::Closed, None),
        child_id(1),
    )
    .unwrap();
    assert_eq!(cmds.len(), 2);
    let (after, cmds) = tell(
        &parent,
        &report(
            "s1",
            SessionState::Closed,
            Some(round(3, Some("t3"), Some(AgentTaskState::Completed))),
        ),
        child_id(1),
    )
    .unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after.job.sessions.note("s1").unwrap().reported, 0);
}

#[test]
fn a_closed_child_stays_closed_whatever_is_said_after() {
    let (mut child, _) = start_child(&report("s1", SessionState::Closed, None));
    for state in [SessionState::Working, SessionState::Idle] {
        let (next, _) =
            transition(&child, &Input::ChildSessionSaid(report("s1", state, None))).unwrap();
        assert_eq!(next.job.child.as_ref().unwrap().state, SessionState::Closed);
        child = next;
    }
}

#[test]
fn a_forked_child_thread_carries_no_child_ledger() {
    let (mut child, _) = start_child(&report("s1", SessionState::Working, None));
    child.job.observed = Some(ObservedRound {
        number: 1,
        task_id: "t1".into(),
        by: None,
    });
    let forked = forked_snapshot(
        &[],
        GatePolicy::default(),
        child.job.title,
        child.job.description,
    );
    assert_eq!(forked.job.child, None);
    assert_eq!(forked.job.observed, None);
    assert!(forked.job.sessions.is_empty());
}

// --- mutation pass (S2a) ---------------------------------------------------------------------

#[test]
fn a_replayed_round_after_its_job_moved_on_is_a_no_op() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let (mut once, _) = started(&snap, 1, None).unwrap();
    // the job ended and the next one began: `observed` is forgotten, `started` is kept
    once.state = Done;
    once.job = once.job.next();
    assert_eq!(once.job.observed, None);
    let (next, cmds) = started(&once, 1, None).unwrap();
    assert_eq!(next, once);
    assert!(cmds.is_empty());
}

#[test]
fn what_is_said_updates_keep_open_and_keeps_the_name_when_none_is_given() {
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    assert!(snap.job.child.as_ref().unwrap().keep_open);
    let mut r = report("s1", SessionState::Idle, None);
    r.keep_open = false;
    r.agent = String::new();
    let (snap, _) = transition(&snap, &Input::ChildSessionSaid(r)).unwrap();
    let child = snap.job.child.unwrap();
    assert!(!child.keep_open);
    assert_eq!(child.agent, "planner", "an empty name does not replace it");
}

#[test]
fn starting_an_older_round_does_not_lower_what_was_followed() {
    use AgentTaskState as T;
    let (snap, _) = start_child(&report("s1", SessionState::Working, None));
    let two = report(
        "s1",
        SessionState::Working,
        Some(round(2, Some("t2"), Some(T::Working))),
    );
    let (snap, cmds) = transition(&snap, &Input::ChildSessionSaid(two.clone())).unwrap();
    assert_eq!(cmds.len(), 1);
    // round 1 is started late: followed stays 2
    let (snap, _) = started(&snap, 1, None).unwrap();
    assert_eq!(snap.job.child.as_ref().unwrap().followed, 2);
    let (_, cmds) = transition(&snap, &Input::ChildSessionSaid(two)).unwrap();
    assert!(cmds.is_empty(), "round 2 is not ordered a second time");
}

#[test]
fn the_child_ledger_is_written_and_read_back_whole() {
    let mut ledger = ChildLedger {
        session: "s1".into(),
        agent: "planner".into(),
        keep_open: false,
        state: SessionState::Idle,
        closing: false,
        followed: 0,
        started: 0,
    };
    let v = serde_json::to_value(&ledger).unwrap();
    assert!(
        v.get("closing").is_none() && v.get("started").is_none(),
        "{v}"
    );
    ledger.closing = true;
    ledger.followed = 3;
    ledger.started = 2;
    let v = serde_json::to_value(&ledger).unwrap();
    assert_eq!(v["closing"], true);
    assert_eq!(v["followed"], 3);
    assert_eq!(v["started"], 2);
    let back: ChildLedger = serde_json::from_value(v).unwrap();
    assert_eq!(back, ledger);
    // a ledger written without the optional members reads as zero and false
    let bare: ChildLedger = serde_json::from_value(serde_json::json!({
        "session": "s1", "agent": "a", "keepOpen": true, "state": "working"
    }))
    .unwrap();
    assert_eq!((bare.followed, bare.started, bare.closing), (0, 0, false));
}

#[test]
fn a_round_with_no_by_leaves_it_out_of_job_started() {
    let v = serde_json::to_value(JobStartedData {
        job: 1,
        round: Some(JobRound {
            number: 2,
            by: None,
        }),
    })
    .unwrap();
    assert_eq!(v, serde_json::json!({"job": 1, "round": {"number": 2}}));
}
