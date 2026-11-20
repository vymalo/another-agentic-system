//! Child sessions a person drives through the application (ADR 0072, S3d): a person starts a helper
//! session from its parent, sends to it, steers it, stops it and closes it. The checks come before
//! anything is written; what only the agent can refuse comes back with its words; and the rounds a
//! thread sends are followed once, never observed a second time.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;

use orch_app::{AppError, ChildCloseState, StartChildSession};
use orch_core::{
    AgentTaskState, ChildSessionsOffer, EventBody, EventKind, KnownExtension, Mention, RoundReport,
    SessionBy, SessionEvent, SessionNews, SessionReport, SessionState, StartOutcomeKind, ThreadId,
    ThreadRecord, ThreadState,
};
use orch_ports::memory::Call;
use orch_ports::{AgentError, ThreadStore};
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
            Call::Send {
                context_id,
                task_id,
                text,
                steer,
                reference_task_ids,
                ..
            } if context_id.as_deref() == Some(context) => {
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
