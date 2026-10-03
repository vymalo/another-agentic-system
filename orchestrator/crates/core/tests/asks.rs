//! Asked agents in the job ledger (ADR 0026, `orch_core::ask`): one test per rule, then a property
//! over random sequences. The checks of who may ask what on the thread's tools endpoint are the
//! application's; here the input is taken as authorised and the core decides what the job allows.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};

use jiff::SignedDuration;
use orch_core::*;
use proptest::prelude::*;
use serde_json::json;

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};

fn user() -> UserId {
    UserId::new("me@example.com")
}
fn agent(id: &str) -> AgentId {
    AgentId::new(id)
}
fn coder() -> Actor {
    Actor::agent(&agent("coder"), Some("r1".into()))
}
fn set(ids: &[&str]) -> BTreeSet<AgentId> {
    ids.iter().map(|id| agent(id)).collect()
}

/// A job that runs (`state`) and whose person mentioned `mentioned`.
fn running(state: ThreadState, mentioned: &[&str]) -> Snapshot {
    let mut snap = Snapshot::new(state);
    snap.job.mentioned = set(mentioned);
    snap
}

/// What an agent that pushed a commit leaves in the ledger, so that its completion is verified.
fn pushed() -> PushedRef {
    PushedRef {
        repository: "github.com/o/r".into(),
        branch: "work".into(),
        commit: "a".repeat(40),
    }
}

fn ask_by(caller: Caller, to: &str, text: &str) -> Input {
    Input::Ask {
        actor: coder(),
        caller,
        agent: agent(to),
        text: text.into(),
        call_key: None,
        parent_step: None,
        limits: AskLimits::default(),
    }
}
fn ask(to: &str) -> Input {
    ask_by(Caller::Main, to, "find the data")
}
fn with_limits(input: Input, limits: AskLimits) -> Input {
    let Input::Ask {
        actor,
        caller,
        agent,
        text,
        call_key,
        parent_step,
        ..
    } = input
    else {
        panic!("an ask");
    };
    Input::Ask {
        actor,
        caller,
        agent,
        text,
        call_key,
        parent_step,
        limits,
    }
}
fn with_key(input: Input, key: &str) -> Input {
    let Input::Ask {
        actor,
        caller,
        agent,
        text,
        parent_step,
        limits,
        ..
    } = input
    else {
        panic!("an ask");
    };
    Input::Ask {
        actor,
        caller,
        agent,
        text,
        call_key: Some(key.into()),
        parent_step,
        limits,
    }
}

fn step(snap: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snap, input).unwrap()
}
fn refused(snap: &Snapshot, input: &Input) -> AskRefusal {
    match transition(snap, input) {
        Err(TransitionError::AskRefused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn finished(ask: u32, outcome: AskOutcome) -> Input {
    Input::AskFinished {
        job: 1,
        ask,
        revision: Some("r9".into()),
        result: AskResult::of(outcome),
    }
}
fn cancel() -> Input {
    Input::Cancel { user: user() }
}
fn status(state: AgentTaskState) -> Input {
    Input::Agent {
        agent: agent("coder"),
        revision: None,
        update: AgentUpdate::Status {
            state,
            detail: None,
        },
    }
}
fn timer(job: u32, ask: u32) -> Input {
    Input::TimerFired(Timer::AskDeadline { job, ask })
}

fn started(cmds: &[Command]) -> Vec<&AskStartedData> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(EventDraft {
                body: EventBody::AskStarted(d),
                ..
            }) => Some(d),
            _ => None,
        })
        .collect()
}
fn finishes(cmds: &[Command]) -> Vec<(&Actor, &AskFinishedData)> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(EventDraft {
                actor,
                body: EventBody::AskFinished(d),
            }) => Some((actor, d)),
            _ => None,
        })
        .collect()
}
fn finish_of(cmds: &[Command], n: u32) -> &AskFinishedData {
    finishes(cmds)
        .into_iter()
        .find(|(_, d)| d.ask == n)
        .map(|(_, d)| d)
        .unwrap_or_else(|| panic!("no ask_finished for ask {n} in {cmds:?}"))
}
fn outcomes(snap: &Snapshot) -> Vec<(u32, Option<AskOutcome>)> {
    snap.job.asks.iter().map(|a| (a.n, a.outcome)).collect()
}
fn index_of_thread_state(cmds: &[Command]) -> Option<usize> {
    cmds.iter().position(|c| {
        matches!(
            c,
            Command::Append(EventDraft {
                body: EventBody::ThreadState(_),
                ..
            })
        )
    })
}

// ---- accepted ------------------------------------------------------------------------------

#[test]
fn an_ask_of_a_mentioned_agent_is_logged_sent_and_given_a_deadline() {
    let snap = running(Working, &["mock-researcher"]);
    let input = Input::Ask {
        actor: coder(),
        caller: Caller::Main,
        agent: agent("mock-researcher"),
        text: "find the data".into(),
        call_key: Some("ask:t:main:c1".into()),
        parent_step: Some("t/call-7".into()),
        limits: AskLimits::default(),
    };
    let (next, cmds) = step(&snap, &input);
    assert_eq!(next.state, Working, "the thread is as it was");
    assert_eq!(
        next.job.asks,
        [Ask {
            n: 1,
            by: Caller::Main,
            agent: agent("mock-researcher"),
            depth: 1,
            call_key: Some("ask:t:main:c1".into()),
            fingerprint: Some(fingerprint(&agent("mock-researcher"), "find the data")),
            task_id: None,
            outcome: None,
        }]
    );
    let [
        Command::Append(EventDraft { actor, body }),
        Command::Ask {
            job,
            ask,
            agent: to,
            depth,
            text,
            continue_task,
            reference_task_ids,
        },
        Command::Schedule { after, timer },
    ] = cmds.as_slice()
    else {
        panic!("an event, the row and the deadline: {cmds:?}");
    };
    assert_eq!(actor, &coder(), "the asking agent's");
    let EventBody::AskStarted(data) = body else {
        panic!("ask_started");
    };
    assert_eq!(
        serde_json::to_value(data).unwrap(),
        json!({"ask": 1, "agent": "mock-researcher", "by": "main", "depth": 1,
               "text": "find the data", "stepId": "ask-1", "parentStepId": "t/call-7"})
    );
    assert_eq!(
        (*job, *ask, to, *depth, text.as_str()),
        (1, 1, &agent("mock-researcher"), 1, "find the data")
    );
    assert!(continue_task.is_none() && reference_task_ids.is_empty());
    assert_eq!(*after, SignedDuration::from_secs(1800));
    assert_eq!(*timer, Timer::AskDeadline { job: 1, ask: 1 });
}

#[test]
fn the_deadline_is_the_limit_the_input_carries() {
    let snap = running(Working, &["a"]);
    let limits = AskLimits::default().with_timeout(std::time::Duration::from_secs(90));
    let (_, cmds) = step(&snap, &with_limits(ask("a"), limits));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Schedule { after, .. } if *after == SignedDuration::from_secs(90)
    )));
}

#[test]
fn a_queued_job_may_ask_too_because_the_agent_can_call_before_its_working_is_read() {
    let snap = running(Queued, &["a"]);
    assert_eq!(step(&snap, &ask("a")).0.job.asks.len(), 1);
}

#[test]
fn asks_are_numbered_from_one_in_the_order_they_were_accepted() {
    let mut snap = running(Working, &["a", "b"]);
    for (want, to) in [(1, "a"), (2, "b"), (3, "a")] {
        snap = step(&snap, &ask(to)).0;
        assert_eq!(snap.job.asks.last().unwrap().n, want);
    }
}

#[test]
fn an_asked_agent_may_ask_one_in_turn_and_the_chain_is_recorded() {
    let snap = running(Working, &["b", "c"]);
    let (snap, _) = step(&snap, &ask("b"));
    let (snap, cmds) = step(&snap, &ask_by(Caller::Ask(1), "c", "and you?"));
    let [start] = started(&cmds)[..] else {
        panic!("one ask_started");
    };
    assert_eq!((start.ask, start.by, start.depth), (2, Caller::Ask(1), 2));
    assert_eq!(
        serde_json::to_value(start).unwrap()["by"],
        json!("ask:1"),
        "the caller is spelled as in the token"
    );
    assert_eq!(snap.job.asks[1].depth, 2);
}

// ---- refused ---------------------------------------------------------------------------------

#[test]
fn an_ask_is_refused_unless_the_job_is_running_and_nothing_is_written() {
    for state in [Blocked, Verifying, Done, Failed, Cancelled] {
        let snap = running(state, &["a"]);
        assert_eq!(refused(&snap, &ask("a")), AskRefusal::TaskOver, "{state:?}");
    }
    for state in [Queued, Working] {
        assert!(transition(&running(state, &["a"]), &ask("a")).is_ok());
    }
}

#[test]
fn a_job_that_is_being_stopped_takes_no_ask() {
    let mut snap = running(Working, &["a"]);
    snap.job.after_stop = Some("never mind".into());
    assert_eq!(refused(&snap, &ask("a")), AskRefusal::TaskOver);
}

#[test]
fn only_an_agent_the_person_mentioned_in_this_job_may_be_asked() {
    let snap = running(Working, &["a", "b"]);
    let why = refused(&snap, &ask("c"));
    assert_eq!(
        why,
        AskRefusal::NotMentioned {
            agent: agent("c"),
            mentioned: vec![agent("a"), agent("b")]
        }
    );
    assert!(why.to_string().contains("(a, b)"), "{why}");
    // a job that mentioned nobody can ask nobody
    assert!(matches!(
        refused(&running(Working, &[]), &ask("a")),
        AskRefusal::NotMentioned { mentioned, .. } if mentioned.is_empty()
    ));
    assert_eq!(
        TransitionError::AskRefused(why).class(),
        ErrorClass::Invalid
    );
}

#[test]
fn an_agent_cannot_ask_itself_or_one_that_waits_for_it() {
    let snap = running(Working, &["b", "c", "d"]);
    let (snap, _) = step(&snap, &ask("b")); // ask 1: main -> b
    // b asks b: itself
    assert_eq!(
        refused(&snap, &ask_by(Caller::Ask(1), "b", "x")),
        AskRefusal::Cycle { agent: agent("b") }
    );
    let (snap, _) = step(&snap, &ask_by(Caller::Ask(1), "c", "x")); // ask 2: b -> c
    // c asks b: b waits for c
    assert_eq!(
        refused(&snap, &ask_by(Caller::Ask(2), "b", "x")),
        AskRefusal::Cycle { agent: agent("b") }
    );
    // c asks itself
    assert_eq!(
        refused(&snap, &ask_by(Caller::Ask(2), "c", "x")),
        AskRefusal::Cycle { agent: agent("c") }
    );
    // the addressed agent may ask b again (a second ask, not a cycle): it is the main caller
    assert!(transition(&snap, &ask("b")).is_ok());
}

#[test]
fn asks_nest_no_deeper_than_the_limit() {
    let snap = running(Working, &["b", "c", "d"]);
    let (snap, _) = step(&snap, &ask("b"));
    let (snap, _) = step(&snap, &ask_by(Caller::Ask(1), "c", "x"));
    // the default depth is 2: c (depth 2) may not ask d
    assert_eq!(
        refused(&snap, &ask_by(Caller::Ask(2), "d", "x")),
        AskRefusal::DepthReached { max: 2 }
    );
    // with depth 3 it may
    let deeper = AskLimits {
        depth: 3,
        ..AskLimits::default()
    };
    let (_, cmds) = step(
        &snap,
        &with_limits(ask_by(Caller::Ask(2), "d", "x"), deeper),
    );
    assert_eq!(started(&cmds)[0].depth, 3);
    // and with depth 1 not even the addressed agent's ask's child
    let shallow = AskLimits {
        depth: 1,
        ..AskLimits::default()
    };
    let fresh = running(Working, &["b", "c"]);
    let (fresh, _) = step(&fresh, &ask("b"));
    assert_eq!(
        refused(
            &fresh,
            &with_limits(ask_by(Caller::Ask(1), "c", "x"), shallow)
        ),
        AskRefusal::DepthReached { max: 1 }
    );
}

#[test]
fn a_job_makes_so_many_asks_and_those_that_ended_count() {
    let limits = AskLimits {
        per_job: 2,
        ..AskLimits::default()
    };
    let mut snap = running(Working, &["a"]);
    for n in 1..=2 {
        snap = step(&snap, &with_limits(ask("a"), limits)).0;
        snap = step(&snap, &finished(n, AskOutcome::Completed)).0;
    }
    assert_eq!(
        refused(&snap, &with_limits(ask("a"), limits)),
        AskRefusal::TooManyInJob { max: 2 }
    );
}

#[test]
fn only_so_many_asks_run_at_once_and_one_that_ends_makes_room() {
    let limits = AskLimits {
        running: 2,
        ..AskLimits::default()
    };
    let mut snap = running(Working, &["a", "b", "c"]);
    snap = step(&snap, &with_limits(ask("a"), limits)).0;
    snap = step(&snap, &with_limits(ask("b"), limits)).0;
    assert_eq!(
        refused(&snap, &with_limits(ask("c"), limits)),
        AskRefusal::TooManyRunning { max: 2 }
    );
    snap = step(&snap, &finished(1, AskOutcome::Completed)).0;
    assert!(transition(&snap, &with_limits(ask("c"), limits)).is_ok());
}

#[test]
fn the_asker_must_be_a_running_ask_of_this_job() {
    let snap = running(Working, &["a", "b"]);
    assert_eq!(
        refused(&snap, &ask_by(Caller::Ask(3), "a", "x")),
        AskRefusal::UnknownCaller { n: 3 }
    );
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, _) = step(&snap, &finished(1, AskOutcome::Completed));
    assert_eq!(
        refused(&snap, &ask_by(Caller::Ask(1), "b", "x")),
        AskRefusal::TaskOver,
        "its task is over"
    );
}

#[test]
fn a_question_has_text_of_a_bounded_size_and_a_call_has_a_key_of_one() {
    let snap = running(Working, &["a"]);
    assert_eq!(
        refused(&snap, &ask_by(Caller::Main, "a", " \n ")),
        AskRefusal::EmptyText
    );
    assert_eq!(
        refused(
            &snap,
            &ask_by(Caller::Main, "a", &"x".repeat(MAX_ASK_TEXT_BYTES + 1))
        ),
        AskRefusal::TextTooLong {
            max: MAX_ASK_TEXT_BYTES
        }
    );
    assert!(
        transition(
            &snap,
            &ask_by(Caller::Main, "a", &"x".repeat(MAX_ASK_TEXT_BYTES))
        )
        .is_ok()
    );
    assert_eq!(
        refused(
            &snap,
            &with_key(ask("a"), &"k".repeat(MAX_CALL_KEY_BYTES + 1))
        ),
        AskRefusal::CallKeyTooLong {
            max: MAX_CALL_KEY_BYTES
        }
    );
}

#[test]
fn the_same_call_again_is_the_same_ask_even_when_the_limits_are_spent_or_the_ask_is_over() {
    let limits = AskLimits {
        per_job: 1,
        ..AskLimits::default()
    };
    let snap = running(Working, &["a"]);
    let call = with_limits(with_key(ask("a"), "ask:t:main:c1"), limits);
    let (snap, cmds) = step(&snap, &call);
    assert_eq!(started(&cmds).len(), 1);
    // running: a repeat writes nothing
    let (again, cmds) = step(&snap, &call);
    assert!(cmds.is_empty());
    assert_eq!(again, snap);
    // over: still the same ask, and not the second the limit would refuse
    let (snap, _) = step(&snap, &finished(1, AskOutcome::Completed));
    let (again, cmds) = step(&snap, &call);
    assert!(cmds.is_empty());
    assert_eq!(again, snap);
    // another key is another call, and here the limit says no
    assert_eq!(
        refused(
            &snap,
            &with_limits(with_key(ask("a"), "ask:t:main:c2"), limits)
        ),
        AskRefusal::TooManyInJob { max: 1 }
    );
    // the same key from another caller is another call
    let snap = running(Working, &["a", "b"]);
    let (snap, _) = step(&snap, &with_key(ask("a"), "k"));
    let (snap, cmds) = step(&snap, &with_key(ask_by(Caller::Ask(1), "b", "x"), "k"));
    assert_eq!(started(&cmds).len(), 1);
    assert_eq!(snap.job.asks.len(), 2);
}

#[test]
fn the_same_call_key_for_another_agent_or_another_question_is_refused() {
    let snap = running(Working, &["a", "b"]);
    let (snap, _) = step(&snap, &with_key(ask("a"), "k"));
    // the same agent and the same words, whatever surrounds them, is the same call
    let (_, cmds) = step(
        &snap,
        &with_key(ask_by(Caller::Main, "a", "  find the data\n"), "k"),
    );
    assert!(cmds.is_empty());
    // another agent, or another question, is not
    assert_eq!(
        refused(&snap, &with_key(ask("b"), "k")),
        AskRefusal::CallKeyReused
    );
    assert_eq!(
        refused(
            &snap,
            &with_key(ask_by(Caller::Main, "a", "find other data"), "k")
        ),
        AskRefusal::CallKeyReused
    );
    assert_eq!(
        AskRefusal::CallKeyReused.to_string(),
        "this callId was used for another ask"
    );
    // nothing was written, and the ledger is as it was
    assert_eq!(snap.job.asks.len(), 1);
    // a ledger that predates the digest vouches for the agent alone
    let mut old = snap.clone();
    old.job.asks[0].fingerprint = None;
    let (_, cmds) = step(
        &old,
        &with_key(ask_by(Caller::Main, "a", "another words"), "k"),
    );
    assert!(cmds.is_empty());
    assert_eq!(
        refused(&old, &with_key(ask("b"), "k")),
        AskRefusal::CallKeyReused
    );
}

// ---- ending ----------------------------------------------------------------------------------

#[test]
fn the_asked_agents_answer_ends_the_ask_with_its_words_and_its_name() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let result = AskResult {
        outcome: AskOutcome::Completed,
        text: Some("the data".into()),
        question: None,
        artifacts: vec![AskArtifact {
            name: "table".into(),
            uri: Some("https://x/y".into()),
            mime_type: Some("text/csv".into()),
        }],
        error: None,
    };
    let (snap, cmds) = step(
        &snap,
        &Input::AskFinished {
            job: 1,
            ask: 1,
            revision: Some("r9".into()),
            result,
        },
    );
    assert_eq!(snap.state, Working, "the job goes on");
    assert_eq!(outcomes(&snap), [(1, Some(AskOutcome::Completed))]);
    let [(actor, data)] = finishes(&cmds)[..] else {
        panic!("one ask_finished");
    };
    assert_eq!(actor, &Actor::agent(&agent("a"), Some("r9".into())));
    assert_eq!(
        serde_json::to_value(data).unwrap(),
        json!({"ask": 1, "state": "completed", "text": "the data",
               "artifacts": [{"name": "table", "uri": "https://x/y", "mimeType": "text/csv"}]})
    );
}

#[test]
fn every_way_the_asked_agent_ends_is_one_ask_finished_with_its_state() {
    for outcome in [
        AskOutcome::Completed,
        AskOutcome::InputRequired,
        AskOutcome::AuthRequired,
        AskOutcome::Failed,
        AskOutcome::Rejected,
        AskOutcome::Canceled,
    ] {
        let snap = running(Working, &["a"]);
        let (snap, _) = step(&snap, &ask("a"));
        let (snap, cmds) = step(&snap, &finished(1, outcome));
        assert_eq!(snap.job.asks[0].outcome, Some(outcome));
        assert_eq!(finish_of(&cmds, 1).state, outcome);
        assert_eq!(finishes(&cmds).len(), 1);
    }
}

#[test]
fn an_ask_ends_exactly_once_and_a_late_input_is_dropped() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (ended, cmds) = step(&snap, &finished(1, AskOutcome::Completed));
    assert_eq!(finishes(&cmds).len(), 1);
    for late in [
        finished(1, AskOutcome::Failed),
        Input::AskFailed {
            job: 1,
            ask: 1,
            reason: "gone".into(),
        },
        timer(1, 1),
        cancel(),
    ] {
        let (same, cmds) = step(&ended, &late);
        assert!(finishes(&cmds).is_empty(), "{late:?}");
        assert_eq!(outcomes(&same), [(1, Some(AskOutcome::Completed))]);
    }
    // an ask the job does not have is as late
    let (same, cmds) = step(&ended, &finished(9, AskOutcome::Completed));
    assert!(cmds.is_empty());
    assert_eq!(same, ended);
    let (same, cmds) = step(
        &ended,
        &Input::AskFailed {
            job: 1,
            ask: 9,
            reason: "x".into(),
        },
    );
    assert!(cmds.is_empty());
    assert_eq!(same, ended);
}

#[test]
fn a_long_answer_is_cut_and_a_long_list_of_artifacts_is_capped() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let artifact = |n: usize| AskArtifact {
        name: format!("f{n}"),
        uri: None,
        mime_type: None,
    };
    let result = AskResult {
        outcome: AskOutcome::InputRequired,
        text: Some("é".repeat(MAX_ASK_ANSWER_BYTES)),
        question: Some("q".repeat(MAX_ASK_NOTE_BYTES + 10)),
        artifacts: (0..MAX_ASK_ARTIFACTS + 5).map(artifact).collect(),
        error: Some("e".repeat(MAX_ASK_NOTE_BYTES + 10)),
    };
    let (_, cmds) = step(
        &snap,
        &Input::AskFinished {
            job: 1,
            ask: 1,
            revision: None,
            result,
        },
    );
    let data = finish_of(&cmds, 1);
    let text = data.text.as_ref().unwrap();
    assert!(text.len() <= MAX_ASK_ANSWER_BYTES && text.len() > MAX_ASK_ANSWER_BYTES - 4);
    assert_eq!(data.question.as_ref().unwrap().len(), MAX_ASK_NOTE_BYTES);
    assert_eq!(data.error.as_ref().unwrap().len(), MAX_ASK_NOTE_BYTES);
    assert_eq!(data.artifacts.len(), MAX_ASK_ARTIFACTS);
}

#[test]
fn an_agent_that_cannot_be_had_ends_the_ask_failed_in_the_orchestrators_name() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, cmds) = step(
        &snap,
        &Input::AskFailed {
            job: 1,
            ask: 1,
            reason: " 'a' is not listed any more ".into(),
        },
    );
    assert_eq!(outcomes(&snap), [(1, Some(AskOutcome::Failed))]);
    let [(actor, data)] = finishes(&cmds)[..] else {
        panic!("one ask_finished");
    };
    assert_eq!(actor, &Actor::system());
    assert_eq!(
        serde_json::to_value(data).unwrap(),
        json!({"ask": 1, "state": "failed", "error": "'a' is not listed any more"})
    );
}

#[test]
fn the_deadline_ends_a_running_ask_timed_out_and_is_stale_for_anything_else() {
    let snap = running(Working, &["a", "b"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, _) = step(&snap, &ask("b"));
    let (next, cmds) = step(&snap, &timer(1, 2));
    assert_eq!(
        outcomes(&next),
        [(1, None), (2, Some(AskOutcome::TimedOut))],
        "only the one whose deadline it is"
    );
    let [(actor, data)] = finishes(&cmds)[..] else {
        panic!("one ask_finished");
    };
    assert_eq!(actor, &Actor::system());
    assert_eq!(
        serde_json::to_value(data).unwrap(),
        json!({"ask": 2, "state": "timed_out", "error": "the asked agent did not answer in time"})
    );
    assert_eq!(next.state, Working, "the job is not the ask's to end");
    // a deadline of another job, or of an ask that is not there, changes nothing
    for stale in [timer(2, 1), timer(1, 7)] {
        let (same, cmds) = step(&snap, &stale);
        assert!(cmds.is_empty());
        assert_eq!(same, snap);
    }
    // it ends the ask while the job waits for the person too, which is not the ask's own wait
    let mut blocked = snap.clone();
    blocked.state = Blocked;
    let (next, cmds) = step(&blocked, &timer(1, 1));
    assert_eq!(finish_of(&cmds, 1).state, AskOutcome::TimedOut);
    assert_eq!(next.state, Blocked);
}

#[test]
fn the_persons_cancel_ends_every_running_ask_canceled() {
    for state in [Queued, Working, Blocked] {
        let snap = running(Working, &["a", "b"]);
        let (mut snap, _) = step(&snap, &ask("a"));
        let (next, _) = step(&snap, &ask("b"));
        snap = Snapshot { state, ..next };
        let (next, cmds) = step(&snap, &cancel());
        assert_eq!(
            outcomes(&next),
            [
                (1, Some(AskOutcome::Canceled)),
                (2, Some(AskOutcome::Canceled))
            ],
            "{state:?}"
        );
        for n in [1, 2] {
            let data = finish_of(&cmds, n);
            assert_eq!(data.error.as_deref(), Some("the person stopped the job"));
        }
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::RequestCancel { .. })),
            "the thread's own cancel is still asked for"
        );
        assert!(finishes(&cmds).iter().all(|(a, _)| **a == Actor::system()));
        // and a second cancel finds none running
        let (_, cmds) = step(&next, &cancel());
        assert!(finishes(&cmds).is_empty());
    }
}

#[test]
fn a_cancel_of_a_thread_that_is_being_verified_has_no_ask_left_to_end() {
    // the asks ended when the task did; the cancel ends a thread that was already past them
    let gate = GatePolicy {
        verifier: Some(agent("v")),
        ..GatePolicy::requiring([CheckSource::Verifier])
    };
    let snap = Snapshot {
        state: Working,
        job: Job {
            mentioned: set(&["a"]),
            pushed: Some(pushed()),
            ..Job::with_gate(gate)
        },
    };
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, cmds) = step(&snap, &status(AgentTaskState::Completed));
    assert_eq!(snap.state, Verifying);
    assert_eq!(finish_of(&cmds, 1).state, AskOutcome::Canceled);
    let (next, cmds) = step(&snap, &cancel());
    assert_eq!(next.state, Cancelled);
    assert!(finishes(&cmds).is_empty());
}

#[test]
fn the_end_of_the_asking_task_ends_every_ask_canceled_before_the_threads_own_event() {
    for (task, ends) in [
        (AgentTaskState::Completed, Done),
        (AgentTaskState::Failed, Failed),
        (AgentTaskState::Rejected, Failed),
        (AgentTaskState::Canceled, Cancelled),
    ] {
        let snap = running(Working, &["a", "b"]);
        let (snap, _) = step(&snap, &ask("a"));
        let (snap, _) = step(&snap, &ask("b"));
        let (next, cmds) = step(&snap, &status(task));
        assert_eq!(next.state, ends);
        assert!(next.job.asks.iter().all(|a| !a.is_running()), "{task:?}");
        for n in [1, 2] {
            let data = finish_of(&cmds, n);
            assert_eq!(data.state, AskOutcome::Canceled);
            assert_eq!(data.error.as_deref(), Some("the asking task ended"));
        }
        let at = index_of_thread_state(&cmds).expect("a thread_state");
        let last_ask = cmds
            .iter()
            .rposition(|c| {
                matches!(
                    c,
                    Command::Append(EventDraft {
                        body: EventBody::AskFinished(_),
                        ..
                    })
                )
            })
            .unwrap();
        assert!(
            last_ask < at,
            "the asks end before the thread does: {cmds:?}"
        );
    }
}

#[test]
fn a_delivery_that_fails_for_good_and_a_cancel_before_the_start_end_the_asks_too() {
    for end in [
        Input::DeliveryFailed {
            reason: "nope".into(),
            retryable: false,
        },
        Input::CancelledBeforeStart,
    ] {
        let snap = running(Working, &["a"]);
        let (snap, _) = step(&snap, &ask("a"));
        let (next, cmds) = step(&snap, &end);
        assert!(next.state.is_terminal());
        assert_eq!(finish_of(&cmds, 1).state, AskOutcome::Canceled);
    }
}

#[test]
fn a_job_that_waits_for_the_person_keeps_its_asks_and_the_answer_goes_on_with_them() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (blocked, cmds) = step(&snap, &status(AgentTaskState::InputRequired));
    assert_eq!(blocked.state, Blocked);
    assert!(finishes(&cmds).is_empty(), "the task has not ended");
    assert_eq!(blocked.job.asks[0].outcome, None);
    // a blocked job takes no new ask
    assert_eq!(refused(&blocked, &ask("a")), AskRefusal::TaskOver);
}

#[test]
fn stop_and_send_ends_the_asks_of_the_job_it_replaces_and_the_next_job_has_none() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let stop = Input::StopAndSend {
        user: user(),
        text: "do this instead".into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: vec![],
    };
    let (stopping, cmds) = step(&snap, &stop);
    assert_eq!(
        finish_of(&cmds, 1).error.as_deref(),
        Some("the person stopped the job")
    );
    assert!(stopping.job.asks.iter().all(|a| !a.is_running()));
    assert_eq!(refused(&stopping, &ask("a")), AskRefusal::TaskOver);
    // the replaced task ends: the next job starts with no ledger
    let (next, _) = step(&stopping, &status(AgentTaskState::Canceled));
    assert_eq!(next.job.number, 2);
    assert!(next.job.asks.is_empty());
}

#[test]
fn a_new_job_starts_a_ledger_of_its_own_and_an_old_deadline_is_stale_in_it() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (done, _) = step(&snap, &status(AgentTaskState::Completed));
    let (next, _) = step(
        &done,
        &Input::UserMessage {
            user: user(),
            text: "again, with @a".into(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: vec![Mention {
                agent_id: agent("a"),
                label: "@a".into(),
                start: 12,
                end: 14,
                card_url: None,
            }],
        },
    );
    assert_eq!((next.job.number, next.job.asks.len()), (2, 0));
    let (next, _) = step(&next, &ask("a"));
    assert_eq!(next.job.asks[0].n, 1, "numbered from 1 again");
    // the first job's deadline for its ask 1 comes now: it is not this job's ask 1
    let (same, cmds) = step(&next, &timer(1, 1));
    assert!(cmds.is_empty());
    assert_eq!(same, next);
    let (_, cmds) = step(&next, &timer(2, 1));
    assert_eq!(finish_of(&cmds, 1).state, AskOutcome::TimedOut);
}

#[test]
fn what_an_earlier_jobs_ask_reports_is_not_the_later_jobs_ask() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (done, _) = step(&snap, &status(AgentTaskState::Completed));
    let (next, _) = step(
        &done,
        &Input::UserMessage {
            user: user(),
            text: "again, with @a".into(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: vec![Mention {
                agent_id: agent("a"),
                label: "@a".into(),
                start: 12,
                end: 14,
                card_url: None,
            }],
        },
    );
    let (next, _) = step(&next, &ask("a"));
    assert_eq!((next.job.number, next.job.asks[0].n), (2, 1));
    // the first job's ask 1 reports its result, its task and its failure: job 2's ask 1 is
    // another ask, and none of it touches it
    for late in [
        Input::AskFinished {
            job: 1,
            ask: 1,
            revision: None,
            result: AskResult::of(AskOutcome::Completed),
        },
        Input::AskFailed {
            job: 1,
            ask: 1,
            reason: "gone".into(),
        },
        Input::AskSent {
            job: 1,
            ask: 1,
            task_id: "t-old".into(),
        },
    ] {
        let (same, cmds) = step(&next, &late);
        assert!(cmds.is_empty(), "{late:?}");
        assert_eq!(same, next, "{late:?}");
    }
    // the same reports for job 2 are heard
    let (heard, cmds) = step(
        &next,
        &Input::AskFinished {
            job: 2,
            ask: 1,
            revision: None,
            result: AskResult::of(AskOutcome::Completed),
        },
    );
    assert_eq!(finish_of(&cmds, 1).state, AskOutcome::Completed);
    assert_eq!(outcomes(&heard), [(1, Some(AskOutcome::Completed))]);
}

#[test]
fn an_ask_that_ends_ends_the_asks_it_asked() {
    let snap = running(Working, &["b", "c", "d"]);
    let (snap, _) = step(&snap, &ask("b")); // 1: main -> b
    let (snap, _) = step(&snap, &ask_by(Caller::Ask(1), "c", "x")); // 2: b -> c
    let deeper = AskLimits {
        depth: 3,
        ..AskLimits::default()
    };
    let (snap, _) = step(
        &snap,
        &with_limits(ask_by(Caller::Ask(2), "d", "x"), deeper),
    ); // 3: c -> d
    let (snap, _) = step(&snap, &ask("c")); // 4: main -> c, not under 1
    let (next, cmds) = step(&snap, &finished(1, AskOutcome::Completed));
    assert_eq!(
        outcomes(&next),
        [
            (1, Some(AskOutcome::Completed)),
            (2, Some(AskOutcome::Canceled)),
            (3, Some(AskOutcome::Canceled)),
            (4, None)
        ]
    );
    let order: Vec<u32> = finishes(&cmds).iter().map(|(_, d)| d.ask).collect();
    assert_eq!(order, [1, 2, 3], "the cause first");
    assert_eq!(
        finish_of(&cmds, 3).error.as_deref(),
        Some("the asking task ended")
    );
    // the same for a deadline
    let (next, cmds) = step(&snap, &timer(1, 1));
    assert_eq!(finishes(&cmds).len(), 3);
    assert_eq!(next.job.asks[3].outcome, None);
}

// ---- continuing ----------------------------------------------------------------------------

fn sent(ask: u32, task: &str) -> Input {
    Input::AskSent {
        job: 1,
        ask,
        task_id: task.into(),
    }
}
fn row_of(cmds: &[Command]) -> (&Option<String>, &[String]) {
    cmds.iter()
        .find_map(|c| match c {
            Command::Ask {
                continue_task,
                reference_task_ids,
                ..
            } => Some((continue_task, reference_task_ids.as_slice())),
            _ => None,
        })
        .expect("an ask row")
}

#[test]
fn a_task_is_recorded_once_while_the_ask_runs_and_writes_no_event() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, cmds) = step(&snap, &sent(1, "t-1"));
    assert!(cmds.is_empty());
    assert_eq!(snap.job.asks[0].task_id.as_deref(), Some("t-1"));
    let (snap, _) = step(&snap, &sent(1, "t-2"));
    assert_eq!(
        snap.job.asks[0].task_id.as_deref(),
        Some("t-1"),
        "first wins"
    );
    let (snap, _) = step(&snap, &sent(7, "t-9"));
    assert_eq!(snap.job.asks.len(), 1);
    let (ended, _) = step(&snap, &finished(1, AskOutcome::Completed));
    let (ended, _) = step(&ended, &sent(1, "t-3"));
    assert_eq!(ended.job.asks[0].task_id.as_deref(), Some("t-1"));
}

#[test]
fn asking_an_agent_that_asked_back_continues_its_task_else_refers_to_the_earlier_ones() {
    let snap = running(Working, &["a", "b"]);
    // ask 1 to a: its task t-1, ends waiting for an answer
    let (snap, cmds) = step(&snap, &ask("a"));
    assert_eq!(
        row_of(&cmds),
        (&None, &[][..]),
        "the first ask is a new task"
    );
    let (snap, _) = step(&snap, &sent(1, "t-1"));
    let (snap, _) = step(
        &snap,
        &Input::AskFinished {
            job: 1,
            ask: 1,
            revision: None,
            result: AskResult {
                question: Some("which years?".into()),
                ..AskResult::of(AskOutcome::InputRequired)
            },
        },
    );
    // asking a again answers it: the same task
    let (snap, cmds) = step(&snap, &ask("a"));
    assert_eq!(row_of(&cmds), (&Some("t-1".to_owned()), &[][..]));
    // asking b is a new task
    let (snap, cmds) = step(&snap, &ask("b"));
    assert_eq!(row_of(&cmds), (&None, &[][..]));
    // ask 2 (to a) ends completed with a task of its own; the next ask of a is a new task that
    // refers to both
    let (snap, _) = step(&snap, &sent(2, "t-2"));
    let (snap, _) = step(&snap, &finished(2, AskOutcome::Completed));
    let (_, cmds) = step(&snap, &ask("a"));
    assert_eq!(
        row_of(&cmds),
        (&None, &["t-1".to_owned(), "t-2".to_owned()][..])
    );
}

#[test]
fn a_task_that_several_asks_continued_is_referred_to_once() {
    let snap = running(Working, &["a"]);
    // ask 1 ends waiting; ask 2 continues the same task and completes it
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, _) = step(&snap, &sent(1, "t-1"));
    let (snap, _) = step(&snap, &finished(1, AskOutcome::InputRequired));
    let (snap, cmds) = step(&snap, &ask("a"));
    assert_eq!(row_of(&cmds), (&Some("t-1".to_owned()), &[][..]));
    let (snap, _) = step(&snap, &sent(2, "t-1"));
    let (snap, _) = step(&snap, &finished(2, AskOutcome::Completed));
    let (_, cmds) = step(&snap, &ask("a"));
    assert_eq!(row_of(&cmds), (&None, &["t-1".to_owned()][..]));
}

#[test]
fn an_input_required_ask_with_no_recorded_task_cannot_be_continued() {
    let snap = running(Working, &["a"]);
    let (snap, _) = step(&snap, &ask("a"));
    let (snap, _) = step(&snap, &finished(1, AskOutcome::AuthRequired));
    let (_, cmds) = step(&snap, &ask("a"));
    assert_eq!(row_of(&cmds), (&None, &[][..]));
}

#[test]
fn only_the_latest_earlier_tasks_are_referred_to() {
    let mut snap = running(Working, &["a"]);
    let limits = AskLimits {
        per_job: 64,
        ..AskLimits::default()
    };
    for n in 1..=(MAX_ASK_REFERENCES as u32 + 3) {
        snap = step(&snap, &with_limits(ask("a"), limits)).0;
        snap = step(&snap, &sent(n, &format!("t-{n}"))).0;
        snap = step(&snap, &finished(n, AskOutcome::Completed)).0;
    }
    let (_, cmds) = step(&snap, &with_limits(ask("a"), limits));
    let (_, refs) = row_of(&cmds);
    assert_eq!(refs.len(), MAX_ASK_REFERENCES);
    assert_eq!(
        refs.last().unwrap(),
        &format!("t-{}", MAX_ASK_REFERENCES + 3)
    );
    assert_eq!(refs.first().unwrap(), "t-4");
}

// ---- the wire ------------------------------------------------------------------------------

#[test]
fn the_events_and_the_ledger_read_back_and_an_old_ledger_has_no_asks() {
    let snap = running(Working, &["a"]);
    let (snap, cmds) = step(&snap, &ask("a"));
    for cmd in &cmds {
        if let Command::Append(draft) = cmd {
            let event = Event {
                seq: 1,
                thread_id: ThreadId(uuid::Uuid::nil()),
                at: Timestamp::UNIX_EPOCH,
                actor: draft.actor.clone(),
                body: draft.body.clone(),
            };
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["kind"], "ask_started");
            assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
        }
    }
    let (snap, cmds) = step(&snap, &finished(1, AskOutcome::TimedOut));
    let Some(Command::Append(draft)) = cmds.first() else {
        panic!("an event");
    };
    let event = Event {
        seq: 2,
        thread_id: ThreadId(uuid::Uuid::nil()),
        at: Timestamp::UNIX_EPOCH,
        actor: draft.actor.clone(),
        body: draft.body.clone(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "ask_finished");
    assert_eq!(json["data"]["state"], "timed_out");
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
    assert_eq!(EventKind::AskStarted.as_str(), "ask_started");
    assert_eq!(EventKind::AskFinished.as_str(), "ask_finished");

    // the ledger round-trips through the job's JSON, and a job without asks does not write the key
    let job = serde_json::to_value(&snap.job).unwrap();
    assert_eq!(
        job["asks"],
        json!([{"n": 1, "by": "main", "agent": "a", "depth": 1, "outcome": "timed_out"}])
    );
    assert_eq!(serde_json::from_value::<Job>(job).unwrap(), snap.job);
    let empty = serde_json::to_value(Job::default()).unwrap();
    assert!(empty.get("asks").is_none());
    let old: Job = serde_json::from_value(json!({"number": 3})).unwrap();
    assert!(old.asks.is_empty());
}

#[test]
fn the_deadline_timer_has_one_spelling() {
    let json = serde_json::to_value(Timer::AskDeadline { job: 2, ask: 3 }).unwrap();
    assert_eq!(json, json!({"kind": "ask_deadline", "job": 2, "ask": 3}));
    assert_eq!(
        serde_json::from_value::<Timer>(json).unwrap(),
        Timer::AskDeadline { job: 2, ask: 3 }
    );
}

// ---- properties ----------------------------------------------------------------------------

const AGENTS: [&str; 4] = ["a", "b", "c", "d"];

fn arb_caller() -> impl Strategy<Value = Caller> {
    prop_oneof![
        3 => Just(Caller::Main),
        2 => (1..=6_u32).prop_map(Caller::Ask),
    ]
}

fn arb_limits() -> impl Strategy<Value = AskLimits> {
    (1..=3_u8, 1..=6_u32, 1..=3_u32, 1..=3600_i64).prop_map(|(depth, per_job, running, secs)| {
        AskLimits {
            depth,
            per_job,
            running,
            timeout: SignedDuration::from_secs(secs),
        }
    })
}

fn arb_outcome() -> impl Strategy<Value = AskOutcome> {
    prop_oneof![
        Just(AskOutcome::Completed),
        Just(AskOutcome::InputRequired),
        Just(AskOutcome::AuthRequired),
        Just(AskOutcome::Failed),
        Just(AskOutcome::Rejected),
        Just(AskOutcome::Canceled),
        Just(AskOutcome::TimedOut),
    ]
}

fn arb_task_state() -> impl Strategy<Value = AgentTaskState> {
    prop_oneof![
        1 => Just(AgentTaskState::Submitted),
        4 => Just(AgentTaskState::Working),
        1 => Just(AgentTaskState::InputRequired),
        1 => Just(AgentTaskState::AuthRequired),
        1 => Just(AgentTaskState::Completed),
        1 => Just(AgentTaskState::Failed),
        1 => Just(AgentTaskState::Canceled),
        1 => Just(AgentTaskState::Rejected),
    ]
}

fn arb_input() -> impl Strategy<Value = Input> {
    let agent_name = || prop::sample::select(AGENTS.to_vec());
    prop_oneof![
        8 => (arb_caller(), agent_name(), arb_limits(), proptest::option::of(0..4_u8)).prop_map(
            |(caller, to, limits, key)| Input::Ask {
                actor: coder(),
                caller,
                agent: agent(to),
                text: "do it".into(),
                call_key: key.map(|k| format!("k{k}")),
                parent_step: None,
                limits,
            }
        ),
        4 => (1..=7_u32, arb_outcome()).prop_map(|(ask, outcome)| finished(ask, outcome)),
        1 => (1..=7_u32).prop_map(|ask| Input::AskFailed { job: 1, ask, reason: "gone".into() }),
        1 => (1..=7_u32).prop_map(|ask| sent(ask, &format!("t{ask}"))),
        3 => (1..=3_u32, 1..=7_u32).prop_map(|(job, ask)| timer(job, ask)),
        3 => arb_task_state().prop_map(status),
        2 => Just(cancel()),
        2 => prop::collection::vec(agent_name(), 0..3).prop_map(|names| {
            let mentions = names
                .iter()
                .map(|n| Mention {
                    agent_id: agent(n),
                    label: format!("@{n}"),
                    start: 0,
                    end: 2,
                    card_url: None,
                })
                .collect();
            Input::UserMessage {
                user: user(),
                text: "go".into(),
                message_id: None,
                run_id: None,
                origin: Origin::Agui,
                catalog: None,
                mentions,
            }
        }),
        1 => agent_name().prop_map(|n| Input::StopAndSend {
            user: user(),
            text: "instead".into(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: vec![Mention {
                agent_id: agent(n),
                label: format!("@{n}"),
                start: 0,
                end: 2,
                card_url: None,
            }],
        }),
        1 => (any::<bool>(), "[a-z]{1,4}").prop_map(|(retryable, reason)|
            Input::DeliveryFailed { reason, retryable }),
        1 => Just(Input::CancelledBeforeStart),
        1 => Just(Input::Redeliver { text: "again".into(), sent: false, mentions: vec![] }),
    ]
}

fn arb_gate() -> impl Strategy<Value = GatePolicy> {
    prop_oneof![
        Just(GatePolicy::default()),
        Just(GatePolicy {
            verifier: Some(agent("v")),
            ..GatePolicy::requiring([CheckSource::Verifier])
        }),
    ]
}

/// What the log of one job says of its asks, built from the events the transitions produce.
#[derive(Default)]
struct Told {
    started: BTreeSet<u32>,
    finished: BTreeMap<u32, u32>,
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Whatever the order of asks, answers, deadlines, cancels, stops and the agent's own
    /// statuses: an ask ends once and none runs after its job's task has ended, the limits
    /// the inputs carried hold, and the log says what the ledger says.
    #[test]
    fn an_ask_always_ends_once_and_none_runs_after_its_job_is_over(
        gate in arb_gate(),
        mentions in prop::collection::vec(prop::sample::select(AGENTS.to_vec()), 1..4),
        inputs in prop::collection::vec(arb_input(), 0..80),
    ) {
        let first = Input::UserMessage {
            user: user(),
            text: "go".into(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: mentions
                .iter()
                .map(|n| Mention {
                    agent_id: agent(n),
                    label: format!("@{n}"),
                    start: 0,
                    end: 2,
                    card_url: None,
                })
                .collect(),
        };
        let (mut snap, _) = start_thread(gate, &first).unwrap();
        if snap.job.gate.is_active() {
            snap.job.pushed = Some(pushed());
        }
        let mut told = Told::default();
        let mut job_number = snap.job.number;
        for input in &inputs {
            let Ok((next, cmds)) = transition(&snap, input) else { continue };
            if next.job.number != job_number {
                // a new job: the ledger and the tale start again
                job_number = next.job.number;
                told = Told::default();
            }
            for cmd in &cmds {
                match cmd {
                    Command::Append(EventDraft { body: EventBody::AskStarted(d), .. }) => {
                        prop_assert!(told.started.insert(d.ask), "ask {} started twice", d.ask);
                    }
                    Command::Append(EventDraft { body: EventBody::AskFinished(d), .. }) => {
                        *told.finished.entry(d.ask).or_default() += 1;
                    }
                    _ => {}
                }
            }
            snap = next;

            // each ask ends exactly once: the log and the ledger agree on it
            for (ask, times) in &told.finished {
                prop_assert_eq!(*times, 1, "ask {} ended {} times", ask, times);
            }
            for a in &snap.job.asks {
                prop_assert!(told.started.contains(&a.n), "ask {} was never started", a.n);
                prop_assert_eq!(
                    told.finished.contains_key(&a.n),
                    !a.is_running(),
                    "ask {}: the log and the ledger disagree on whether it ended", a.n
                );
            }
            prop_assert_eq!(snap.job.asks.len(), told.started.len());
            prop_assert!(
                snap.job.asks.iter().enumerate().all(|(i, a)| a.n as usize == i + 1),
                "numbers are 1.. in order"
            );

            // none runs once the job's task has ended, or while it is being replaced
            let task_over = matches!(
                snap.state,
                Verifying | Done | Failed | Cancelled
            );
            let running = snap.job.asks.iter().filter(|a| a.is_running()).count();
            if task_over || snap.job.after_stop.is_some() {
                prop_assert_eq!(running, 0, "an ask runs in {:?}", snap.state);
            }

            // only mentioned agents were asked, and nobody waits for itself
            for a in &snap.job.asks {
                prop_assert!(snap.job.mentioned.contains(&a.agent));
                if let Caller::Ask(m) = a.by {
                    let parent = snap.job.asks.iter().find(|p| p.n == m).unwrap();
                    prop_assert!(m < a.n, "children are numbered after their parent");
                    prop_assert_eq!(a.depth, parent.depth + 1);
                    prop_assert_ne!(&a.agent, &parent.agent);
                } else {
                    prop_assert_eq!(a.depth, 1);
                }
            }
            // a child does not run under a parent that ended
            for a in snap.job.asks.iter().filter(|a| a.is_running()) {
                if let Caller::Ask(m) = a.by {
                    let parent = snap.job.asks.iter().find(|p| p.n == m).unwrap();
                    prop_assert!(parent.is_running(), "ask {} runs under an ended ask", a.n);
                }
            }
        }
    }

    /// The limits an input carries are the ones it is held to: no accepted ask goes over them.
    #[test]
    fn an_accepted_ask_never_goes_over_the_limits_it_carried(
        limits in arb_limits(),
        steps in prop::collection::vec((arb_caller(), prop::sample::select(AGENTS.to_vec()), arb_outcome(), any::<bool>()), 0..40),
    ) {
        let mut snap = running(Working, &AGENTS);
        for (caller, to, outcome, end_one) in steps {
            let input = with_limits(ask_by(caller, to, "x"), limits);
            if let Ok((next, _)) = transition(&snap, &input) {
                snap = next;
            }
            let asked = snap.job.asks.len() as u32;
            let running_now = snap.job.asks.iter().filter(|a| a.is_running()).count() as u32;
            prop_assert!(asked <= limits.per_job);
            prop_assert!(running_now <= limits.running);
            prop_assert!(snap.job.asks.iter().all(|a| a.depth <= limits.depth));
            if end_one && let Some(first) = snap.job.asks.iter().find(|a| a.is_running()) {
                snap = transition(&snap, &finished(first.n, outcome)).unwrap().0;
            }
        }
    }
}
