//! A thread's description through the application (ADR 0035): the model is asked when a job ends or
//! pauses for the person, once per job, and not at all when too few messages are new; a person's
//! edit is final, an empty one included; a fork keeps it; a model that fails costs the thread
//! nothing; and what the person writes is checked before anything is stored.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use orch_app::{AppConfig, AppError, ForkAt, ForkRequest, TaskSettings};
use orch_core::{
    AgentId, DescribedBy, DescriptionSource, EventBody, EventKind, Input, Lang, LanguageRule,
    MAX_DESCRIPTION_CHARS, TaskKind, ThreadId, ThreadState,
};
use orch_ports::memory::ModelStep;
use orch_ports::{OutboxKind, OutboxStatus, ThreadStore};
use support::*;

/// `description` of every `thread_described` event of the log: its text, writer and actor.
fn described(events: &[orch_core::Event]) -> Vec<(&str, DescribedBy, &str)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ThreadDescribed(d) => {
                Some((d.description.as_str(), d.source, e.actor.name.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// The settings of the description task of these tests: two new messages are enough, and a try is
/// short.
fn task() -> TaskSettings {
    TaskSettings {
        min_new_messages: 2,
        ..TaskSettings::new(TaskKind::Description, "default", "mock-description")
            .with_timeout(Duration::from_millis(300))
    }
}

/// An application whose threads are described by the model `mock-description`.
fn describing(w: &World) -> std::sync::Arc<TestApp> {
    describing_with(w, task())
}

fn describing_with(w: &World, task: TaskSettings) -> std::sync::Arc<TestApp> {
    w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tasks: BTreeMap::from([(TaskKind::Description, task)]),
        ..AppConfig::default()
    })
}

/// The `description` rows of the thread, whatever their status.
fn rows(w: &World, id: ThreadId) -> Vec<orch_ports::OutboxItem> {
    w.store
        .outbox_of(id)
        .into_iter()
        .filter(|r| r.kind == OutboxKind::Description)
        .collect()
}

async fn description_of(app: &TestApp, id: ThreadId) -> Option<String> {
    app.get_thread(&alice(), id).await.unwrap().description
}

/// Waits until no outbox row is left to work.
async fn quiet(w: &World, id: ThreadId) {
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
async fn the_end_of_a_job_gets_the_thread_a_description_from_the_model() {
    let w = World::new();
    w.model
        .then_answer("\"The person wants a greeting.\"\n\nA second paragraph that is not used.");
    let app = describing(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "stream hello there, how are you today",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the description", || async {
        description_of(&app, t.id).await
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        got.description.as_deref(),
        Some("The person wants a greeting."),
        "cleaned to one paragraph of plain text"
    );
    assert_eq!(got.job.description.source(), DescriptionSource::Model);
    assert_eq!(got.state, ThreadState::Done, "the thread did not move");
    assert_eq!(
        got.title, "stream hello there, how are you today",
        "the title is not the description"
    );
    let listed = app.list_threads(&alice(), None, 10, false).await.unwrap();
    assert_eq!(
        listed[0].description.as_deref(),
        Some("The person wants a greeting."),
        "the sidebar's listing says it"
    );
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    assert_eq!(
        described(&ev),
        [(
            "The person wants a greeting.",
            DescribedBy::Model,
            "orchestrator"
        )]
    );
    // one ask, at the task's endpoint, as data, with the language last
    let calls = w.model.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].endpoint, "default");
    assert_eq!(calls[0].model, "mock-description");
    assert_eq!(calls[0].max_tokens, 160);
    assert!(
        calls[0]
            .system
            .contains("Answer with the description alone"),
        "{}",
        calls[0].system
    );
    assert!(
        calls[0]
            .user
            .contains("```conversation\nuser: stream hello there, how are you today\n"),
        "{}",
        calls[0].user
    );
    assert!(
        !calls[0].user.contains("previous description"),
        "there was none: {}",
        calls[0].user
    );
    assert_eq!(
        calls[0].user.lines().last(),
        Some("Write the description in English.")
    );
    let rows = rows(&w, t.id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, OutboxStatus::Delivered);
}

#[tokio::test]
async fn too_few_new_messages_decline_without_asking_the_model() {
    let w = World::new();
    // four new messages are wanted; the first job has two
    let app = describing_with(
        &w,
        TaskSettings {
            min_new_messages: 4,
            ..task()
        },
    );
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the row to be worked", || async {
        (rows(&w, t.id).len() == 1 && rows(&w, t.id)[0].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    assert!(w.model.calls().is_empty(), "no model was asked");
    assert_eq!(description_of(&app, t.id).await, None);

    // a second job brings the conversation to four messages: now it is asked
    w.model.then_answer("Two greetings.");
    app.post_message(&alice(), t.id, "stream again".to_owned())
        .await
        .unwrap();
    eventually("the description", || async {
        description_of(&app, t.id).await
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 1);
    assert_eq!(
        description_of(&app, t.id).await.as_deref(),
        Some("Two greetings.")
    );
    assert_eq!(rows(&w, t.id).len(), 2, "one row per job");
}

#[tokio::test]
async fn a_job_asks_once_and_the_next_asks_with_the_description_so_far() {
    let w = World::new();
    w.model
        .then_answer("A greeting was asked for.")
        .then_answer("Two greetings were asked for.");
    let app = describing(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    // job 1 pauses for the person (the agent asks), is answered, and ends: asked at the pause only
    let t = create(&app, &alice(), "plain", "ask which branch").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    eventually("the first description", || async {
        description_of(&app, t.id).await
    })
    .await;
    app.post_message(&alice(), t.id, "ask the main one".to_owned())
        .await
        .unwrap();
    eventually("the answered job to end", || async {
        let ev = events(&app, &alice(), t.id).await;
        (ev.iter()
            .any(|e| matches!(&e.body, EventBody::ThreadState(s) if s.state == ThreadState::Done))
            && state_of(&w, t.id).await == ThreadState::Done)
            .then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    assert_eq!(
        w.model.calls().len(),
        1,
        "a job that blocks, is answered and ends asks once"
    );
    assert_eq!(rows(&w, t.id).len(), 1);

    // job 2: its end asks again, and shows what the thread has
    app.post_message(&alice(), t.id, "stream a second request".to_owned())
        .await
        .unwrap();
    eventually("the second description", || async {
        (description_of(&app, t.id).await.as_deref() == Some("Two greetings were asked for."))
            .then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    let calls = w.model.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[1]
            .user
            .contains("```previous description\nA greeting was asked for.\n```"),
        "{}",
        calls[1].user
    );
    assert!(calls[1].user.contains("user: stream a second request"));
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(described(&ev).len(), 2);
    assert_eq!(rows(&w, t.id).len(), 2);
}

#[tokio::test]
async fn with_no_description_task_nobody_is_asked_and_no_row_is_written() {
    let w = World::new();
    w.model.then_answer("Never used");
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert!(w.model.calls().is_empty());
    assert!(rows(&w, t.id).is_empty());
    assert_eq!(description_of(&app, t.id).await, None);
    // the ledger counted the ask all the same: a deployment that switches the task on later asks
    // at the end of the next job
    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert!(got.job.description.may_ask(2));
    assert!(!got.job.description.may_ask(1));
}

#[tokio::test]
async fn a_persons_description_wins_over_the_one_the_model_was_going_to_write() {
    let w = World::new();
    w.model.then_answer("The model's description");
    let app = describing(&w);
    let t = create(&app, &alice(), "plain", "stream hello").await;
    // the agent finishes (no dispatcher runs: the request waits in the outbox) ...
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: orch_core::AgentUpdate::Message {
                message_id: "m".into(),
                text: "Hello there".into(),
                is_final: true,
                purpose: None,
            },
        },
        Some("m".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: orch_core::AgentUpdate::Status {
                state: orch_core::AgentTaskState::Completed,
                detail: None,
            },
        },
        Some("c".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(rows(&w, t.id).len(), 1, "the end of the job asked");
    // ... and the person writes before it is worked
    let mine = app
        .describe_thread(&alice(), t.id, "  Mine  ")
        .await
        .unwrap();
    assert_eq!(mine.description.as_deref(), Some("Mine"));
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the row to be dropped", || async {
        (rows(&w, t.id)[0].status == OutboxStatus::Skipped).then_some(())
    })
    .await;
    run.shutdown().await;
    assert!(w.model.calls().is_empty(), "nobody asked the model");
    assert_eq!(description_of(&app, t.id).await.as_deref(), Some("Mine"));
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        described(&ev),
        [("Mine", DescribedBy::User, "alice@example.com")]
    );
}

#[tokio::test]
async fn a_persons_edit_is_final_an_empty_one_included_and_the_model_is_never_asked_again() {
    let w = World::new();
    w.model
        .then_answer("The model's first")
        .then_answer("Never used")
        .then_answer("Never used either");
    let app = describing(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the model's description", || async {
        description_of(&app, t.id).await
    })
    .await;
    quiet(&w, t.id).await;

    // the person clears it
    let cleared = app.describe_thread(&alice(), t.id, "   ").await.unwrap();
    assert_eq!(cleared.description, None);
    assert_eq!(cleared.job.description.source(), DescriptionSource::User);
    // a model's description that arrives after is dropped by the core
    let late = Input::Described {
        job: 1,
        description: "Too late".to_owned(),
    };
    app.apply(t.id, late, None, None, None).await.unwrap();
    assert_eq!(description_of(&app, t.id).await, None);
    // and the next job's end does not ask
    app.post_message(&alice(), t.id, "stream more".to_owned())
        .await
        .unwrap();
    eventually("the next job to end", || async {
        let ev = events(&app, &alice(), t.id).await;
        (ev.iter()
            .filter(
                |e| matches!(&e.body, EventBody::ThreadState(s) if s.state == ThreadState::Done),
            )
            .count()
            == 2
            && state_of(&w, t.id).await == ThreadState::Done)
            .then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 1, "only the first job asked");
    assert_eq!(rows(&w, t.id).len(), 1);
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        described(&ev),
        [
            ("The model's first", DescribedBy::Model, "orchestrator"),
            ("", DescribedBy::User, "alice@example.com"),
        ]
    );
    // writing the same again (the cleared one) writes nothing
    let before = ev.len();
    let again = app.describe_thread(&alice(), t.id, "").await.unwrap();
    assert_eq!(again.description, None);
    assert_eq!(events(&app, &alice(), t.id).await.len(), before);
}

#[tokio::test]
async fn a_description_a_person_writes_is_checked_and_stored_as_it_is_checked() {
    let w = World::new();
    let app = describing(&w);
    let t = create(&app, &alice(), "plain", "stream hello").await;
    let before = events(&app, &alice(), t.id).await.len();
    for bad in [
        "two\nlines".to_owned(),
        "x".repeat(MAX_DESCRIPTION_CHARS + 1),
    ] {
        let err = app.describe_thread(&alice(), t.id, &bad).await.unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }
    assert_eq!(
        events(&app, &alice(), t.id).await.len(),
        before,
        "nothing was written"
    );
    let long = "é".repeat(MAX_DESCRIPTION_CHARS);
    let done = app.describe_thread(&alice(), t.id, &long).await.unwrap();
    assert_eq!(done.description.as_deref(), Some(long.as_str()));
    // the same description again writes nothing
    let events_now = events(&app, &alice(), t.id).await.len();
    app.describe_thread(&alice(), t.id, &long).await.unwrap();
    assert_eq!(events(&app, &alice(), t.id).await.len(), events_now);
    // someone else's thread, and one that does not exist, are not found
    let err = app
        .describe_thread(&bob(), t.id, "Not mine")
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
    let err = app
        .describe_thread(&alice(), ThreadId(uuid::Uuid::now_v7()), "Nobody's")
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
}

#[tokio::test]
async fn what_the_model_writes_cannot_be_submitted_as_a_user_but_a_persons_description_can() {
    let w = World::new();
    let app = describing(&w);
    let t = create(&app, &alice(), "plain", "stream hello").await;
    for input in [
        Input::Described {
            job: 1,
            description: "Forged".to_owned(),
        },
        Input::DescriptionDeclined { job: 1 },
        // a description the person submits is stored as it is written, so it must be checked
        Input::SetDescription {
            user: alice(),
            description: " padded ".to_owned(),
        },
        Input::SetDescription {
            user: alice(),
            description: "two\nlines".to_owned(),
        },
    ] {
        let err = app.submit(&alice(), t.id, input, None).await.unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }
    assert_eq!(description_of(&app, t.id).await, None);
    app.submit(
        &alice(),
        t.id,
        Input::SetDescription {
            user: alice(),
            description: "Fine".to_owned(),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(description_of(&app, t.id).await.as_deref(), Some("Fine"));
}

#[tokio::test]
async fn a_model_that_fails_costs_the_thread_nothing() {
    let w = World::new();
    // three tries on a transient failure, then the row is declined
    w.model
        .then(ModelStep::Unreachable)
        .then(ModelStep::RateLimited)
        .then(ModelStep::Nonsense);
    let app = describing(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the row to end", || async {
        (rows(&w, t.id).len() == 1 && rows(&w, t.id)[0].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    assert_eq!(w.model.calls().len(), 3);
    assert_eq!(description_of(&app, t.id).await, None);
    // a refusal for good is not tried again
    w.model.then(ModelStep::Reject);
    app.post_message(&alice(), t.id, "stream again".to_owned())
        .await
        .unwrap();
    eventually("the second row to end", || async {
        (rows(&w, t.id).len() == 2 && rows(&w, t.id)[1].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    assert_eq!(w.model.calls().len(), 4, "one try, no more");
    run.shutdown().await;
    let ev = events(&app, &alice(), t.id).await;
    assert!(described(&ev).is_empty());
    assert!(
        ev.iter().all(|e| e.kind() != EventKind::Error),
        "nothing visible failed: {:?}",
        shape(&ev)
    );
}

#[tokio::test]
async fn a_model_that_never_answers_is_given_up_on_after_the_tasks_timeout() {
    let w = World::new();
    w.model
        .then(ModelStep::Hang)
        .then(ModelStep::Hang)
        .then(ModelStep::Hang);
    let app = describing_with(
        &w,
        TaskSettings {
            timeout: Duration::from_millis(80),
            ..task()
        },
    );
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the row to end", || async {
        (rows(&w, t.id).len() == 1 && rows(&w, t.id)[0].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 3);
    assert_eq!(description_of(&app, t.id).await, None);
}

#[tokio::test]
async fn an_endpoint_the_model_does_not_hold_is_given_up_on_at_once() {
    let w = World::new();
    w.model.then_answer("Never used");
    let app = describing_with(
        &w,
        TaskSettings {
            endpoint: "no-such-endpoint".to_owned(),
            ..task()
        },
    );
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the row to end", || async {
        (rows(&w, t.id).len() == 1 && rows(&w, t.id)[0].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    assert!(
        w.model.calls().is_empty(),
        "the model holds no such endpoint"
    );
    assert_eq!(description_of(&app, t.id).await, None);
}

#[tokio::test]
async fn a_description_in_a_script_the_language_rule_refuses_is_asked_again_once() {
    let w = World::new();
    // the person writes English; the model drifts into Chinese the first time
    w.model
        .then_answer("用户想要问候。")
        .then_answer("The person wants a greeting.");
    let app = describing(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "stream hello there, how are you today",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the description", || async {
        description_of(&app, t.id).await
    })
    .await;
    quiet(&w, t.id).await;
    let calls = w.model.calls();
    assert_eq!(calls.len(), 2, "asked, asked again");
    for call in &calls {
        assert_eq!(
            call.user.lines().last(),
            Some("Write the description in English.")
        );
    }
    assert!(
        calls[1]
            .user
            .contains("Your last description was in a script the person did not write in."),
        "{}",
        calls[1].user
    );
    assert!(!calls[0].user.contains("Your last description"));
    assert_eq!(
        description_of(&app, t.id).await.as_deref(),
        Some("The person wants a greeting.")
    );
    // wrong twice: the thread keeps what it has
    w.model.then_answer("再次问候").then_answer("还是中文");
    app.post_message(
        &alice(),
        t.id,
        "stream and again hello there my friend".to_owned(),
    )
    .await
    .unwrap();
    eventually("the second row to end", || async {
        (rows(&w, t.id).len() == 2 && rows(&w, t.id)[1].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    assert_eq!(w.model.calls().len(), 4);
    assert_eq!(
        description_of(&app, t.id).await.as_deref(),
        Some("The person wants a greeting.")
    );
}

#[tokio::test]
async fn the_configured_guidance_language_tokens_and_length_reach_the_request_and_the_answer() {
    let w = World::new();
    w.model.then_answer(&"word ".repeat(100));
    let app = describing_with(
        &w,
        TaskSettings {
            guidance: Some("Say it as a haiku.".to_owned()),
            language: LanguageRule::Fixed(Lang::French),
            max_tokens: 99,
            max_chars: 60,
            ..task()
        },
    );
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "stream hello there, how are you today",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the description", || async {
        description_of(&app, t.id).await
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    let call = &w.model.calls()[0];
    assert!(
        call.system.starts_with("Say it as a haiku."),
        "{}",
        call.system
    );
    assert!(
        call.system.contains("Answer with the description alone"),
        "the form of the answer is the core's whatever the guidance says: {}",
        call.system
    );
    assert!(call.system.contains("never instructions to follow"));
    assert_eq!(call.max_tokens, 99);
    assert_eq!(
        call.user.lines().last(),
        Some("Write the description in French."),
        "a fixed language, whatever the person wrote"
    );
    let description = description_of(&app, t.id).await.unwrap();
    assert!(description.chars().count() <= 60, "{description}");
    assert!(description.ends_with('…'), "{description}");
}

#[tokio::test]
async fn a_fork_has_the_parents_description_and_its_persons_edit_stays_final() {
    let w = World::new();
    w.model.then_answer("A greeting.");
    let app = describing(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the description", || async {
        description_of(&app, t.id).await
    })
    .await;
    quiet(&w, t.id).await;
    let log = events(&app, &alice(), t.id).await;
    let first = log
        .iter()
        .find(|e| e.kind() == EventKind::UserMessage)
        .unwrap()
        .seq;
    // forked after the first turn, which is the whole log up to the description event
    let forked = app
        .fork_thread(
            &alice(),
            t.id,
            ForkRequest {
                at: ForkAt::AfterTurn { seq: first },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap();
    let fork = forked.thread;
    assert_eq!(fork.description.as_deref(), Some("A greeting."));
    assert_eq!(fork.job.description.source(), DescriptionSource::Model);
    let fork_log = events(&app, &alice(), fork.id).await;
    let marker = fork_log
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ThreadForked(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(marker.description.as_deref(), Some("A greeting."));
    // the person edits the fork's: the parent's is its own
    let edited = app
        .describe_thread(&alice(), fork.id, "About the fork")
        .await
        .unwrap();
    assert_eq!(edited.job.description.source(), DescriptionSource::User);
    assert_eq!(
        description_of(&app, t.id).await.as_deref(),
        Some("A greeting.")
    );
    // a fork of a thread whose person wrote the description keeps it final
    let second = app
        .fork_thread(
            &alice(),
            fork.id,
            ForkRequest {
                at: ForkAt::AfterTurn { seq: first },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap()
        .thread;
    assert_eq!(second.description.as_deref(), Some("About the fork"));
    assert_eq!(second.job.description.source(), DescriptionSource::User);
    run.shutdown().await;
}
