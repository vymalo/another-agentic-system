//! The turn's announced answer through the application (ADR 0031, `turn_output`): recorded as the
//! agent's message while the thread works, refused when the turn is over, and never a user's to
//! submit.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, ApplyOutcome};
use orch_core::{
    Actor, AgentId, AnswerVia, EventBody, Input, MAX_ANSWER_BYTES, MessagePurpose, ThreadState,
    TransitionError,
};
use orch_ports::ThreadStore;
use support::*;

async fn events_of(app: &TestApp, id: orch_core::ThreadId) -> Vec<orch_core::Event> {
    events(app, &alice(), id).await
}

fn plain() -> AgentId {
    AgentId::new("plain")
}

#[tokio::test]
async fn an_answer_announced_while_the_agent_works_is_its_message_and_the_turn_goes_on() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    // a `gate` agent holds its task open
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;

    let outcome = app
        .record_answer(t.id, &plain(), 1, "m-1", "## Result\n\nIt works.".into())
        .await
        .unwrap();
    let ApplyOutcome::Applied { events, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(events.len(), 1);
    let EventBody::AgentMessage(message) = &events[0].body else {
        panic!("{:?}", events[0])
    };
    assert_eq!(message.message_id, "out-m-1-1");
    assert_eq!(message.text, "## Result\n\nIt works.");
    assert!(message.is_final);
    assert_eq!(message.purpose, Some(MessagePurpose::Answer));
    assert_eq!(message.via, Some(AnswerVia::TurnOutput));
    assert_eq!(events[0].actor, Actor::agent(&plain(), None));

    // the ledger of the job knows, and the thread still works
    let record = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    assert_eq!(record.state, ThreadState::Working);
    assert!(record.job.answer.is_announced());

    // a second one is the next message of the token
    app.record_answer(t.id, &plain(), 1, "m-1", "Better.".into())
        .await
        .unwrap();
    let ev = events_of(&app, t.id).await;
    let ids: Vec<_> = ev
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::AgentMessage(m) => Some(m.message_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["out-m-1-1", "out-m-1-2"]);
    assert_contiguous(&ev);

    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn once_the_turn_is_over_or_for_another_job_the_answer_is_refused() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let before = events_of(&app, t.id).await.len();
    let err = app
        .record_answer(t.id, &plain(), 1, "m-1", "late".into())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Transition(TransitionError::InvalidInState {
                input: "answer",
                ..
            })
        ),
        "{err:?}"
    );
    assert_eq!(events_of(&app, t.id).await.len(), before);

    // a thread that works, asked with a token of another job
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let err = app
        .record_answer(t.id, &plain(), 2, "m-1", "from another job".into())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Transition(_)), "{err:?}");
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn an_answer_that_is_empty_or_too_long_is_invalid_and_a_thread_that_is_not_there_is_not_found()
 {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    for text in ["", " \n", &"x".repeat(MAX_ANSWER_BYTES + 1)] {
        let err = app
            .record_answer(t.id, &plain(), 1, "m-1", text.to_owned())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }
    let missing = orch_core::ThreadId(uuid::Uuid::now_v7());
    let err = app
        .record_answer(missing, &plain(), 1, "m-1", "x".into())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_user_cannot_submit_an_announced_answer() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let err = app
        .submit(
            &alice(),
            t.id,
            Input::Answer {
                actor: Actor::agent(&plain(), None),
                text: "forged".into(),
                job: 1,
                token: "m-1".into(),
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    assert!(
        !events_of(&app, t.id)
            .await
            .iter()
            .any(|e| matches!(&e.body, EventBody::AgentMessage(_)))
    );
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}
