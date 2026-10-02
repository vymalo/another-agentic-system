//! A thread's title through the application: a person renames it in any state, the title and the
//! `thread_titled` event are one commit, the rename survives what the agent does next, and a
//! rename that cannot be used writes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_app::{AppConfig, AppError};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, EventBody, EventKind, Input, ThreadState, TitleSource,
    TitledBy,
};
use orch_ports::memory::ModelStep;
use orch_ports::{OutboxKind, ThreadStore};
use support::*;

fn titled(events: &[orch_core::Event]) -> Vec<(&str, TitledBy, &str)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ThreadTitled(t) => Some((t.title.as_str(), t.source, e.actor.name.as_str())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_rename_is_the_title_and_one_event_in_the_log() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo fix the build").await;
    assert_eq!(t.title, "echo fix the build");
    assert_eq!(t.job.title.source(), TitleSource::FirstMessage);

    let renamed = app
        .rename_thread(&alice(), t.id, "  The build  ")
        .await
        .unwrap();
    assert_eq!(renamed.title, "The build", "the title is trimmed");
    assert_eq!(renamed.job.title.source(), TitleSource::User);
    assert_eq!(renamed.state, ThreadState::Queued, "nothing else moved");
    assert!(renamed.version > t.version);

    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(got.title, "The build");
    let listed = app.list_threads(&alice(), None, 10, false).await.unwrap();
    assert_eq!(
        listed[0].title, "The build",
        "the sidebar's listing says it"
    );

    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev), ["user_message", "thread_titled"]);
    assert_contiguous(&ev);
    assert_eq!(
        titled(&ev),
        [("The build", TitledBy::User, "alice@example.com")]
    );
}

#[tokio::test]
async fn a_thread_in_every_state_can_be_renamed_and_the_state_does_not_move() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let done = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), done.id, ThreadState::Done).await;
    let blocked = create(&app, &alice(), "plain", "ask me").await;
    wait_state(&app, &alice(), blocked.id, ThreadState::Blocked).await;
    run.shutdown().await;

    for (id, state) in [
        (done.id, ThreadState::Done),
        (blocked.id, ThreadState::Blocked),
    ] {
        let before = events(&app, &alice(), id).await.len();
        let renamed = app.rename_thread(&alice(), id, "Mine").await.unwrap();
        assert_eq!(renamed.state, state);
        let ev = events(&app, &alice(), id).await;
        assert_eq!(ev.len(), before + 1, "{state:?}: one event");
        assert_eq!(ev.last().unwrap().kind(), EventKind::ThreadTitled);
    }

    // the rename queued no work for the agent
    let stats = app.outbox_stats().await.unwrap().1;
    assert_eq!(stats.due + stats.waiting + stats.leased, 0);
}

#[tokio::test]
async fn the_same_title_again_is_no_event() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo hi").await;
    app.rename_thread(&alice(), t.id, "Mine").await.unwrap();
    let once = events(&app, &alice(), t.id).await;
    let again = app.rename_thread(&alice(), t.id, " Mine ").await.unwrap();
    assert_eq!(again.title, "Mine");
    assert_eq!(events(&app, &alice(), t.id).await, once);
    // another title is another rename
    app.rename_thread(&alice(), t.id, "Other").await.unwrap();
    assert_eq!(titled(&events(&app, &alice(), t.id).await).len(), 2);
}

#[tokio::test]
async fn a_title_the_thread_has_from_its_first_words_is_still_the_persons_once_they_say_so() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo hi").await;
    assert_eq!(t.title, "echo hi");
    // the same words as the first message's, but now they are the person's
    let renamed = app.rename_thread(&alice(), t.id, "echo hi").await.unwrap();
    assert_eq!(renamed.job.title.source(), TitleSource::User);
    assert_eq!(titled(&events(&app, &alice(), t.id).await).len(), 1);
}

#[tokio::test]
async fn a_title_that_cannot_be_used_writes_nothing() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo hi").await;
    let before = app.get_thread(&alice(), t.id).await.unwrap();
    let long = "x".repeat(201);
    for bad in ["", "   ", "two\nlines", "tab\there", long.as_str()] {
        let err = app.rename_thread(&alice(), t.id, bad).await.unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{bad:?}: {err:?}");
    }
    assert_eq!(app.get_thread(&alice(), t.id).await.unwrap(), before);
    assert!(titled(&events(&app, &alice(), t.id).await).is_empty());
    // the largest is fine
    let fits = "x".repeat(200);
    let ok = app.rename_thread(&alice(), t.id, &fits).await.unwrap();
    assert_eq!(ok.title, fits);
}

#[tokio::test]
async fn nobody_renames_another_persons_thread() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo hi").await;
    let err = app.rename_thread(&bob(), t.id, "Mine").await.unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
    let err = app
        .rename_thread(
            &alice(),
            orch_core::ThreadId(uuid::Uuid::from_u128(0xdead)),
            "x",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().title,
        "echo hi"
    );
}

#[tokio::test]
async fn a_surface_cannot_submit_a_title_the_application_would_not_store() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo hi").await;
    for bad in [" padded ", "", "two\nlines"] {
        let err = app
            .submit(
                &alice(),
                t.id,
                Input::Rename {
                    user: alice(),
                    title: bad.to_owned(),
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{bad:?}: {err:?}");
    }
    app.submit(
        &alice(),
        t.id,
        Input::Rename {
            user: alice(),
            title: "Fine".to_owned(),
        },
        Some("k".to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(app.get_thread(&alice(), t.id).await.unwrap().title, "Fine");
}

#[tokio::test]
async fn the_persons_title_survives_what_the_agent_does_and_the_next_job() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    app.rename_thread(&alice(), t.id, "Mine").await.unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(app.get_thread(&alice(), t.id).await.unwrap().title, "Mine");

    // a message on the finished thread starts the next job; the title stays
    app.post_message(&alice(), t.id, "echo again".to_owned())
        .await
        .unwrap();
    eventually("the second job", || async {
        let t = app.get_thread(&alice(), t.id).await.unwrap();
        (t.job.number == 2 && t.state == ThreadState::Done).then_some(())
    })
    .await;
    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(got.title, "Mine");
    assert_eq!(got.job.title.source(), TitleSource::User);
    run.shutdown().await;
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    assert_eq!(titled(&ev).len(), 1);
}

#[tokio::test]
async fn renames_racing_the_agent_are_all_applied_and_the_log_stays_contiguous() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    let mut tasks = Vec::new();
    for n in 0..5 {
        let app = app.clone();
        let id = t.id;
        tasks.push(tokio::spawn(async move {
            app.rename_thread(&alice(), id, &format!("Title {n}")).await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    let names = titled(&ev);
    assert_eq!(names.len(), 5, "{names:?}");
    // the stored title is the last one the log says
    let last = names.last().unwrap().0;
    assert_eq!(app.get_thread(&alice(), t.id).await.unwrap().title, last);
}

// ---- the model's titles (S6.7) ---------------------------------------------------------------

/// An application whose threads are titled by the model `mock-title` (the scripted one).
fn titling(w: &World) -> std::sync::Arc<TestApp> {
    w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        title_model: Some("mock-title".to_owned()),
        title_timeout: Duration::from_millis(300),
        ..AppConfig::default()
    })
}

/// The `title` rows of the thread, whatever their status.
fn title_rows(w: &World, id: orch_core::ThreadId) -> Vec<orch_ports::OutboxItem> {
    w.store
        .outbox_of(id)
        .into_iter()
        .filter(|r| r.kind == OutboxKind::Title)
        .collect()
}

async fn title_of(app: &TestApp, id: orch_core::ThreadId) -> String {
    app.get_thread(&alice(), id).await.unwrap().title
}

/// Waits until no outbox row is left to work: the title row has ended one way or the other.
async fn quiet(w: &World, id: orch_core::ThreadId) {
    eventually("the outbox to be worked", || async {
        w.store
            .list_open_outbox(id)
            .await
            .unwrap()
            .is_empty()
            .then_some(())
    })
    .await;
}

#[tokio::test]
async fn the_agents_first_words_get_the_thread_a_title_from_the_model() {
    let w = World::new();
    w.model
        .then_answer("\"Which branch to use\"\n(the model explains)");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "ask me about the build").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    eventually("the title", || async {
        (title_of(&app, t.id).await != "ask me about the build").then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        got.title, "Which branch to use",
        "cleaned to one line of plain text"
    );
    assert_eq!(got.job.title.source(), TitleSource::Model);
    assert_eq!(got.state, ThreadState::Blocked, "the thread did not move");
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    assert_eq!(
        titled(&ev),
        [("Which branch to use", TitledBy::Model, "orchestrator")]
    );
    // the model was asked once, about the conversation, as data
    let calls = w.model.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].model, "mock-title");
    assert!(
        calls[0].system.contains("3 to 6 word title"),
        "{:?}",
        calls[0].system
    );
    assert!(
        calls[0].user.contains("user: ask me about the build"),
        "{}",
        calls[0].user
    );
    assert!(
        calls[0].user.contains("agent: Which branch?"),
        "{}",
        calls[0].user
    );
    assert!(
        calls[0].user.contains("```conversation"),
        "{}",
        calls[0].user
    );
}

#[tokio::test]
async fn a_title_arrives_for_a_thread_that_finished_meanwhile_and_does_not_move_it() {
    let w = World::new();
    w.model.then_answer("Fibonacci in Rust");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream write fibonacci").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the title", || async {
        (title_of(&app, t.id).await == "Fibonacci in Rust").then_some(())
    })
    .await;
    run.shutdown().await;
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().state,
        ThreadState::Done
    );
    assert_contiguous(&events(&app, &alice(), t.id).await);
}

#[tokio::test]
async fn none_keeps_the_first_words_and_the_next_reply_asks_again() {
    let w = World::new();
    w.model
        .then_answer("NONE")
        .then_answer("Writing a Fibonacci function");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    assert_eq!(
        title_of(&app, t.id).await,
        "stream hi",
        "no topic yet: the first words stay"
    );
    assert_eq!(w.model.calls().len(), 1);

    // the next job's reply asks again, and now there is a topic
    app.post_message(
        &alice(),
        t.id,
        "stream write a fibonacci function".to_owned(),
    )
    .await
    .unwrap();
    eventually("the title", || async {
        (title_of(&app, t.id).await == "Writing a Fibonacci function").then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    assert_eq!(w.model.calls().len(), 2);
    // and that is the last time: a third reply asks nobody
    app.post_message(&alice(), t.id, "stream thanks".to_owned())
        .await
        .unwrap();
    eventually("the third job", || async {
        let t = app.get_thread(&alice(), t.id).await.unwrap();
        (t.job.number == 3 && t.state == ThreadState::Done).then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 2);
    assert_eq!(titled(&events(&app, &alice(), t.id).await).len(), 1);
}

/// The agent's reply reaches the thread as two inputs: the final message and, a moment later, the
/// status that ends the turn with the same words (a streamed reply is stated in both). The
/// dispatcher works the title row on its own task, so the model's answer can be applied between
/// them. Played by hand, in the order that used to ask twice: the reply must ask once, or the
/// thread's second ask is spent on the same words and the next reply (the one with a topic)
/// is never asked for.
#[tokio::test]
async fn a_reply_the_model_answered_before_it_ended_does_not_ask_twice() {
    let w = World::new();
    let app = titling(&w);
    let t = create(&app, &alice(), "plain", "stream hi").await;
    let agent = |update| Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    };
    let words = "Streaming a reply";
    app.apply(
        t.id,
        agent(AgentUpdate::Message {
            message_id: "m".into(),
            text: words.into(),
            is_final: true,
        }),
        Some("m".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(title_rows(&w, t.id).len(), 1, "the reply asked for a title");
    // the model had no topic yet, and its answer is applied before the reply is over
    app.apply(
        t.id,
        Input::TitleDeclined { ask: 1 },
        Some("title:1".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    app.apply(
        t.id,
        agent(AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: Some(words.into()),
        }),
        Some("s".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(got.state, ThreadState::Done);
    assert_eq!(title_rows(&w, t.id).len(), 1, "the same reply asks once");
    assert_eq!(got.job.title.asks(), 1);

    // the next reply is another reply, and asks
    app.post_message(&alice(), t.id, "write a fibonacci function".to_owned())
        .await
        .unwrap();
    app.apply(
        t.id,
        agent(AgentUpdate::Message {
            message_id: "m2".into(),
            text: "Here it is".into(),
            is_final: true,
        }),
        Some("m2".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(title_rows(&w, t.id).len(), 2, "the next reply asked");
}

#[tokio::test]
async fn a_model_that_cannot_be_reached_costs_the_thread_nothing() {
    let w = World::new();
    let model = orch_ports::memory::ScriptedModel::new(ModelStep::Unreachable);
    let w = World {
        model: model.clone(),
        ..w
    };
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    assert_eq!(title_of(&app, t.id).await, "stream hello");
    assert_eq!(
        model.calls().len(),
        3,
        "asked three times, then given up on"
    );
    let ev = events(&app, &alice(), t.id).await;
    assert!(titled(&ev).is_empty());
    assert!(
        ev.iter().all(|e| e.kind() != EventKind::Error),
        "nothing visible failed: {:?}",
        shape(&ev)
    );
    // the row ended as delivered, not dead
    let row = title_rows(&w, t.id);
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].status, orch_ports::OutboxStatus::Delivered);
}

#[tokio::test]
async fn a_refusal_for_good_is_asked_once() {
    let w = World::new();
    let model = orch_ports::memory::ScriptedModel::new(ModelStep::Reject);
    let w = World {
        model: model.clone(),
        ..w
    };
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(model.calls().len(), 1);
    assert_eq!(title_of(&app, t.id).await, "stream hello");
}

#[tokio::test]
async fn a_model_that_never_answers_is_given_up_on_at_the_timeout() {
    let w = World::new();
    let model = orch_ports::memory::ScriptedModel::new(ModelStep::Hang);
    let w = World {
        model: model.clone(),
        ..w
    };
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(model.calls().len(), 3);
    assert_eq!(title_of(&app, t.id).await, "stream hello");
}

#[tokio::test]
async fn with_no_title_model_nobody_is_asked_and_no_row_is_written() {
    let w = World::new();
    w.model.then_answer("Never used");
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert!(w.model.calls().is_empty());
    assert!(title_rows(&w, t.id).is_empty());
    assert_eq!(title_of(&app, t.id).await, "stream hello");
}

#[tokio::test]
async fn a_rename_before_the_answer_wins_and_the_model_is_never_asked() {
    let w = World::new();
    w.model.then_answer("The model's title");
    let app = titling(&w);
    let t = create(&app, &alice(), "plain", "stream hello").await;
    // the agent speaks (no dispatcher is running: the request waits in the outbox) ...
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Message {
                message_id: "m".into(),
                text: "Hello there".into(),
                is_final: true,
            },
        },
        Some("m".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    let rows = title_rows(&w, t.id);
    assert_eq!(rows.len(), 1, "the reply asked for a title");
    // ... and the person renames before it is worked
    app.rename_thread(&alice(), t.id, "Mine").await.unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the title row", || async {
        let rows = title_rows(&w, t.id);
        (rows[0].status == orch_ports::OutboxStatus::Skipped).then_some(())
    })
    .await;
    run.shutdown().await;
    assert!(w.model.calls().is_empty(), "nobody asked the model");
    assert_eq!(title_of(&app, t.id).await, "Mine");
    assert_eq!(titled(&events(&app, &alice(), t.id).await).len(), 1);
}

#[tokio::test]
async fn a_title_nobody_can_submit_as_a_user() {
    let w = World::new();
    let app = titling(&w);
    let t = create(&app, &alice(), "plain", "stream hello").await;
    for input in [
        Input::Titled {
            ask: 1,
            title: "Forged".to_owned(),
        },
        Input::TitleDeclined { ask: 1 },
    ] {
        let err = app.submit(&alice(), t.id, input, None).await.unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }
    assert_eq!(title_of(&app, t.id).await, "stream hello");
    let _ = AgentTaskState::Working;
}

// ---- the language of the title (plan 10, section 3.6) --------------------------------------------

/// The last line of what the model was shown on its `n`-th call (from 0).
fn last_line_of_call(w: &World, n: usize) -> String {
    w.model.calls()[n].user.lines().last().unwrap().to_owned()
}

#[tokio::test]
async fn a_chinese_title_for_an_english_conversation_is_declined_and_the_second_ask_gets_the_english_one()
 {
    let w = World::new();
    // the model of the owner's thread: it drifts into Chinese the first time
    w.model
        .then_answer("Node.js 绘图导出")
        .then_answer("Exporting the drawing");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "ask how do I export the drawing to a file",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    eventually("the title", || async {
        (title_of(&app, t.id).await == "Exporting the drawing").then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        titled(&ev),
        [("Exporting the drawing", TitledBy::Model, "orchestrator")],
        "the Chinese title is in no event"
    );
    // two questions in one row, each ending with the language
    assert_eq!(w.model.calls().len(), 2);
    assert_eq!(last_line_of_call(&w, 0), "Write the title in English.");
    assert_eq!(last_line_of_call(&w, 1), "Write the title in English.");
    let second = &w.model.calls()[1].user;
    assert!(
        second.contains("Your last title was in a script the person did not write in."),
        "{second}"
    );
    assert!(!w.model.calls()[0].user.contains("Your last title"));
    // one row, delivered; the ledger counts one ask
    let rows = title_rows(&w, t.id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, orch_ports::OutboxStatus::Delivered);
    assert_eq!(
        app.get_thread(&alice(), t.id)
            .await
            .unwrap()
            .job
            .title
            .asks(),
        1
    );
}

#[tokio::test]
async fn a_model_that_is_wrong_twice_leaves_the_first_words() {
    let w = World::new();
    w.model.then_answer("绘图导出").then_answer("导出图片问题");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "ask how do I export the drawing to a file",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 2, "asked, asked again, and no more");
    assert_eq!(
        title_of(&app, t.id).await,
        "ask how do I export the drawing to a file",
        "the first words stay"
    );
    let ev = events(&app, &alice(), t.id).await;
    assert!(titled(&ev).is_empty());
    assert!(
        ev.iter().all(|e| e.kind() != EventKind::Error),
        "nothing visible failed: {:?}",
        shape(&ev)
    );
    let rows = title_rows(&w, t.id);
    assert_eq!(rows[0].status, orch_ports::OutboxStatus::Delivered);
}

#[tokio::test]
async fn a_chinese_conversation_keeps_its_chinese_title_and_a_french_one_its_french_title() {
    let w = World::new();
    w.model.then_answer("登录页面修复");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "ask 请修复登录页面的重定向问题").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    eventually("the title", || async {
        (title_of(&app, t.id).await == "登录页面修复").then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    assert_eq!(
        w.model.calls().len(),
        1,
        "a title in the person's script is not declined"
    );
    assert_eq!(last_line_of_call(&w, 0), "Write the title in Chinese.");

    w.model.then_answer("Correction de la redirection");
    let f = create(
        &app,
        &alice(),
        "plain",
        "ask peux-tu corriger la page de connexion et la redirection ?",
    )
    .await;
    wait_state(&app, &alice(), f.id, ThreadState::Blocked).await;
    eventually("the title", || async {
        (title_of(&app, f.id).await == "Correction de la redirection").then_some(())
    })
    .await;
    quiet(&w, f.id).await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 2);
    assert_eq!(last_line_of_call(&w, 1), "Write the title in French.");
}

#[tokio::test]
async fn a_model_that_says_none_is_not_asked_again_for_the_language() {
    let w = World::new();
    w.model.then_answer("NONE");
    let app = titling(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "ask how do I export the drawing to a file",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(
        w.model.calls().len(),
        1,
        "NONE is no title, not a wrong language"
    );
}
