//! A thread's title through the application: a person renames it in any state, the title and the
//! `thread_titled` event are one commit, the rename survives what the agent does next, and a
//! rename that cannot be used writes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::AppError;
use orch_core::{EventBody, EventKind, Input, ThreadState, TitleSource, TitledBy};
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
    let listed = app.list_threads(&alice(), None, 10).await.unwrap();
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
