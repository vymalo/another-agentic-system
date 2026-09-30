//! The thread service without a dispatcher: validation, isolation, streams, idempotency.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use futures::StreamExt;
use orch_app::{AppError, ApplyOutcome, NewThread};
use orch_core::{
    AgentId, AgentTarget, AgentTaskState, AgentUpdate, Classify, ErrorClass, EventKind, Input,
    ThreadId, ThreadState,
};
use orch_ports::{StoreError, ThreadStore};
use support::*;
use uuid::Uuid;

fn new_thread(agent: &str, release: Option<&str>, text: &str) -> NewThread {
    NewThread {
        title: None,
        target: AgentTarget {
            agent_id: AgentId::new(agent),
            release: release.map(str::to_owned),
        },
        text: text.to_owned(),
    }
}

fn invalid(err: AppError) -> String {
    match err {
        AppError::Invalid(m) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[tokio::test]
async fn create_thread_writes_the_first_message_and_queues_the_delegation() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "  Fix the login bug\nsecond line").await;
    assert_eq!(t.state, ThreadState::Queued);
    assert_eq!(t.title, "Fix the login bug");
    assert_eq!(t.last_seq, 1);
    assert_eq!(t.owner, alice());
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev), ["user_message"]);
    assert_eq!(ev[0].actor.name, "alice@example.com");
    assert_eq!(w.store.list_open_outbox(t.id).await.unwrap().len(), 1);
    let binding = w.store.get_binding(t.id).await.unwrap().unwrap();
    assert_eq!(binding.context_id, t.id.to_string());
}

#[tokio::test]
async fn the_default_title_is_cut_at_80_characters_on_a_char_boundary() {
    let w = World::new();
    let app = w.app();
    let text = "é".repeat(200);
    let t = create(&app, &alice(), "plain", &text).await;
    assert_eq!(t.title.chars().count(), 80);
    let explicit = app
        .create_thread(
            &alice(),
            NewThread {
                title: Some("Mine".into()),
                ..new_thread("plain", None, "x")
            },
        )
        .await
        .unwrap();
    assert_eq!(explicit.title, "Mine");
}

#[tokio::test]
async fn validation() {
    let w = World::new();
    let app = w.app();
    let u = alice();
    assert!(
        invalid(
            app.create_thread(&u, new_thread("plain", None, ""))
                .await
                .unwrap_err()
        )
        .contains("empty")
    );
    assert!(
        invalid(
            app.create_thread(&u, new_thread("plain", None, "  \n\t"))
                .await
                .unwrap_err()
        )
        .contains("empty")
    );
    assert!(
        invalid(
            app.create_thread(&u, new_thread("plain", None, &"x".repeat(100_001)))
                .await
                .unwrap_err()
        )
        .contains("100000")
    );
    app.create_thread(&u, new_thread("plain", None, &"x".repeat(100_000)))
        .await
        .unwrap();
    assert!(
        invalid(
            app.create_thread(&u, new_thread("nope", None, "hi"))
                .await
                .unwrap_err()
        )
        .contains("unknown agent")
    );
    let long_title = NewThread {
        title: Some("t".repeat(201)),
        ..new_thread("plain", None, "hi")
    };
    assert!(invalid(app.create_thread(&u, long_title).await.unwrap_err()).contains("title"));
    // Releases: only on agents whose live card offers them, and only known ones.
    assert!(
        invalid(
            app.create_thread(&u, new_thread("plain", Some("stable"), "hi"))
                .await
                .unwrap_err()
        )
        .contains("does not offer releases")
    );
    assert!(
        invalid(
            app.create_thread(&u, new_thread("coder", Some("nightly"), "hi"))
                .await
                .unwrap_err()
        )
        .contains("unknown release")
    );
    app.create_thread(&u, new_thread("coder", Some("staging"), "hi"))
        .await
        .unwrap();
    app.create_thread(&u, new_thread("coder", Some("rev-1"), "hi"))
        .await
        .unwrap();
    // Fail closed: an unreachable card cannot validate a release.
    w.agent.set_card_down("coder", true);
    let err = app
        .create_thread(&u, new_thread("coder", Some("staging"), "hi"))
        .await
        .unwrap_err();
    match &err {
        AppError::Upstream { agent, source } => {
            assert_eq!(agent.as_str(), "coder");
            assert!(source.is_retryable(), "{source:?}");
        }
        other => panic!("expected Upstream, got {other:?}"),
    }
    assert_eq!(err.class(), ErrorClass::Transient);
    // ... but a thread without a release does not need the card.
    app.create_thread(&u, new_thread("coder", None, "hi"))
        .await
        .unwrap();
}

#[tokio::test]
async fn list_agents_reads_live_cards_and_fails_closed() {
    let w = World::new();
    let app = w.app();
    let agents = app.list_agents().await;
    assert_eq!(
        agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["coder", "plain"]
    );
    assert_eq!(agents[0].name, "Coder");
    assert_eq!(
        agents[0].releases.as_ref().unwrap().default_channel,
        "stable"
    );
    assert!(agents[1].releases.is_none());
    w.agent.set_card_down("coder", true);
    let agents = app.list_agents().await;
    assert_eq!(
        agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["coder", "plain"],
        "an agent whose card is down keeps its place: the first stays the default"
    );
    assert!(
        agents[0].releases.is_none(),
        "no picker when the card cannot be read"
    );
    assert!(agents[0].description.is_none());
    // Live, never cached: back up, back to offering releases.
    w.agent.set_card_down("coder", false);
    assert!(app.list_agents().await[0].releases.is_some());
}

#[tokio::test]
async fn describe_agent_is_one_agent_with_its_live_card() {
    let w = World::new();
    let app = w.app();
    let coder = app.describe_agent(&AgentId::new("coder")).await.unwrap();
    assert_eq!(coder.name, "Coder");
    let card = coder.card.unwrap();
    assert_eq!(card.version.as_deref(), Some("1.0.0"));
    assert_eq!(card.releases.unwrap().default_channel, "stable");
    assert!(
        app.describe_agent(&AgentId::new("nobody")).await.is_none(),
        "not configured"
    );
    // Fail closed, live, never cached.
    w.agent.set_card_down("coder", true);
    let down = app.describe_agent(&AgentId::new("coder")).await.unwrap();
    assert_eq!(down.name, "Coder");
    assert!(down.card.is_none());
    w.agent.set_card_down("coder", false);
    assert!(
        app.describe_agent(&AgentId::new("coder"))
            .await
            .unwrap()
            .card
            .is_some()
    );
}

#[tokio::test]
async fn other_users_threads_are_not_found() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "secret").await;
    assert!(matches!(
        app.get_thread(&bob(), t.id).await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        app.list_events(&bob(), t.id, 0, 10).await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        app.post_message(&bob(), t.id, "x".into()).await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        app.cancel(&bob(), t.id).await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        app.event_stream(&bob(), t.id, 0).await,
        Err(AppError::NotFound)
    ));
    assert!(app.list_threads(&bob(), None, 50).await.unwrap().is_empty());
    let random = ThreadId(Uuid::from_u128(5));
    assert!(matches!(
        app.get_thread(&alice(), random).await,
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
async fn threads_list_newest_first_with_a_cursor() {
    let w = World::new();
    let app = w.app();
    let a = create(&app, &alice(), "plain", "a").await;
    let b = create(&app, &alice(), "plain", "b").await;
    let c = create(&app, &alice(), "plain", "c").await;
    let all = app.list_threads(&alice(), None, 50).await.unwrap();
    assert_eq!(
        all.iter().map(|t| t.id).collect::<Vec<_>>(),
        [c.id, b.id, a.id]
    );
    let page = app.list_threads(&alice(), Some(c.id), 1).await.unwrap();
    assert_eq!(page.iter().map(|t| t.id).collect::<Vec<_>>(), [b.id]);
}

#[tokio::test]
async fn post_message_validates_and_rejects_finished_threads() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "hi").await;
    assert!(matches!(
        app.post_message(&alice(), t.id, " ".into()).await,
        Err(AppError::Invalid(_))
    ));
    let ev = app
        .post_message(&alice(), t.id, "more".into())
        .await
        .unwrap();
    assert_eq!((ev.seq, ev.kind()), (2, EventKind::UserMessage));
    // Two delegations are now queued, in order.
    assert_eq!(w.store.list_open_outbox(t.id).await.unwrap().len(), 2);
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Status {
                state: AgentTaskState::Completed,
                detail: None,
            },
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        app.post_message(&alice(), t.id, "late".into()).await,
        Err(AppError::Finished)
    ));
}

#[tokio::test]
async fn apply_is_idempotent_per_key_and_persists_the_binding() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "hi").await;
    let status = |state| Input::Agent {
        agent: AgentId::new("plain"),
        revision: Some("r1".into()),
        update: AgentUpdate::Status {
            state,
            detail: None,
        },
    };
    let binding = || orch_ports::BindingUpdate {
        task_id: Some("task-9".into()),
        task_state: Some(AgentTaskState::Working),
        revision: Some("r1".into()),
    };
    let first = app
        .apply(
            t.id,
            status(AgentTaskState::Working),
            Some("k".into()),
            Some(binding()),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(first, ApplyOutcome::Applied { ref events, .. } if events.len() == 1));
    let replay = app
        .apply(
            t.id,
            status(AgentTaskState::Working),
            Some("k".into()),
            Some(binding()),
            None,
        )
        .await
        .unwrap();
    // Either suppressed by the state machine (no events) or a store-level duplicate; never a second event.
    match replay {
        ApplyOutcome::Applied { events, .. } => assert!(events.is_empty()),
        ApplyOutcome::Duplicate => {}
        ApplyOutcome::Fenced => panic!("no lease was given"),
    }
    let done = app
        .apply(
            t.id,
            status(AgentTaskState::Completed),
            Some("c".into()),
            None,
            None,
        )
        .await
        .unwrap();
    let ApplyOutcome::Applied { events, thread } = done else {
        panic!()
    };
    assert_eq!(events.len(), 2);
    assert_eq!(thread.state, ThreadState::Done);
    // Replaying the completion key: the thread is finished, so the state machine says no.
    let late = app
        .apply(
            t.id,
            status(AgentTaskState::Completed),
            Some("c".into()),
            None,
            None,
        )
        .await;
    assert!(matches!(late, Err(AppError::Transition(_))));
    let b = w.store.get_binding(t.id).await.unwrap().unwrap();
    assert_eq!(b.task_id.as_deref(), Some("task-9"));
    assert_eq!(b.revision.as_deref(), Some("r1"));
    let ev = events_after(&app, t.id).await;
    assert_eq!(
        ev,
        [
            "user_message",
            "agent_status:working",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
}

async fn events_after(app: &TestApp, id: ThreadId) -> Vec<String> {
    shape(&events(app, &alice(), id).await)
}

#[tokio::test]
async fn the_event_stream_replays_then_goes_live_without_gaps_or_duplicates() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "hi").await;
    for i in 0..3 {
        app.post_message(&alice(), t.id, format!("m{i}"))
            .await
            .unwrap();
    }
    // Replay from seq 2, then live.
    let mut stream = app.event_stream(&alice(), t.id, 1).await.unwrap();
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(stream.next().await.unwrap().seq);
    }
    assert_eq!(seen, [2, 3, 4]);
    let app2 = app.clone();
    let id = t.id;
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        for i in 0..5 {
            app2.post_message(&alice(), id, format!("live{i}"))
                .await
                .unwrap();
        }
    });
    for want in 5..=9 {
        let ev = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.seq, want);
    }
    writer.await.unwrap();
    // Nothing more is pending.
    assert!(
        tokio::time::timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_event_stream_survives_a_missing_wakeup_via_the_safety_poll() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "hi").await;
    let mut stream = app.event_stream(&alice(), t.id, 1).await.unwrap();
    // Write straight to the store: no wakeup is sent at all.
    let record = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    w.store
        .commit(
            t.id,
            record.version,
            orch_ports::Commit {
                new_state: ThreadState::Queued,
                job: None,
                events: vec![orch_ports::NewEvent {
                    at: jiff::Timestamp::now(),
                    actor: orch_core::Actor::system(),
                    body: orch_core::EventBody::Error(orch_core::ErrorData {
                        message: "quiet".into(),
                        retryable: false,
                    }),
                    idempotency_key: None,
                }],
                outbox: vec![],
                binding: None,
                now: jiff::Timestamp::now(),
                lease: None,
                watches: vec![],
                timers: vec![],
                inbox: None,
            },
        )
        .await
        .unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ev.seq, 2);
}

#[tokio::test]
async fn the_event_stream_ends_once_caught_up_when_shutdown_started() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "hi").await;
    let mut stream = app.event_stream(&alice(), t.id, 0).await.unwrap();
    app.set_shutting_down();
    // Buffered history is still delivered, then the stream ends instead of waiting for more.
    assert_eq!(stream.next().await.unwrap().seq, 1);
    let end = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the stream must end within a poll interval");
    assert!(end.is_none());
}

#[tokio::test]
async fn a_thread_that_keeps_changing_under_the_commit_loop_is_contended() {
    let w = World::new();
    let app = w.app();
    let u = alice();
    let t = create(&app, &u, "plain", "hi").await;
    // Every attempt of the optimistic loop loses its race: after `max_commit_attempts` the
    // caller is told to try again, which is not an internal error.
    w.store
        .fail_next_commits(100, || StoreError::VersionConflict);
    let err = app.cancel(&u, t.id).await.unwrap_err();
    assert!(matches!(err, AppError::Contended), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Conflict);
    assert!(err.is_retryable());
}

#[tokio::test]
async fn a_store_outage_is_transient_and_keeps_its_source() {
    let w = World::new();
    let app = w.app();
    let u = alice();
    let t = create(&app, &u, "plain", "hi").await;
    w.store.fail_next_commits(1, || {
        StoreError::unavailable(std::io::Error::other("pool timed out"))
    });
    let err = app.cancel(&u, t.id).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient);
    assert_eq!(err.to_string(), "store unavailable");
    assert_eq!(orch_core::report(&err), "store unavailable: pool timed out");
}

// ---- consumer-chosen thread ids (a surface where the consumer mints them) -----------------

fn free_id() -> ThreadId {
    ThreadId(Uuid::now_v7())
}

fn inbound(message: &str, run: &str) -> orch_app::Inbound {
    orch_app::Inbound {
        message_id: Some(message.to_owned()),
        run_id: Some(run.to_owned()),
        key: Some(format!("k:{message}")),
        ..orch_app::Inbound::default()
    }
}

#[tokio::test]
async fn find_thread_tells_mine_from_free_from_someone_elses() {
    let w = World::new();
    let app = w.app();
    let mine = create(&app, &alice(), "plain", "echo mine").await;
    assert_eq!(
        app.find_thread(&alice(), mine.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        mine.id
    );
    assert!(
        app.find_thread(&alice(), free_id())
            .await
            .unwrap()
            .is_none()
    );
    // Someone else's thread is the same refusal as a thread that is not there for the caller.
    assert!(matches!(
        app.find_thread(&bob(), mine.id).await,
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
async fn create_thread_as_uses_the_callers_id_and_records_what_the_surface_says() {
    let w = World::new();
    let app = w.app();
    let id = free_id();
    let created = app
        .create_thread_as(
            &alice(),
            id,
            new_thread("plain", None, "echo hi"),
            inbound("m-1", "r-1"),
        )
        .await
        .unwrap();
    let orch_app::Creation::Created { thread, events } = created else {
        panic!("not created");
    };
    assert_eq!(thread.id, id);
    assert_eq!(thread.owner, alice());
    assert_eq!(events.len(), 1);
    let v = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(v["data"]["messageId"], "m-1");
    assert_eq!(v["data"]["runId"], "r-1");
    assert_eq!(
        w.store.get_binding(id).await.unwrap().unwrap().context_id,
        id.to_string()
    );
    assert_eq!(w.store.list_open_outbox(id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn create_thread_as_on_a_taken_id_is_not_found_for_a_stranger_and_exists_for_the_owner() {
    let w = World::new();
    let app = w.app();
    let mine = create(&app, &alice(), "plain", "echo mine").await;
    let before = events(&app, &alice(), mine.id).await;
    // A collision with someone else's thread does not reveal it.
    let stranger = app
        .create_thread_as(
            &bob(),
            mine.id,
            new_thread("plain", None, "echo yours"),
            orch_app::Inbound::default(),
        )
        .await;
    assert!(matches!(stranger, Err(AppError::NotFound)), "{stranger:?}");
    // The owner racing with itself learns the thread is there.
    let again = app
        .create_thread_as(
            &alice(),
            mine.id,
            new_thread("plain", None, "echo mine"),
            orch_app::Inbound::default(),
        )
        .await
        .unwrap();
    assert_eq!(again, orch_app::Creation::Exists);
    assert_eq!(
        events(&app, &alice(), mine.id).await,
        before,
        "nothing written"
    );
}

#[tokio::test]
async fn create_thread_as_still_reports_a_real_store_failure() {
    let w = World::new();
    let app = w.app();
    w.store
        .fail_next_creates(1, || StoreError::unavailable(std::io::Error::other("down")));
    let err = app
        .create_thread_as(
            &alice(),
            free_id(),
            new_thread("plain", None, "echo hi"),
            orch_app::Inbound::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient, "{err:?}");
}

#[tokio::test]
async fn submit_applies_under_a_key_and_a_replay_is_a_duplicate() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "ask me").await;
    let answer = || Input::UserMessage {
        user: alice(),
        text: "main".to_owned(),
        message_id: Some("m-2".to_owned()),
        run_id: Some("r-2".to_owned()),
    };
    // Not yet blocked, but a user message is valid while working, so it is applied.
    let first = app
        .submit(&alice(), t.id, answer(), Some("k:m-2".to_owned()))
        .await
        .unwrap();
    let ApplyOutcome::Applied { events, .. } = first else {
        panic!("not applied");
    };
    assert_eq!(events[0].kind(), EventKind::UserMessage);
    let again = app
        .submit(&alice(), t.id, answer(), Some("k:m-2".to_owned()))
        .await
        .unwrap();
    assert_eq!(again, ApplyOutcome::Duplicate);
    assert_eq!(
        kinds(&events_of(&app, t.id).await),
        [EventKind::UserMessage; 2]
    );
}

async fn events_of(app: &TestApp, id: ThreadId) -> Vec<orch_core::Event> {
    events(app, &alice(), id).await
}

#[tokio::test]
async fn submit_validates_the_text_and_checks_the_owner() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo hi").await;
    let message = |text: &str| Input::UserMessage {
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
    };
    assert!(
        invalid(
            app.submit(&alice(), t.id, message("  "), None)
                .await
                .unwrap_err()
        )
        .contains("empty")
    );
    let long = "x".repeat(100_001);
    assert!(
        invalid(
            app.submit(&alice(), t.id, message(&long), None)
                .await
                .unwrap_err()
        )
        .contains("at most")
    );
    assert!(matches!(
        app.submit(&bob(), t.id, message("hi"), None).await,
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        app.submit(&alice(), free_id(), message("hi"), None).await,
        Err(AppError::NotFound)
    ));
    assert_eq!(events_of(&app, t.id).await.len(), 1, "nothing was written");
}
