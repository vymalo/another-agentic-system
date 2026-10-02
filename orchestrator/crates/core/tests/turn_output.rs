//! The turn's announced answer (`turn_output`, ADR 0031): `Input::Answer`, and what the rest of
//! the turn's words become once an answer is announced.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};

fn agent() -> AgentId {
    AgentId::new("coder")
}

fn actor() -> Actor {
    Actor::agent(&agent(), None)
}

fn answer(text: &str, job: u32, token: &str) -> Input {
    Input::Answer {
        actor: actor(),
        text: text.into(),
        job,
        token: token.into(),
    }
}

fn status(state: AgentTaskState, detail: Option<&str>) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Status {
            state,
            detail: detail.map(Into::into),
        },
    }
}

fn message(id: &str, text: &str, purpose: Option<MessagePurpose>) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Message {
            message_id: id.into(),
            text: text.into(),
            is_final: true,
            purpose,
        },
    }
}

fn user_says(text: &str) -> Input {
    Input::UserMessage {
        user: UserId::new("me@example.com"),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
    }
}

/// A thread in `state`, on its first job.
fn at(state: ThreadState) -> Snapshot {
    Snapshot::new(state)
}

fn step(snapshot: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snapshot, input).unwrap()
}

fn bodies(cmds: &[Command]) -> Vec<&EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(&d.body),
            _ => None,
        })
        .collect()
}

fn messages(cmds: &[Command]) -> Vec<&AgentMessageData> {
    bodies(cmds)
        .into_iter()
        .filter_map(|b| match b {
            EventBody::AgentMessage(m) => Some(m),
            _ => None,
        })
        .collect()
}

#[test]
fn an_announced_answer_is_a_final_message_marked_answer_via_turn_output() {
    for state in [Queued, Working] {
        let (next, cmds) = step(&at(state), &answer("The result.", 1, "jti-1"));
        assert_eq!(next.state, state, "an announcement moves nothing");
        assert!(next.job.answer.is_announced());
        assert_eq!(
            bodies(&cmds),
            [&EventBody::AgentMessage(AgentMessageData {
                text: "The result.".into(),
                message_id: "out-jti-1-1".into(),
                is_final: true,
                purpose: Some(MessagePurpose::Answer),
                via: Some(AnswerVia::TurnOutput),
            })]
        );
        match &cmds[0] {
            Command::Append(d) => assert_eq!(d.actor, actor()),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_turn_is_over_unless_the_thread_is_queued_or_working() {
    for state in [Blocked, Verifying, Done, Failed, Cancelled] {
        let err = transition(&at(state), &answer("late", 1, "jti-1")).unwrap_err();
        assert!(
            matches!(
                err,
                TransitionError::InvalidInState {
                    input: "answer",
                    ..
                }
            ),
            "{state:?}: {err:?}"
        );
    }
}

#[test]
fn a_token_of_an_earlier_job_announces_nothing() {
    let mut snapshot = at(Working);
    snapshot.job.number = 3;
    assert!(transition(&snapshot, &answer("stale", 2, "jti-old")).is_err());
    assert!(transition(&snapshot, &answer("ahead", 4, "jti-new")).is_err());
    assert!(transition(&snapshot, &answer("current", 3, "jti-3")).is_ok());
}

#[test]
fn an_empty_or_oversize_answer_is_not_recorded() {
    for text in ["", "  \n ", &"x".repeat(MAX_ANSWER_BYTES + 1)] {
        assert!(transition(&at(Working), &answer(text, 1, "j")).is_err());
    }
}

#[test]
fn a_later_announcement_is_another_message_with_the_next_number() {
    let (snapshot, first) = step(&at(Working), &answer("Draft.", 1, "j"));
    let (snapshot, second) = step(&snapshot, &answer("Final.", 1, "j"));
    assert_eq!(messages(&first)[0].message_id, "out-j-1");
    assert_eq!(messages(&second)[0].message_id, "out-j-2");
    // Both say `answer`, `turn_output`: the log is append-only, the later one is the answer by the
    // rule (the last message marked answer in the turn), and the earlier one is working text.
    for m in messages(&first).into_iter().chain(messages(&second)) {
        assert_eq!(m.purpose, Some(MessagePurpose::Answer));
        assert_eq!(m.via, Some(AnswerVia::TurnOutput));
    }
    assert!(snapshot.job.answer.is_announced());
}

#[test]
fn a_token_that_did_not_announce_is_a_turn_that_is_over_once_another_has() {
    let (snapshot, _) = step(&at(Working), &answer("Mine.", 1, "j-new"));
    assert!(transition(&snapshot, &answer("Not mine.", 1, "j-old")).is_err());
    assert!(transition(&snapshot, &answer("Mine again.", 1, "j-new")).is_ok());
}

#[test]
fn the_words_that_end_the_turn_are_working_text_after_an_announcement() {
    for ending in [
        AgentTaskState::Completed,
        AgentTaskState::InputRequired,
        AgentTaskState::AuthRequired,
    ] {
        let (snapshot, _) = step(&at(Working), &answer("The result.", 1, "j"));
        // the adapter's pair for a stated stream on the turn-ending status: the message, then the
        // status that keeps the same words as its detail
        let (snapshot, said) = step(
            &snapshot,
            &message("s-1", "All done, see above.", Some(MessagePurpose::Answer)),
        );
        let m = messages(&said);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].purpose, Some(MessagePurpose::Working), "{ending:?}");
        assert_eq!(m[0].text, "All done, see above.");
        assert_eq!(m[0].via, None);

        let (_, cmds) = step(&snapshot, &status(ending, Some("All done, see above.")));
        // the words were said as the message just before: not again, and the status keeps them
        assert!(messages(&cmds).is_empty(), "{ending:?}: {cmds:?}");
        assert!(bodies(&cmds).iter().any(|b| matches!(
            b,
            EventBody::AgentStatus(s) if s.detail.as_deref() == Some("All done, see above.")
        )));
    }
}

#[test]
fn status_words_nobody_said_are_said_as_working_text_ahead_of_the_status() {
    for ending in [
        AgentTaskState::Completed,
        AgentTaskState::InputRequired,
        AgentTaskState::AuthRequired,
    ] {
        let (snapshot, _) = step(&at(Working), &answer("The result.", 1, "j"));
        let (next, cmds) = step(&snapshot, &status(ending, Some("One more thing?")));
        let m = messages(&cmds);
        assert_eq!(m.len(), 1, "{ending:?}");
        assert_eq!(m[0].text, "One more thing?");
        assert_eq!(m[0].purpose, Some(MessagePurpose::Working));
        assert_eq!(m[0].message_id, "out-j-words-1");
        assert!(m[0].is_final);
        // ahead of the status, so that a reader that dedupes the status's words against the last
        // message finds them said
        let kinds: Vec<_> = bodies(&cmds).iter().map(|b| b.kind()).collect();
        assert_eq!(kinds[0], EventKind::AgentMessage);
        assert_eq!(kinds[1], EventKind::AgentStatus);
        assert!(next.job.answer.is_announced());
    }
}

#[test]
fn words_that_repeat_the_announced_answer_are_not_said_again() {
    // an agent whose `completed` carries the answer it announced (adam does)
    let (snapshot, _) = step(&at(Working), &answer("The result.", 1, "j"));
    let (snapshot, cmds) = step(
        &snapshot,
        &message("s-1", "The result.\n", Some(MessagePurpose::Answer)),
    );
    assert!(cmds.is_empty(), "{cmds:?}");
    let (_, cmds) = step(
        &snapshot,
        &status(AgentTaskState::Completed, Some("The result.")),
    );
    assert!(messages(&cmds).is_empty());
}

#[test]
fn other_words_of_the_turn_are_working_text_too() {
    let (snapshot, _) = step(&at(Working), &answer("The result.", 1, "j"));
    for purpose in [
        None,
        Some(MessagePurpose::Working),
        Some(MessagePurpose::Answer),
    ] {
        let (_, cmds) = step(&snapshot, &message("m", "Committing.", purpose));
        assert_eq!(
            messages(&cmds)[0].purpose,
            Some(MessagePurpose::Working),
            "{purpose:?}"
        );
    }
}

#[test]
fn without_an_announcement_the_adapters_marks_stand() {
    for purpose in [
        None,
        Some(MessagePurpose::Working),
        Some(MessagePurpose::Answer),
    ] {
        let (_, cmds) = step(&at(Working), &message("m", "Words.", purpose));
        assert_eq!(messages(&cmds)[0].purpose, purpose);
    }
    // and the status words are the status's: nothing extra is written
    let (_, cmds) = step(
        &at(Working),
        &status(AgentTaskState::Completed, Some("Done.")),
    );
    assert!(messages(&cmds).is_empty());
}

#[test]
fn a_new_turn_forgets_the_announcement() {
    let announced = step(&at(Working), &answer("The result.", 1, "j")).0;
    assert!(announced.job.answer.is_announced());

    // the person writes
    let (next, _) = step(&announced, &user_says("and another thing"));
    assert!(!next.job.answer.is_announced());
    let (_, cmds) = step(
        &next,
        &message("m", "Reading.", Some(MessagePurpose::Answer)),
    );
    assert_eq!(messages(&cmds)[0].purpose, Some(MessagePurpose::Answer));

    // the person answers a card
    let acted = Input::UiAction {
        user: UserId::new("me@example.com"),
        action: UiActionData {
            surface_id: "s1".into(),
            name: "go".into(),
            source_component_id: "go".into(),
            context: serde_json::Map::new(),
            version: UiVersion::V0_9_1,
            run_id: None,
        },
        catalog: None,
    };
    let (next, _) = step(&announced, &acted);
    assert!(!next.job.answer.is_announced());

    // the next job (a message on a finished thread)
    let mut finished = announced.clone();
    finished.state = Done;
    let (next, _) = step(&finished, &user_says("again"));
    assert!(next.job.answer.is_empty());
    assert_eq!(next.job.number, 2);
}

#[test]
fn an_announcement_the_ledger_remembers_survives_the_store() {
    let (snapshot, _) = step(&at(Working), &answer("The result.", 1, "j"));
    let stored = serde_json::to_value(&snapshot.job).unwrap();
    assert_eq!(stored["answer"]["token"], "j");
    assert_eq!(stored["answer"]["calls"], 1);
    let back: Job = serde_json::from_value(stored).unwrap();
    assert_eq!(back, snapshot.job);
    // a turn that announced nothing leaves nothing in the ledger
    let plain = serde_json::to_value(Job::default()).unwrap();
    assert!(plain.get("answer").is_none(), "{plain}");
    // and a ledger written before the field existed reads as one
    let old: Job = serde_json::from_value(plain).unwrap();
    assert!(old.answer.is_empty());
}
