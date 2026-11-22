//! Child sessions a person drives through the application (ADR 0072, S3d): a person starts a helper
//! session from its parent, sends to it, steers it, stops it and closes it. The checks come before
//! anything is written; what only the agent can refuse comes back with its words; and the rounds a
//! thread sends are followed once, never observed a second time.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;

use orch_app::{AppError, ChildCloseState, ChildSessionDrop, StartChildSession};
use orch_core::{
    AgentTaskState, ChildSessionsOffer, EventBody, EventKind, KnownExtension, Mention, RoundReport,
    SessionBy, SessionEvent, SessionNews, SessionReport, SessionState, StartOutcomeKind, ThreadId,
    ThreadRecord, ThreadState,
};
use orch_ports::memory::{AnswerFault, Call, SendCall};
use orch_ports::{AgentError, OutboxPayload, OutboxStatus, ThreadStore};
use serde_json::json;
use support::*;

fn offer() -> ChildSessionsOffer {
    ChildSessionsOffer::from_params(&json!({
        "agents": [
            {"name": "planner", "description": "Plans"},
            {"name": "researcher", "description": "Reads"}
        ],
        "maxOpen": 16
    }))
    .unwrap()
}

/// A world whose `plain` agent offers helpers and takes steers.
fn world() -> World {
    let w = World::new();
    w.agent.set_child_sessions("plain", Some(offer()));
    w.agent.set_extensions("plain", &[KnownExtension::Steer]);
    w
}

fn start(request: &str, text: &str, keep_open: bool) -> StartChildSession {
    StartChildSession {
        request: request.into(),
        agent: "planner".into(),
        description: Some("Find the writers".into()),
        text: text.into(),
        keep_open,
        tier: None,
    }
}

fn count(events: &[orch_core::Event], kind: EventKind) -> usize {
    events.iter().filter(|e| e.kind() == kind).count()
}

async fn children(w: &World, parent: ThreadId) -> Vec<ThreadRecord> {
    w.store.child_threads(&alice(), parent).await.unwrap()
}

/// A parent that has answered (it has a context), and the dispatcher stopped.
async fn answered(app: &Arc<TestApp>) -> ThreadRecord {
    let run = spawn_dispatcher(app, fast(), "d0");
    let parent = create(app, &alice(), "plain", "echo hi").await;
    wait_state(app, &alice(), parent.id, ThreadState::Done).await;
    run.shutdown().await;
    app.get_thread(&alice(), parent.id).await.unwrap()
}

/// What the agent was sent about the context `context`: `(task, text, steer)`.
fn sent_to(w: &World, context: &str) -> Vec<(Option<String>, String, bool)> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) if call.context_id.as_deref() == Some(context) => {
                let SendCall {
                    task_id,
                    text,
                    steer,
                    reference_task_ids,
                    ..
                } = *call;
                assert!(
                    reference_task_ids.is_empty(),
                    "nothing references an earlier round of a session: {reference_task_ids:?}"
                );
                Some((task_id, text, steer))
            }
            _ => None,
        })
        .collect()
}

async fn the_child(w: &World, parent: ThreadId) -> ThreadRecord {
    eventually("the child thread", || async {
        children(w, parent).await.into_iter().next()
    })
    .await
}

fn code_of(e: &AppError) -> &str {
    match e {
        AppError::Arrangement { code, .. } | AppError::Blocked { code, .. } => code,
        other => panic!("not a coded refusal: {other:?}"),
    }
}

// --- starting ----------------------------------------------------------------------------------

#[tokio::test]
async fn a_person_starts_a_helper_and_it_opens_with_their_prompt() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.start_child_session(
        &alice(),
        parent.id,
        start("req-1", "echo list every banana", true),
    )
    .await
    .unwrap();
    let kid = the_child(&w, parent.id).await;
    let kid = wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    let ended = eventually("the outcome", || async {
        let ev = events(&app, &alice(), parent.id).await;
        (count(&ev, EventKind::ChildSessionStartEnded) == 1).then_some(ev)
    })
    .await;
    run.shutdown().await;

    // the parent's log: asked (no text), opened by the person, ended opened, each once
    assert_eq!(count(&ended, EventKind::ChildSessionStartAsked), 1);
    assert_eq!(count(&ended, EventKind::ChildSessionOpened), 1);
    let asked = ended
        .iter()
        .position(|e| e.kind() == EventKind::ChildSessionStartAsked)
        .unwrap();
    let done = ended
        .iter()
        .position(|e| e.kind() == EventKind::ChildSessionStartEnded)
        .unwrap();
    assert!(asked < done);
    assert!(
        !serde_json::to_string(&ended[asked])
            .unwrap()
            .contains("banana"),
        "the log keeps no word of the prompt"
    );
    match &ended[done].body {
        EventBody::ChildSessionStartEnded(d) => {
            assert_eq!(d.request, "req-1");
            assert_eq!(d.outcome, StartOutcomeKind::Opened);
            assert_eq!(d.thread, Some(kid.id));
            assert!(d.session.as_deref().is_some_and(|s| s.starts_with("s-")));
        }
        other => panic!("{other:?}"),
    }
    let opened = ended
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionOpened(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(opened.by, Some(SessionBy::Person));

    // the child's log: the person's prompt, then the round it began, by the person
    let ev = events(&app, &alice(), kid.id).await;
    assert_eq!(ev[0].kind(), EventKind::UserMessage, "{:?}", shape(&ev));
    match &ev[0].body {
        EventBody::UserMessage(m) => assert_eq!(m.text, "echo list every banana"),
        other => panic!("{other:?}"),
    }
    let started = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::JobStarted(s) => Some(s),
            _ => None,
        })
        .unwrap();
    assert_eq!(started.round.as_ref().unwrap().by, Some(SessionBy::Person));
    // the agent was asked once to start, in the parent's context, naming no task
    let starts = sent_to(&w, "ctx-1");
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(
        starts[0],
        (None, "echo list every banana".to_owned(), false)
    );
    // the helper is nested and says whose child it is
    assert_eq!(kid.child_of.as_ref().unwrap().thread, parent.id);
}

#[tokio::test]
async fn a_start_is_decided_before_anything_is_written() {
    let w = world();
    let app = w.app();
    // a parent that never answered has no context to start a session in
    let fresh = create(&app, &alice(), "plain", "echo hi").await;
    let e = app
        .start_child_session(&alice(), fresh.id, start("r", "echo x", false))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "no_context");
    let parent = answered(&app).await;
    let log_before = events(&app, &alice(), parent.id).await.len();

    let mut cases: Vec<(StartChildSession, &str)> = Vec::new();
    let mut unknown = start("r", "echo x", false);
    unknown.agent = "x".into();
    cases.push((unknown, "unknown_helper"));
    for description in [None, Some(""), Some("  "), Some("two\nlines")] {
        let mut s = start("r", "echo x", false);
        s.description = description.map(str::to_owned);
        cases.push((s, "bad_description"));
    }
    let mut long = start("r", "echo x", false);
    long.description = Some("x".repeat(81));
    cases.push((long, "bad_description"));
    for (request, code) in cases {
        let e = app
            .start_child_session(&alice(), parent.id, request)
            .await
            .unwrap_err();
        assert_eq!(code_of(&e), code, "{e:?}");
    }
    // the words of the refusals are the agent's
    let mut unknown = start("r", "echo x", false);
    unknown.agent = "x".into();
    let e = app
        .start_child_session(&alice(), parent.id, unknown)
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "no helper `x`; `start.agent` is one of: planner, researcher"
    );
    // an empty text is refused as a message is
    let e = app
        .start_child_session(&alice(), parent.id, start("r", "", false))
        .await
        .unwrap_err();
    assert!(
        matches!(e, AppError::Invalid(_) | AppError::Unprocessable(_)),
        "{e:?}"
    );
    // the card stops offering helpers
    w.agent.set_child_sessions("plain", None);
    let e = app
        .start_child_session(&alice(), parent.id, start("r", "echo x", false))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "no_child_sessions");
    // the card cannot be read
    w.agent.set_card_down("plain", true);
    let e = app
        .start_child_session(&alice(), parent.id, start("r", "echo x", false))
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::CardUnavailable { .. }), "{e:?}");
    // another person's thread is a 404
    w.agent.set_card_down("plain", false);
    w.agent.set_child_sessions("plain", Some(offer()));
    let e = app
        .start_child_session(&bob(), parent.id, start("r", "echo x", false))
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::NotFound), "{e:?}");

    assert_eq!(events(&app, &alice(), parent.id).await.len(), log_before);
    assert!(
        w.store
            .list_open_outbox(parent.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_helper_thread_cannot_start_a_helper() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.start_child_session(&alice(), parent.id, start("r", "echo x", false))
        .await
        .unwrap();
    let kid = the_child(&w, parent.id).await;
    run.shutdown().await;
    let e = app
        .start_child_session(&alice(), kid.id, start("r2", "echo y", false))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "child_thread");
}

#[tokio::test]
async fn the_same_request_is_one_start_and_a_fifth_waiting_start_is_refused() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    // no dispatcher: every start waits for its outcome
    for n in 1..=4 {
        app.start_child_session(
            &alice(),
            parent.id,
            start(&format!("r{n}"), "echo x", false),
        )
        .await
        .unwrap();
    }
    // a repeat of the first changes nothing
    app.start_child_session(&alice(), parent.id, start("r1", "echo x", false))
        .await
        .unwrap();
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionStartAsked), 4);
    assert_eq!(w.store.list_open_outbox(parent.id).await.unwrap().len(), 4);
    let e = app
        .start_child_session(&alice(), parent.id, start("r5", "echo x", false))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "too_many_starts");
    assert_eq!(
        count(
            &events(&app, &alice(), parent.id).await,
            EventKind::ChildSessionStartAsked
        ),
        4
    );
}

#[tokio::test]
async fn a_start_the_agent_refuses_ends_with_its_words_and_nothing_is_retried() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    let words = "this context already has 16 sessions open; close one first";
    w.agent
        .fail_next_sends(1, || AgentError::Unsupported(words.to_owned()));
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.start_child_session(&alice(), parent.id, start("req-9", "echo x", false))
        .await
        .unwrap();
    let ev = eventually("the outcome", || async {
        let ev = events(&app, &alice(), parent.id).await;
        (count(&ev, EventKind::ChildSessionStartEnded) == 1).then_some(ev)
    })
    .await;
    let ended = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionStartEnded(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(ended.outcome, StartOutcomeKind::Failed);
    assert_eq!(ended.reason.as_deref(), Some(words));
    assert!(ended.thread.is_none());
    assert!(children(&w, parent.id).await.is_empty());
    // the start was sent once and not again
    assert_eq!(sent_to(&w, "ctx-1").len(), 1);
    // the pending start is gone: another is accepted
    app.start_child_session(&alice(), parent.id, start("req-10", "echo y", false))
        .await
        .unwrap();
    the_child(&w, parent.id).await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_start_that_could_not_reach_the_agent_is_tried_again_and_opens_once() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    w.agent
        .fail_next_sends(2, || AgentError::unreachable("scripted outage"));
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.start_child_session(&alice(), parent.id, start("req-3", "echo x", false))
        .await
        .unwrap();
    let kid = the_child(&w, parent.id).await;
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionStartEnded), 1);
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
    assert_eq!(children(&w, parent.id).await.len(), 1);
    // two attempts failed and the third was the start
    assert_eq!(sent_to(&w, "ctx-1").len(), 3);
}

#[tokio::test]
async fn a_session_a_person_started_that_the_parents_event_reaches_first_is_made_once() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    app.start_child_session(&alice(), parent.id, start("r1", "echo x", false))
        .await
        .unwrap();
    let row = w.store.list_open_outbox(parent.id).await.unwrap().remove(0);
    let session = format!("s-{}", row.id);
    let said = SessionReport {
        session: session.clone(),
        context_id: format!("ctx-1/s/{session}"),
        agent: "planner".into(),
        description: "Find the writers".into(),
        keep_open: false,
        by: Some(SessionBy::Person),
        state: SessionState::Working,
        closing: false,
        round: Some(RoundReport {
            number: 1,
            task_id: Some("t-1".into()),
            state: Some(AgentTaskState::Working),
        }),
        parent_context_id: Some("ctx-1".into()),
    };
    // the parent's own stream says it while the start has not been answered: nothing is made
    assert!(
        app.child_sessions_news(
            parent.id,
            None,
            SessionNews::Event(SessionEvent::Opened, said.clone())
        )
        .await
        .unwrap()
    );
    assert!(children(&w, parent.id).await.is_empty());
    // the start's answer makes it, once
    let run = spawn_dispatcher(&app, fast(), "d1");
    let kid = the_child(&w, parent.id).await;
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    eventually("the outcome", || async {
        let ev = events(&app, &alice(), parent.id).await;
        (count(&ev, EventKind::ChildSessionStartEnded) == 1).then_some(())
    })
    .await;
    run.shutdown().await;
    // and a later listing of the same session makes nothing more
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said]))
        .await
        .unwrap();
    assert_eq!(children(&w, parent.id).await.len(), 1);
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
}

// --- sending, steering, stopping ---------------------------------------------------------------

/// A parent, a kept-open helper whose first round has ended (the script is `echo`), the dispatcher
/// running, and the helper's context.
async fn idle_helper(
    w: &World,
    app: &Arc<TestApp>,
) -> (ThreadRecord, ThreadRecord, String, crate::support::Running) {
    let parent = answered(app).await;
    let run = spawn_dispatcher(app, fast(), "d1");
    app.start_child_session(&alice(), parent.id, start("r1", "echo first", true))
        .await
        .unwrap();
    let kid = the_child(w, parent.id).await;
    let kid = wait_state(app, &alice(), kid.id, ThreadState::Done).await;
    let context = w
        .store
        .get_binding(kid.id)
        .await
        .unwrap()
        .unwrap()
        .context_id
        .unwrap();
    (parent, kid, context, run)
}

#[tokio::test]
async fn a_message_to_an_idle_helper_is_its_next_round_and_is_followed_once() {
    let w = world();
    let app = w.app();
    let (_parent, kid, context, run) = idle_helper(&w, &app).await;
    app.post_message(&alice(), kid.id, "echo again".into())
        .await
        .unwrap();
    eventually("the second job", || async {
        let ev = events(&app, &alice(), kid.id).await;
        (count(&ev, EventKind::JobStarted) == 2
            && app.get_thread(&alice(), kid.id).await.unwrap().state == ThreadState::Done)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    // the agent was told by message to the helper's context, naming no task
    let sent = sent_to(&w, &context);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0], (None, "echo again".to_owned(), false));
    // the round was this thread's own: never observed a second time
    let ev = events(&app, &alice(), kid.id).await;
    assert_eq!(count(&ev, EventKind::JobStarted), 2, "{:?}", shape(&ev));
    assert_eq!(count(&ev, EventKind::UserMessage), 2);
    assert!(w.store.list_open_outbox(kid.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_message_to_a_working_helper_is_a_steer_into_its_round() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.start_child_session(&alice(), parent.id, start("r1", "gate first", true))
        .await
        .unwrap();
    let kid = the_child(&w, parent.id).await;
    let kid = wait_state(&app, &alice(), kid.id, ThreadState::Working).await;
    let context = w
        .store
        .get_binding(kid.id)
        .await
        .unwrap()
        .unwrap()
        .context_id
        .unwrap();
    app.post_message(&alice(), kid.id, "also the readers".into())
        .await
        .unwrap();
    eventually("the steer", || async {
        sent_to(&w, &context)
            .iter()
            .any(|(task, text, steer)| *steer && task.is_some() && text == "also the readers")
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn an_answer_from_a_foreign_context_to_a_helpers_message_fails_closed() {
    let w = world();
    let app = w.app();
    let (_parent, kid, context, run) = idle_helper(&w, &app).await;
    let log_before = events(&app, &alice(), kid.id).await.len();
    w.agent.answer_from_another_context(true);
    app.post_message(&alice(), kid.id, "echo again".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), kid.id, ThreadState::Failed).await;
    run.shutdown().await;
    // the message went to the helper's own context; the answer named another, and nothing of it
    // was applied: no artifact, no text of the agent's, the binding unchanged
    assert_eq!(sent_to(&w, &context).len(), 1);
    let ev = events(&app, &alice(), kid.id).await;
    assert!(
        ev[log_before..].iter().all(|e| !matches!(
            e.kind(),
            EventKind::AgentMessage | EventKind::Artifact | EventKind::AgentStatus
        )),
        "{:?}",
        shape(&ev)
    );
    assert_eq!(count(&ev, EventKind::Error), 1, "{:?}", shape(&ev));
    let dropped = app
        .child_session_stats()
        .dropped
        .iter()
        .find(|(reason, _)| *reason == ChildSessionDrop::ForeignContext)
        .map_or(0, |(_, n)| *n);
    assert!(dropped >= 1);
    let binding = w.store.get_binding(kid.id).await.unwrap().unwrap();
    assert_eq!(binding.context_id.as_deref(), Some(context.as_str()));
}

#[tokio::test]
async fn stop_and_send_is_only_for_a_session_kept_open() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    // not kept open: the stop is its end
    app.start_child_session(&alice(), parent.id, start("r1", "gate one", false))
        .await
        .unwrap();
    let kid = the_child(&w, parent.id).await;
    wait_state(&app, &alice(), kid.id, ThreadState::Working).await;
    let e = app
        .stop_and_send(&alice(), kid.id, "echo new".into())
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "session_closes_on_stop");
    assert_eq!(
        count(
            &events(&app, &alice(), kid.id).await,
            EventKind::UserMessage
        ),
        1,
        "nothing was written"
    );
    w.agent.release_gate();
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_helper_thread_takes_no_mentions() {
    let w = world();
    let app = w.app();
    let (_parent, kid, _context, run) = idle_helper(&w, &app).await;
    let e = app
        .submit(
            &alice(),
            kid.id,
            orch_core::Input::UserMessage {
                user: alice(),
                text: "ask @coder".into(),
                message_id: None,
                run_id: None,
                origin: orch_core::Origin::Agui,
                catalog: None,
                mentions: vec![Mention {
                    agent_id: orch_core::AgentId::new("coder"),
                    label: "@coder".into(),
                    start: 4,
                    end: 10,
                    card_url: None,
                    tier_cap: None,
                }],
                tier: None,
                tier_cap: None,
                locale: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "child_mentions");
    run.shutdown().await;
}

// --- closing -----------------------------------------------------------------------------------

#[tokio::test]
async fn a_close_reaches_the_agent_and_its_closing_round_is_never_a_job() {
    let w = world();
    let app = w.app();
    let (_parent, kid, context, run) = idle_helper(&w, &app).await;
    assert_eq!(
        app.close_child_session(&alice(), kid.id, false)
            .await
            .unwrap(),
        ChildCloseState::Closing
    );
    eventually("the close", || async {
        let ev = events(&app, &alice(), kid.id).await;
        let closed = ev.iter().any(|e| match &e.body {
            EventBody::ChildSessionState(s) => s.state == SessionState::Closed,
            _ => false,
        });
        closed.then_some(())
    })
    .await;
    let sent = sent_to(&w, &context);
    assert_eq!(sent.last().unwrap(), &(None, "close".to_owned(), false));
    let ev = events(&app, &alice(), kid.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionCloseAsked), 1);
    assert_eq!(count(&ev, EventKind::JobStarted), 1, "{:?}", shape(&ev));
    // a second close is nothing; a message is refused with the code
    assert_eq!(
        app.close_child_session(&alice(), kid.id, true)
            .await
            .unwrap(),
        ChildCloseState::Closed
    );
    assert_eq!(
        events(&app, &alice(), kid.id).await.len(),
        ev.len(),
        "nothing was written"
    );
    let e = app
        .post_message(&alice(), kid.id, "echo late".into())
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "session_closed");
    run.shutdown().await;
}

#[tokio::test]
async fn a_close_the_agent_refuses_says_why_and_may_be_asked_again() {
    let w = world();
    let app = w.app();
    let (_parent, kid, context, run) = idle_helper(&w, &app).await;
    let words = format!("session {context} is closed and takes no more messages");
    let said = words.clone();
    w.agent
        .fail_next_sends(1, move || AgentError::Unsupported(said.clone()));
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    let ev = eventually("the error", || async {
        let ev = events(&app, &alice(), kid.id).await;
        ev.iter()
            .any(|e| e.kind() == EventKind::Error)
            .then_some(ev)
    })
    .await;
    let error = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (error.message.as_str(), error.retryable),
        (words.as_str(), false)
    );
    // asked again, the close goes out
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    eventually("the second close", || async {
        (count(
            &events(&app, &alice(), kid.id).await,
            EventKind::ChildSessionCloseAsked,
        ) == 2)
            .then_some(())
    })
    .await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_close_that_could_not_reach_the_agent_is_tried_again_and_closes_once() {
    let w = world();
    let app = w.app();
    let (_parent, kid, context, run) = idle_helper(&w, &app).await;
    let before = sent_to(&w, &context).len();
    w.agent
        .fail_next_sends(2, || AgentError::unreachable("scripted outage"));
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    eventually("the session closed", || async {
        let record = app.get_thread(&alice(), kid.id).await.unwrap();
        (record.job.child.as_ref().map(|c| c.state) == Some(SessionState::Closed)).then_some(())
    })
    .await;
    run.shutdown().await;
    // two attempts failed and the third was the close: once in the log, no error, the row done
    let ev = events(&app, &alice(), kid.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionCloseAsked), 1);
    assert_eq!(count(&ev, EventKind::Error), 0, "{:?}", shape(&ev));
    assert_eq!(sent_to(&w, &context).len() - before, 3);
    assert!(w.store.list_open_outbox(kid.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn only_a_helper_thread_can_be_closed_and_only_by_its_owner() {
    let w = world();
    let app = w.app();
    let (parent, kid, _context, run) = idle_helper(&w, &app).await;
    let e = app
        .close_child_session(&alice(), parent.id, false)
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "not_child_thread");
    let e = app
        .close_child_session(&bob(), kid.id, false)
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::NotFound), "{e:?}");
    run.shutdown().await;
}

// --- a stop is no end of a person's start or close -----------------------------------------------

/// A dispatcher whose retries wait long enough for a stop to land between two attempts.
fn slow_retries() -> orch_app::DispatcherConfig {
    let mut cfg = fast();
    cfg.backoff_base = std::time::Duration::from_millis(1500);
    cfg.backoff_max = std::time::Duration::from_millis(1500);
    cfg
}

/// Waits until the thread's open row `matches` has been tried once and waits for its retry.
async fn backing_off(w: &World, thread: ThreadId, matches: fn(&orch_ports::OutboxPayload) -> bool) {
    eventually("the row backing off", || async {
        w.store
            .list_open_outbox(thread)
            .await
            .unwrap()
            .iter()
            .any(|r| matches(&r.payload) && r.attempts >= 1)
            .then_some(())
    })
    .await;
}

#[tokio::test]
async fn a_stop_and_send_leaves_a_start_backing_off_to_resolve() {
    let w = world();
    let app = w.app();
    let run = spawn_dispatcher(&app, slow_retries(), "d1");
    let parent = create(&app, &alice(), "plain", "gate one").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Working).await;
    eventually("the context", || async {
        w.store
            .get_binding(parent.id)
            .await
            .unwrap()
            .and_then(|b| b.context_id)
    })
    .await;
    w.agent
        .fail_next_sends(1, || AgentError::unreachable("scripted outage"));
    app.start_child_session(&alice(), parent.id, start("req-s", "echo x", false))
        .await
        .unwrap();
    backing_off(&w, parent.id, |p| {
        matches!(p, orch_ports::OutboxPayload::SessionStart { .. })
    })
    .await;
    app.stop_and_send(&alice(), parent.id, "echo next".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = the_child(&w, parent.id).await;
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    let ev = eventually("the start's end", || async {
        let ev = events(&app, &alice(), parent.id).await;
        (count(&ev, EventKind::ChildSessionStartEnded) == 1).then_some(ev)
    })
    .await;
    run.shutdown().await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
    let record = app.get_thread(&alice(), parent.id).await.unwrap();
    assert!(
        record.job.session_starts.is_empty(),
        "the pending start drained: {:?}",
        record.job.session_starts
    );
}

#[tokio::test]
async fn a_stop_and_send_leaves_a_close_backing_off_in_a_kept_open_helper_to_resolve() {
    let w = world();
    let app = w.app();
    let (_parent, kid, context, run) = idle_helper(&w, &app).await;
    run.shutdown().await;
    let run = spawn_dispatcher(&app, slow_retries(), "d2");
    app.post_message(&alice(), kid.id, "gate more".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), kid.id, ThreadState::Working).await;
    w.agent
        .fail_next_sends(1, || AgentError::unreachable("scripted outage"));
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    backing_off(&w, kid.id, |p| {
        matches!(p, orch_ports::OutboxPayload::SessionClose { .. })
    })
    .await;
    app.stop_and_send(&alice(), kid.id, "echo last".into())
        .await
        .unwrap();
    eventually("the session closed", || async {
        let record = app.get_thread(&alice(), kid.id).await.unwrap();
        (record.job.child.as_ref().map(|c| c.state) == Some(SessionState::Closed)).then_some(())
    })
    .await;
    run.shutdown().await;
    let _ = context;
}

// --- a parent that notes its limit of sessions ----------------------------------------------------

/// The parent notes 256 sessions the agent listed.
async fn fill_the_ledger(app: &Arc<TestApp>, parent: ThreadId) {
    let reports: Vec<SessionReport> = (0..256)
        .map(|n| SessionReport {
            session: format!("s{n}"),
            context_id: format!("ctx-1/s/s{n}"),
            agent: "planner".into(),
            description: "Plan".into(),
            parent_context_id: None,
            keep_open: false,
            by: Some(SessionBy::Agent),
            state: SessionState::Idle,
            closing: false,
            round: None,
        })
        .collect();
    for chunk in reports.chunks(64) {
        app.child_sessions_news(parent, None, SessionNews::Listed(chunk.to_vec()))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_start_for_a_parent_that_notes_its_limit_of_sessions_is_refused_and_writes_nothing() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    fill_the_ledger(&app, parent.id).await;
    let before = events(&app, &alice(), parent.id).await.len();
    let e = app
        .start_child_session(&alice(), parent.id, start("req-1", "echo x", false))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "too_many_sessions");
    assert!(matches!(e, AppError::Blocked { .. }), "{e:?}");
    assert_eq!(events(&app, &alice(), parent.id).await.len(), before);
    assert!(
        w.store
            .list_open_outbox(parent.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_start_that_opens_into_a_ledger_that_filled_meanwhile_ends_failed_and_makes_no_thread() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    // accepted while there was room; the ledger fills before the agent opens the session
    app.start_child_session(&alice(), parent.id, start("req-1", "echo x", false))
        .await
        .unwrap();
    fill_the_ledger(&app, parent.id).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let ev = eventually("the outcome", || async {
        let ev = events(&app, &alice(), parent.id).await;
        (count(&ev, EventKind::ChildSessionStartEnded) == 1).then_some(ev)
    })
    .await;
    run.shutdown().await;
    let ended = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionStartEnded(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(ended.outcome, StartOutcomeKind::Failed);
    assert!(
        ended.reason.is_some() && ended.thread.is_none(),
        "{ended:?}"
    );
    assert_eq!(children(&w, parent.id).await.len(), 256);
    let record = app.get_thread(&alice(), parent.id).await.unwrap();
    assert!(record.job.session_starts.is_empty());
    assert_eq!(record.job.sessions.len(), 256);
}

// --- a round of the person's own, reported before the answer names it ------------------------------

/// The parent's stream says round `number` of the helper's session.
fn round_report(
    kid: &ThreadRecord,
    number: u32,
    closing: bool,
    state: SessionState,
) -> SessionReport {
    let of = kid.child_of.clone().unwrap();
    SessionReport {
        session: of.session.clone(),
        context_id: format!("ctx-{}", of.session),
        agent: "planner".into(),
        description: "Find the writers".into(),
        parent_context_id: None,
        keep_open: true,
        by: Some(SessionBy::Person),
        state,
        closing,
        round: Some(orch_core::RoundReport {
            number,
            task_id: Some(format!("{}.r{number}", of.session)),
            state: Some(AgentTaskState::Working),
        }),
    }
}

fn observes(rows: &[orch_ports::OutboxItem]) -> usize {
    rows.iter()
        .filter(|r| matches!(r.payload, orch_ports::OutboxPayload::Observe { .. }))
        .count()
}

#[tokio::test]
async fn the_closing_round_reported_before_the_answer_is_not_followed_as_a_job() {
    let w = world();
    let app = w.app();
    let (parent, kid, _context, run) = idle_helper(&w, &app).await;
    run.shutdown().await;
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    // no dispatcher: the answer to the close is not applied; the parent's stream is ahead of it
    let said = round_report(&kid, 2, true, SessionState::Working);
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said]))
        .await
        .unwrap();
    let rows = w.store.list_open_outbox(kid.id).await.unwrap();
    assert_eq!(observes(&rows), 0, "{rows:?}");
    let after = app.get_thread(&alice(), kid.id).await.unwrap();
    assert_eq!(after.state, ThreadState::Done, "no job was started");
}

#[tokio::test]
async fn a_round_the_person_sent_reported_before_the_answer_is_not_observed() {
    let w = world();
    let app = w.app();
    let (parent, kid, _context, run) = idle_helper(&w, &app).await;
    run.shutdown().await;
    app.post_message(&alice(), kid.id, "echo again".into())
        .await
        .unwrap();
    let said = round_report(&kid, 2, false, SessionState::Working);
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said]))
        .await
        .unwrap();
    let rows = w.store.list_open_outbox(kid.id).await.unwrap();
    assert_eq!(observes(&rows), 0, "{rows:?}");
}

// --- a close checks the live card as a start does ---------------------------------------------------

#[tokio::test]
async fn a_close_for_an_agent_whose_card_stopped_offering_sessions_is_refused_and_writes_nothing() {
    let w = world();
    let app = w.app();
    let (_parent, kid, _context, run) = idle_helper(&w, &app).await;
    run.shutdown().await;
    let before = events(&app, &alice(), kid.id).await.len();
    w.agent.set_child_sessions("plain", None);
    let e = app
        .close_child_session(&alice(), kid.id, false)
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), "no_child_sessions");
    // a card that cannot be read is no answer either
    w.agent.set_child_sessions("plain", Some(offer()));
    w.agent.set_card_down("plain", true);
    let e = app
        .close_child_session(&alice(), kid.id, false)
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::CardUnavailable { .. }), "{e:?}");
    assert_eq!(events(&app, &alice(), kid.id).await.len(), before);
    let record = app.get_thread(&alice(), kid.id).await.unwrap();
    assert!(!record.job.child.unwrap().close_asked);
    assert!(w.store.list_open_outbox(kid.id).await.unwrap().is_empty());
}

// --- what the agent's answer must be to be applied, and what a start or close is checked for ----

/// The status of the one row of `thread` whose payload `matches`.
fn row_status(w: &World, thread: ThreadId, matches: fn(&OutboxPayload) -> bool) -> OutboxStatus {
    let rows: Vec<_> = w
        .store
        .outbox_of(thread)
        .into_iter()
        .filter(|r| matches(&r.payload))
        .collect();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    row.status
}

fn is_start(p: &OutboxPayload) -> bool {
    matches!(p, OutboxPayload::SessionStart { .. })
}

fn is_close(p: &OutboxPayload) -> bool {
    matches!(p, OutboxPayload::SessionClose { .. })
}

async fn start_ended(
    app: &Arc<TestApp>,
    parent: ThreadId,
) -> orch_core::ChildSessionStartEndedData {
    let ev = eventually("the outcome", || async {
        let ev = events(app, &alice(), parent).await;
        (count(&ev, EventKind::ChildSessionStartEnded) == 1).then_some(ev)
    })
    .await;
    ev.iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionStartEnded(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn a_start_answered_with_a_report_that_is_not_the_sessions_ends_failed_and_makes_no_thread() {
    for fault in [
        AnswerFault::ParentsOwnContext,
        AnswerFault::ReportContext,
        AnswerFault::ParentContext,
        AnswerFault::LaterRound,
        AnswerFault::OtherTask,
    ] {
        let w = world();
        let app = w.app();
        let parent = answered(&app).await;
        w.agent.fault_session_answers(Some(fault));
        let run = spawn_dispatcher(&app, fast(), "d1");
        app.start_child_session(&alice(), parent.id, start("r1", "echo x", false))
            .await
            .unwrap();
        let ended = start_ended(&app, parent.id).await;
        run.shutdown().await;
        assert_eq!(ended.outcome, StartOutcomeKind::Failed, "{fault:?}");
        assert_eq!(
            ended.reason.as_deref(),
            Some("the agent did not report the session it started"),
            "{fault:?}"
        );
        assert!(children(&w, parent.id).await.is_empty(), "{fault:?}");
        assert_eq!(
            row_status(&w, parent.id, is_start),
            OutboxStatus::Dead,
            "{fault:?}"
        );
    }
}

#[tokio::test]
async fn a_start_is_the_persons_whatever_the_agent_says_and_its_row_is_delivered() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    w.agent.fault_session_answers(Some(AnswerFault::ByAgent));
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.start_child_session(&alice(), parent.id, start("r1", "echo x", false))
        .await
        .unwrap();
    let ended = start_ended(&app, parent.id).await;
    run.shutdown().await;
    assert_eq!(ended.outcome, StartOutcomeKind::Opened);
    let ev = events(&app, &alice(), parent.id).await;
    let opened = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionOpened(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(opened.by, Some(SessionBy::Person));
    assert_eq!(row_status(&w, parent.id, is_start), OutboxStatus::Delivered);
}

#[tokio::test]
async fn the_first_message_of_a_started_helper_keeps_the_requests_id_and_tier() {
    let w = world();
    w.agent.set_access_tier(
        "plain",
        Some(
            orch_core::TierOffer::from_params(
                &json!({"tiers": ["plan", "edits", "auto"], "default": "auto"}),
            )
            .unwrap(),
        ),
    );
    let app = w.app();
    let parent = answered(&app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let mut request = start("req-7", "echo x", false);
    request.tier = Some(orch_core::AccessTier::Edits);
    app.start_child_session(&alice(), parent.id, request)
        .await
        .unwrap();
    let kid = the_child(&w, parent.id).await;
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;
    let asked = events(&app, &alice(), parent.id)
        .await
        .into_iter()
        .find_map(|e| match e.body {
            EventBody::ChildSessionStartAsked(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(asked.tier, Some(orch_core::AccessTier::Edits));
    let first = events(&app, &alice(), kid.id).await.remove(0);
    match first.body {
        EventBody::UserMessage(m) => {
            assert_eq!(m.message_id.as_deref(), Some("req-7"));
            assert_eq!(m.tier, Some(orch_core::AccessTier::Edits));
        }
        other => panic!("{other:?}"),
    }
    let tiers: Vec<_> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|c| match c {
            Call::Send(call) if call.text == "echo x" => Some(call.tier),
            _ => None,
        })
        .collect();
    assert_eq!(tiers, [Some(orch_core::AccessTier::Edits)]);
}

fn grant_without_invoke(cap: Option<orch_core::AccessTier>, invoke: bool) -> orch_app::AppConfig {
    use orch_app::{AgentScope, Permission, Policy, RoleGrant};
    let mut permissions = std::collections::BTreeSet::from([
        Permission::AgentRead,
        Permission::ThreadRead,
        Permission::ThreadWrite,
    ]);
    if invoke {
        permissions.insert(Permission::AgentInvoke);
    }
    orch_app::AppConfig {
        policy: Policy::new(
            std::collections::BTreeMap::from([(
                orch_ports::Role::new("careful"),
                RoleGrant {
                    permissions,
                    agents: AgentScope::All,
                    max_tier: cap,
                },
            )]),
            Some(orch_ports::Role::new("careful")),
        )
        .unwrap(),
        stream_poll: std::time::Duration::from_millis(100),
        ..orch_app::AppConfig::default()
    }
}

#[tokio::test]
async fn a_start_takes_the_right_to_invoke_the_agent_and_writes_nothing_without_it() {
    let w = world();
    let parent = answered(&w.app()).await;
    let app = w.app_with(grant_without_invoke(None, false));
    let e = app
        .start_child_session(&alice(), parent.id, start("r1", "echo x", false))
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Forbidden { .. }), "{e:?}");
    assert_eq!(
        count(
            &events(&app, &alice(), parent.id).await,
            EventKind::ChildSessionStartAsked
        ),
        0
    );
    assert!(
        w.store
            .list_open_outbox(parent.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_cap_of_a_persons_role_is_pinned_on_the_order_to_start() {
    let w = world();
    let parent = answered(&w.app()).await;
    let app = w.app_with(grant_without_invoke(
        Some(orch_core::AccessTier::Edits),
        true,
    ));
    app.start_child_session(&alice(), parent.id, start("r1", "echo x", false))
        .await
        .unwrap();
    let rows = w.store.list_open_outbox(parent.id).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::SessionStart { tier, tier_cap, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    assert_eq!(
        (*tier, *tier_cap),
        (None, Some(orch_core::AccessTier::Edits))
    );
}

#[tokio::test]
async fn a_description_that_reads_as_nothing_is_refused_and_eighty_characters_are_taken() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    for description in ["plan\u{200b}ner", "plan\u{202e}ner", "\u{feff}planner"] {
        let mut s = start("r", "echo x", false);
        s.description = Some(description.to_owned());
        let e = app
            .start_child_session(&alice(), parent.id, s)
            .await
            .unwrap_err();
        assert_eq!(code_of(&e), "bad_description", "{description:?}");
    }
    let mut s = start("r80", "echo x", false);
    s.description = Some("x".repeat(80));
    app.start_child_session(&alice(), parent.id, s)
        .await
        .unwrap();
    let asked = events(&app, &alice(), parent.id)
        .await
        .into_iter()
        .find_map(|e| match e.body {
            EventBody::ChildSessionStartAsked(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(asked.description.chars().count(), 80);
}

#[tokio::test]
async fn a_start_the_agent_cannot_reach_is_tried_as_often_as_allowed_and_no_more() {
    let w = world();
    let app = w.app();
    let parent = answered(&app).await;
    w.agent
        .fail_next_sends(100, || AgentError::unreachable("scripted outage"));
    let mut cfg = fast();
    cfg.max_attempts = 3;
    let run = spawn_dispatcher(&app, cfg, "d1");
    app.start_child_session(&alice(), parent.id, start("r1", "echo x", false))
        .await
        .unwrap();
    let ended = start_ended(&app, parent.id).await;
    run.shutdown().await;
    assert_eq!(ended.outcome, StartOutcomeKind::Failed);
    assert_eq!(sent_to(&w, "ctx-1").len(), 3);
    assert_eq!(row_status(&w, parent.id, is_start), OutboxStatus::Dead);
}

/// A kept-open helper whose first round has ended, the dispatcher stopped.
async fn rested_helper(w: &World, app: &Arc<TestApp>) -> (ThreadRecord, String) {
    let (_parent, kid, context, run) = idle_helper(w, app).await;
    run.shutdown().await;
    (kid, context)
}

#[tokio::test]
async fn a_close_with_cancel_asks_the_agent_to_cancel_and_says_so_in_the_log() {
    let w = world();
    let app = w.app();
    let (kid, _context) = rested_helper(&w, &app).await;
    app.close_child_session(&alice(), kid.id, true)
        .await
        .unwrap();
    let rows = w.store.list_open_outbox(kid.id).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    assert!(matches!(
        row.payload,
        OutboxPayload::SessionClose { cancel: true }
    ));
    let asked = events(&app, &alice(), kid.id)
        .await
        .into_iter()
        .find_map(|e| match e.body {
            EventBody::ChildSessionCloseAsked(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert!(asked.cancel);
}

#[tokio::test]
async fn the_round_a_close_made_is_the_one_its_answer_names_and_its_row_is_delivered() {
    let w = world();
    let app = w.app();
    let (kid, _context) = rested_helper(&w, &app).await;
    let run = spawn_dispatcher(&app, fast(), "d2");
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    eventually("the session closed", || async {
        let record = app.get_thread(&alice(), kid.id).await.unwrap();
        (record.job.child.as_ref().map(|c| c.state) == Some(SessionState::Closed)).then_some(())
    })
    .await;
    run.shutdown().await;
    let ledger = app
        .get_thread(&alice(), kid.id)
        .await
        .unwrap()
        .job
        .child
        .unwrap();
    assert_eq!(
        (ledger.followed, ledger.started, ledger.track.close_round),
        (2, 1, 2)
    );
    assert_eq!(row_status(&w, kid.id, is_close), OutboxStatus::Delivered);
}

#[tokio::test]
async fn a_close_answered_with_a_report_that_is_not_the_sessions_applies_nothing_of_it() {
    for fault in [
        AnswerFault::OtherSession,
        AnswerFault::ReportContext,
        AnswerFault::OtherTask,
    ] {
        let w = world();
        let app = w.app();
        let (kid, _context) = rested_helper(&w, &app).await;
        w.agent.fault_session_answers(Some(fault));
        let run = spawn_dispatcher(&app, fast(), "d2");
        app.close_child_session(&alice(), kid.id, false)
            .await
            .unwrap();
        eventually("the row done", || async {
            (row_status(&w, kid.id, is_close) == OutboxStatus::Delivered).then_some(())
        })
        .await;
        run.shutdown().await;
        let ledger = app
            .get_thread(&alice(), kid.id)
            .await
            .unwrap()
            .job
            .child
            .unwrap();
        // a report of another task's round is still the session's word, but names no round it made
        if fault != AnswerFault::OtherTask {
            assert_ne!(ledger.state, SessionState::Closed, "{fault:?}");
        }
        assert_eq!(ledger.track.close_round, 0, "{fault:?}");
    }
}

#[tokio::test]
async fn a_close_answered_from_another_context_fails_closed_with_a_word() {
    let w = world();
    let app = w.app();
    let (kid, _context) = rested_helper(&w, &app).await;
    w.agent.answer_from_another_context(true);
    let run = spawn_dispatcher(&app, fast(), "d2");
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    let ev = eventually("the error", || async {
        let ev = events(&app, &alice(), kid.id).await;
        ev.iter()
            .any(|e| e.kind() == EventKind::Error)
            .then_some(ev)
    })
    .await;
    run.shutdown().await;
    assert!(ev.iter().any(|e| matches!(
        &e.body,
        EventBody::Error(d) if d.message == "the agent answered the close with a task of another context"
    )));
    assert_eq!(row_status(&w, kid.id, is_close), OutboxStatus::Dead);
    let ledger = app
        .get_thread(&alice(), kid.id)
        .await
        .unwrap()
        .job
        .child
        .unwrap();
    assert!(!ledger.close_asked, "the ask is cleared");
    assert_ne!(ledger.state, SessionState::Closed);
}

#[tokio::test]
async fn a_close_the_agent_refuses_leaves_its_row_dead() {
    let w = world();
    let app = w.app();
    let (kid, _context) = rested_helper(&w, &app).await;
    w.agent
        .fail_next_sends(1, || AgentError::Unsupported("no".to_owned()));
    let run = spawn_dispatcher(&app, fast(), "d2");
    app.close_child_session(&alice(), kid.id, false)
        .await
        .unwrap();
    eventually("the error", || async {
        events(&app, &alice(), kid.id)
            .await
            .iter()
            .any(|e| e.kind() == EventKind::Error)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    assert_eq!(row_status(&w, kid.id, is_close), OutboxStatus::Dead);
}
