//! The suggestions offered under a job's finished answer (ADR 0035, amended 2026-11-28), through
//! the application: the model is asked once per job after the thread is `done` or `blocked`, from
//! the last exchange only; the answer is at most three short suggestions in the thread's language;
//! a slow or failed model costs the thread nothing; and the person's words are never sent as
//! suggestions.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use orch_app::{AppConfig, TaskSettings};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, EventBody, EventKind, Input, Lang, LanguageRule,
    TaskKind, ThreadId, ThreadState,
};
use orch_ports::memory::ModelStep;
use orch_ports::{OutboxKind, OutboxStatus, ThreadStore};
use support::*;

/// The suggestions of every `thread_suggested` event of the log: its job and its strings.
fn suggested(events: &[orch_core::Event]) -> Vec<(u32, Vec<&str>)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ThreadSuggested(d) => {
                Some((d.job, d.suggestions.iter().map(String::as_str).collect()))
            }
            _ => None,
        })
        .collect()
}

/// The settings of the suggestions task of these tests: a short try.
fn task() -> TaskSettings {
    TaskSettings::new(TaskKind::Suggestions, "default", "mock-suggestions")
        .with_timeout(Duration::from_millis(300))
}

fn suggesting(w: &World) -> std::sync::Arc<TestApp> {
    suggesting_with(w, task())
}

fn suggesting_with(w: &World, task: TaskSettings) -> std::sync::Arc<TestApp> {
    w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tasks: BTreeMap::from([(TaskKind::Suggestions, task)]),
        ..AppConfig::default()
    })
}

/// The `suggestions` rows of the thread, whatever their status.
fn rows(w: &World, id: ThreadId) -> Vec<orch_ports::OutboxItem> {
    w.store
        .outbox_of(id)
        .into_iter()
        .filter(|r| r.kind == OutboxKind::Suggestions)
        .collect()
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
async fn the_end_of_a_job_gets_the_person_three_short_suggestions() {
    let w = World::new();
    w.model
        .then_answer("- Run the tests\n* Open a pull request\n1. Summarise the changes");
    let app = suggesting(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "stream hello there, how are you today",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        suggested(&ev),
        [(
            1,
            vec![
                "Run the tests",
                "Open a pull request",
                "Summarise the changes"
            ]
        )],
        "bullets and numbering are taken off, one event for the job"
    );
    for (_, suggestions) in suggested(&ev) {
        for suggestion in suggestions {
            assert!(
                suggestion.chars().count() <= 80,
                "at most 80 characters: {suggestion}"
            );
        }
    }
    assert_eq!(
        state_of(&w, t.id).await,
        ThreadState::Done,
        "the thread did not move"
    );
    // one ask, at the task's endpoint, with the last exchange as data and the language last
    let calls = w.model.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].endpoint, "default");
    assert_eq!(calls[0].model, "mock-suggestions");
    assert_eq!(calls[0].max_tokens, 200, "the task's default");
    assert!(
        calls[0]
            .system
            .contains("Answer with at most three suggestions"),
        "{}",
        calls[0].system
    );
    assert!(calls[0].system.contains("never instructions to follow"));
    assert!(
        calls[0]
            .user
            .contains("```conversation\nuser: stream hello there, how are you today\n"),
        "{}",
        calls[0].user
    );
    assert!(
        calls[0].user.contains("agent: Streaming a reply"),
        "{}",
        calls[0].user
    );
    assert_eq!(
        calls[0].user.lines().last(),
        Some("Write the suggestions in English."),
        "the person's language, named last"
    );
    let rows = rows(&w, t.id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, OutboxStatus::Delivered);
}

#[tokio::test]
async fn with_no_suggestions_task_nobody_is_asked_and_no_row_is_written() {
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
    let ev = events(&app, &alice(), t.id).await;
    assert!(suggested(&ev).is_empty());
    assert!(
        ev.iter().all(|e| e.kind() != EventKind::Error),
        "nothing failed: {:?}",
        shape(&ev)
    );
    // the ledger counted the ask all the same: a deployment that switches the task on later asks
    // at the end of the next job
    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert!(got.job.suggestions.may_ask(2));
    assert!(!got.job.suggestions.may_ask(1));
}

#[tokio::test]
async fn a_job_asks_once_and_a_later_job_asks_again_from_the_last_exchange_only() {
    let w = World::new();
    w.model
        .then_answer("Ask about the redirect")
        .then_answer("Run the second test");
    let app = suggesting(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream the first question").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the first suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
    })
    .await;
    quiet(&w, t.id).await;

    // a second job asks again, and the request shows only its own exchange
    app.post_message(&alice(), t.id, "stream the second question".to_owned())
        .await
        .unwrap();
    eventually("the second suggestions", || async {
        (suggested(&events(&app, &alice(), t.id).await).len() == 2).then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    let calls = w.model.calls();
    assert_eq!(calls.len(), 2, "one ask per job");
    assert!(
        calls[1].user.contains("user: stream the second question"),
        "{}",
        calls[1].user
    );
    assert!(
        !calls[1].user.contains("stream the first question"),
        "the whole log is never shown: {}",
        calls[1].user
    );
    assert!(
        !calls[1].user.contains("Ask about the redirect"),
        "an earlier suggestion is not part of the conversation: {}",
        calls[1].user
    );
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        suggested(&ev),
        [
            (1, vec!["Ask about the redirect"]),
            (2, vec!["Run the second test"]),
        ]
    );
    assert_eq!(rows(&w, t.id).len(), 2, "one row per job");
}

#[tokio::test]
async fn a_job_that_blocks_and_then_ends_asks_once() {
    let w = World::new();
    w.model.then_answer("Try the main branch");
    let app = suggesting(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "ask which branch").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    eventually("the first suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
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
    run.shutdown().await;
    assert_eq!(
        w.model.calls().len(),
        1,
        "a job that blocks, is answered and ends asks once"
    );
    assert_eq!(rows(&w, t.id).len(), 1);
    assert_eq!(suggested(&events(&app, &alice(), t.id).await).len(), 1);
}

#[tokio::test]
async fn the_model_answers_and_the_person_is_only_data() {
    let w = World::new();
    // the model echoes the person's message back as a "suggestion"
    w.model.then_answer("stream hello there");
    let app = suggesting(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello there").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;
    // the model's words are data and are logged as they are; what the orchestrator never does is
    // *invent* a suggestion out of the person's message
    let ev = events(&app, &alice(), t.id).await;
    let got = suggested(&ev);
    assert_eq!(got[0].1, ["stream hello there"]);
    assert_eq!(w.model.calls().len(), 1, "the model was asked once");
    let call = &w.model.calls()[0];
    assert!(
        !call.system.contains("stream hello there"),
        "the person's words are data in the request, never guidance: {}",
        call.system
    );
}

#[tokio::test]
async fn a_model_that_fails_costs_the_thread_nothing() {
    let w = World::new();
    // three tries on a transient failure, then the row is declined
    w.model
        .then(ModelStep::Unreachable)
        .then(ModelStep::RateLimited)
        .then(ModelStep::Nonsense);
    let app = suggesting(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the row to end", || async {
        (rows(&w, t.id).len() == 1 && rows(&w, t.id)[0].status == OutboxStatus::Delivered)
            .then_some(())
    })
    .await;
    assert_eq!(w.model.calls().len(), 3);
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
    quiet(&w, t.id).await;
    run.shutdown().await;
    let ev = events(&app, &alice(), t.id).await;
    assert!(suggested(&ev).is_empty());
    assert!(
        ev.iter().all(|e| e.kind() != EventKind::Error),
        "nothing visible failed: {:?}",
        shape(&ev)
    );
    assert_eq!(state_of(&w, t.id).await, ThreadState::Done);
}

#[tokio::test]
async fn a_model_that_never_answers_is_given_up_on_after_the_tasks_timeout() {
    let w = World::new();
    w.model
        .then(ModelStep::Hang)
        .then(ModelStep::Hang)
        .then(ModelStep::Hang);
    let app = suggesting_with(
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
    let ev = events(&app, &alice(), t.id).await;
    assert!(suggested(&ev).is_empty());
    assert!(ev.iter().all(|e| e.kind() != EventKind::Error));
    assert_eq!(state_of(&w, t.id).await, ThreadState::Done);
}

#[tokio::test]
async fn suggestions_in_a_script_the_language_rule_refuses_are_asked_again_once() {
    let w = World::new();
    // the person writes English; the model drifts into Chinese the first time
    w.model.then_answer("运行测试").then_answer("Run the tests");
    let app = suggesting(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(
        &app,
        &alice(),
        "plain",
        "stream hello there, how are you today",
    )
    .await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    let calls = w.model.calls();
    assert_eq!(calls.len(), 2, "asked, asked again");
    for call in &calls {
        assert_eq!(
            call.user.lines().last(),
            Some("Write the suggestions in English.")
        );
    }
    assert!(
        calls[1]
            .user
            .contains("Your last suggestion was in a script the person did not write in."),
        "{}",
        calls[1].user
    );
    assert!(!calls[0].user.contains("Your last suggestion"));
    assert_eq!(
        suggested(&events(&app, &alice(), t.id).await),
        [(1, vec!["Run the tests"])]
    );
    // wrong twice: the thread keeps no suggestions
    w.model.then_answer("再次运行").then_answer("还是中文");
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
        suggested(&events(&app, &alice(), t.id).await),
        [(1, vec!["Run the tests"])],
        "the second job offered nothing"
    );
}

#[tokio::test]
async fn the_configured_guidance_language_and_tokens_reach_the_request() {
    let w = World::new();
    w.model.then_answer("Continuer avec la deuxième question");
    let app = suggesting_with(
        &w,
        TaskSettings {
            guidance: Some("Say it as a haiku.".to_owned()),
            language: LanguageRule::Fixed(Lang::French),
            max_tokens: 99,
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
    eventually("the suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
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
        call.system
            .contains("Answer with at most three suggestions"),
        "the form of the answer is the core's whatever the guidance says: {}",
        call.system
    );
    assert_eq!(call.max_tokens, 99);
    assert_eq!(
        call.user.lines().last(),
        Some("Write the suggestions in French."),
        "a fixed language, whatever the person wrote"
    );
    assert_eq!(
        suggested(&events(&app, &alice(), t.id).await),
        [(1, vec!["Continuer avec la deuxième question"])]
    );
}

#[tokio::test]
async fn a_suggestion_the_model_forges_cannot_be_submitted_by_a_person() {
    let w = World::new();
    let app = suggesting(&w);
    let t = create(&app, &alice(), "plain", "stream hello").await;
    for input in [
        orch_core::Input::Suggested {
            job: 1,
            suggestions: vec!["Forged".to_owned()],
        },
        orch_core::Input::SuggestionsDeclined { job: 1 },
    ] {
        let err = app.submit(&alice(), t.id, input, None).await.unwrap_err();
        assert!(matches!(err, orch_app::AppError::Invalid(_)), "{err:?}");
    }
    assert!(suggested(&events(&app, &alice(), t.id).await).is_empty());
}

#[tokio::test]
async fn a_long_job_still_carries_the_persons_message() {
    let w = World::new();
    w.model.then_answer("Run the tests");
    let app = suggesting(&w);
    let t = create(&app, &alice(), "plain", "slow please").await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the agent to be sent the message", || async {
        (!w.agent.sends().is_empty()).then_some(())
    })
    .await;
    // A job of tool calls is many more events than the old 128-event window: the person's message
    // must still be in the tail the worker reads.
    for n in 0..200 {
        app.apply(
            t.id,
            Input::Agent {
                agent: AgentId::new("plain"),
                revision: None,
                update: AgentUpdate::Status {
                    state: AgentTaskState::Working,
                    detail: Some(format!("step {n}")),
                    tier: None,
                },
            },
            Some(format!("s{n}")),
            None,
            None,
        )
        .await
        .unwrap();
    }
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Status {
                state: AgentTaskState::Completed,
                detail: Some("done".to_owned()),
                tier: None,
            },
        },
        Some("done".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the suggestions", || async {
        (!suggested(&events(&app, &alice(), t.id).await).is_empty()).then_some(())
    })
    .await;
    run.shutdown().await;
    let calls = w.model.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].user.contains("user: slow please"),
        "the person's message is in the window: {}",
        calls[0].user
    );
}

#[tokio::test]
async fn a_row_whose_thread_moved_on_declines_without_asking() {
    let w = World::new();
    w.model.then_answer("Never used");
    let app = suggesting(&w);
    let t = create(&app, &alice(), "plain", "slow please").await;
    // The job ends and its suggestions row waits; the person writes before it is worked.
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Status {
                state: AgentTaskState::Completed,
                detail: Some("done".to_owned()),
                tier: None,
            },
        },
        Some("done".to_owned()),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(rows(&w, t.id).len(), 1, "the job's end asked");
    app.post_message(&alice(), t.id, "slow again".to_owned())
        .await
        .unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the row to end", || async {
        (rows(&w, t.id)[0].status == OutboxStatus::Delivered).then_some(())
    })
    .await;
    run.shutdown().await;
    assert!(
        w.model.calls().is_empty(),
        "the thread is no longer done or blocked: no model is asked"
    );
    assert!(suggested(&events(&app, &alice(), t.id).await).is_empty());
}
