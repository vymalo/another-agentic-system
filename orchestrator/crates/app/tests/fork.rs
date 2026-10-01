//! Forking a thread through the application (ADR 0029): the parent's events up to a cut are copied,
//! then `thread_forked`; an edit also carries the replacing message and its delegation; the fork
//! has its own context, the parent's title, and the parent is untouched. A cut the thread does not
//! allow, a text or a target that cannot be used, an id that is taken and a family with no room
//! write nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, ForkAt, ForkRequest};
use orch_core::{
    Event, EventBody, EventKind, ForkError, ForkKind, MAX_FORK_FAMILY, ThreadId, ThreadRecord,
    ThreadState, TitleSource,
};
use orch_ports::{OutboxKind, OutboxPayload, ThreadStore};
use support::*;

/// A thread of two finished turns: `echo one`, then `echo two`.
async fn two_turns(w: &World, app: &std::sync::Arc<TestApp>) -> ThreadRecord {
    let run = spawn_dispatcher(app, fast(), "d1");
    let t = create(app, &alice(), "plain", "echo one").await;
    wait_state(app, &alice(), t.id, ThreadState::Done).await;
    app.post_message(&alice(), t.id, "echo two".to_owned())
        .await
        .unwrap();
    eventually("the second turn to finish", || async {
        let ev = events(app, &alice(), t.id).await;
        (ev.iter()
            .filter(|e| e.kind() == EventKind::UserMessage)
            .count()
            == 2
            && state_of(w, t.id).await == ThreadState::Done)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    app.get_thread(&alice(), t.id).await.unwrap()
}

fn after(seq: i64) -> ForkRequest {
    ForkRequest {
        at: ForkAt::AfterTurn { seq },
        target: None,
        id: None,
    }
}

fn replace(seq: i64, text: &str) -> ForkRequest {
    ForkRequest {
        at: ForkAt::Replace {
            seq,
            text: text.to_owned(),
            message_id: Some("m-edit".to_owned()),
        },
        target: None,
        id: None,
    }
}

fn seq_of_nth_message(events: &[Event], n: usize) -> i64 {
    events
        .iter()
        .filter(|e| e.kind() == EventKind::UserMessage)
        .nth(n)
        .unwrap()
        .seq
}

#[tokio::test]
async fn a_fork_copies_a_turn_and_is_a_finished_thread_of_its_own() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let log = events(&app, &alice(), parent.id).await;
    let second = seq_of_nth_message(&log, 1);

    let forked = app
        .fork_thread(&alice(), parent.id, after(1))
        .await
        .unwrap();
    assert!(forked.created);
    let fork = forked.thread;
    assert_ne!(fork.id, parent.id);
    assert_eq!(fork.state, ThreadState::Done);
    assert_eq!(fork.title, parent.title);
    assert_eq!(fork.target, parent.target);
    assert_eq!(fork.owner, alice());
    let origin = fork.forked_from.unwrap();
    assert_eq!(
        (origin.thread_id, origin.seq, origin.kind),
        (Some(parent.id), second - 1, ForkKind::Fork)
    );

    // the first turn as it was, then the event of the fork
    let flog = events(&app, &alice(), fork.id).await;
    assert_contiguous(&flog);
    assert_eq!(flog.len(), usize::try_from(second).unwrap());
    for (copy, original) in flog
        .iter()
        .zip(&log)
        .take(usize::try_from(second - 1).unwrap())
    {
        assert_eq!(
            (copy.seq, copy.at, &copy.actor, &copy.body),
            (original.seq, original.at, &original.actor, &original.body)
        );
    }
    let EventBody::ThreadForked(data) = &flog.last().unwrap().body else {
        panic!("{:?}", flog.last())
    };
    assert_eq!(
        (data.from.thread_id, data.from.seq, data.kind),
        (parent.id, second - 1, ForkKind::Fork)
    );
    assert_eq!(data.title, parent.title);
    assert_eq!(data.target, parent.target);
    assert_eq!(flog.last().unwrap().actor.name, "alice@example.com");

    // its own A2A context, and nothing asked of an agent
    let binding = w.store.get_binding(fork.id).await.unwrap().unwrap();
    assert_eq!(binding.context_id, fork.id.to_string());
    assert_ne!(binding.context_id, parent.id.to_string());
    assert!(w.store.outbox_of(fork.id).is_empty());

    // the parent did not change
    assert_eq!(app.get_thread(&alice(), parent.id).await.unwrap(), parent);
    assert_eq!(events(&app, &alice(), parent.id).await, log);

    // both are in the list
    let listed = app.list_threads(&alice(), None, 10, false).await.unwrap();
    assert_eq!(
        listed.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![fork.id, parent.id]
    );
    // the export says where it came from
    let export = app.export_thread(&alice(), fork.id).await.unwrap();
    assert_eq!(export.thread.forked_from, fork.forked_from);
}

#[tokio::test]
async fn the_next_message_of_a_fork_starts_the_next_job_in_its_own_context() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let parent_log = events(&app, &alice(), parent.id).await;
    let fork = app
        .fork_thread(&alice(), parent.id, after(1))
        .await
        .unwrap()
        .thread;
    let run = spawn_dispatcher(&app, fast(), "d1");
    app.post_message(&alice(), fork.id, "echo three".to_owned())
        .await
        .unwrap();
    eventually("the fork's turn to finish", || async {
        let ev = events(&app, &alice(), fork.id).await;
        (ev.iter().any(|e| e.kind() == EventKind::JobStarted)
            && state_of(&w, fork.id).await == ThreadState::Done)
            .then_some(())
    })
    .await;
    run.shutdown().await;
    let flog = events(&app, &alice(), fork.id).await;
    assert_contiguous(&flog);
    let started: Vec<u32> = flog
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::JobStarted(d) => Some(d.job),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        [2],
        "the parent's first job was copied: this is the next"
    );
    // the parent heard nothing of it
    assert_eq!(events(&app, &alice(), parent.id).await, parent_log);
    let binding = w.store.get_binding(fork.id).await.unwrap().unwrap();
    assert_eq!(binding.context_id, fork.id.to_string());
    assert!(binding.task_id.is_some());
}

#[tokio::test]
async fn a_turn_that_is_going_on_cannot_be_forked() {
    let w = World::new();
    let app = w.app();
    // no dispatcher: the thread stays queued
    let t = create(&app, &alice(), "plain", "echo one").await;
    let err = app.fork_thread(&alice(), t.id, after(1)).await.unwrap_err();
    assert!(
        matches!(err, AppError::Fork(ForkError::TurnOpen)),
        "{err:?}"
    );
    assert_eq!(
        orch_core::Classify::class(&err),
        orch_core::ErrorClass::Rejected
    );
    assert_eq!(
        app.list_threads(&alice(), None, 10, true)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn an_edit_is_a_branch_with_its_message_and_its_delegation() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let log = events(&app, &alice(), parent.id).await;
    let second = seq_of_nth_message(&log, 1);

    let forked = app
        .fork_thread(&alice(), parent.id, replace(second, "echo instead"))
        .await
        .unwrap();
    let fork = forked.thread;
    assert_eq!(fork.state, ThreadState::Queued);
    assert_eq!(fork.forked_from.unwrap().kind, ForkKind::Edit);
    assert_eq!(fork.forked_from.unwrap().seq, second - 1);

    let flog = events(&app, &alice(), fork.id).await;
    assert_contiguous(&flog);
    let tail: Vec<EventKind> = flog[usize::try_from(second - 1).unwrap()..]
        .iter()
        .map(Event::kind)
        .collect();
    assert_eq!(
        tail,
        [
            EventKind::ThreadForked,
            EventKind::UserMessage,
            EventKind::JobStarted
        ]
    );
    let EventBody::UserMessage(m) = &flog[usize::try_from(second).unwrap()].body else {
        panic!()
    };
    assert_eq!(
        (m.text.as_str(), m.message_id.as_deref()),
        ("echo instead", Some("m-edit"))
    );
    // one delegation, for the fork's own thread, which opens a new task
    let rows = w.store.outbox_of(fork.id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, OutboxKind::Delegate);
    assert!(matches!(
        &rows[0].payload,
        OutboxPayload::Delegate { text, new_job: true, .. } if text == "echo instead"
    ));
    assert!(
        w.store
            .outbox_of(parent.id)
            .iter()
            .all(|r| !r.status.is_open())
    );

    // the dispatcher answers it like any message
    let run = spawn_dispatcher(&app, fast(), "d2");
    wait_state(&app, &alice(), fork.id, ThreadState::Done).await;
    run.shutdown().await;
    let flog = events(&app, &alice(), fork.id).await;
    assert!(flog.iter().any(|e| matches!(
        &e.body,
        EventBody::Artifact(a) if a.text.as_deref() == Some("echo: echo instead")
    )));

    // a branch is not listed unless asked for, and the two are versions of one message
    let hidden = app.list_threads(&alice(), None, 10, false).await.unwrap();
    assert_eq!(
        hidden.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![parent.id]
    );
    let all = app.list_threads(&alice(), None, 10, true).await.unwrap();
    assert_eq!(all.len(), 2);
    let own = app.branches(&alice(), fork.id).await.unwrap();
    assert_eq!(own.root, parent.id);
    assert_eq!(own.points.len(), 1);
    let point = &own.points[0];
    assert_eq!((point.index, point.siblings.len()), (1, 2));
    assert_eq!(
        point
            .siblings
            .iter()
            .map(|s| (s.thread_id, s.seq))
            .collect::<Vec<_>>(),
        vec![(parent.id, second), (fork.id, second + 1)]
    );
    let theirs = app.branches(&alice(), parent.id).await.unwrap();
    assert_eq!(theirs.root, parent.id);
    assert_eq!((theirs.points[0].seq, theirs.points[0].index), (second, 0));
    assert_eq!(theirs.points[0].siblings[0].title, parent.title);
}

#[tokio::test]
async fn the_first_message_can_be_edited_and_a_family_has_a_cap() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let first = app
        .fork_thread(&alice(), parent.id, replace(1, "echo from the start"))
        .await
        .unwrap()
        .thread;
    let flog = events(&app, &alice(), first.id).await;
    assert_eq!(
        flog.iter().map(Event::kind).collect::<Vec<_>>(),
        [
            EventKind::ThreadForked,
            EventKind::UserMessage,
            EventKind::JobStarted
        ]
    );
    // nothing was copied: nothing but the first message is its own
    assert_eq!(first.forked_from.unwrap().seq, 0);

    // a family has room for MAX_FORK_FAMILY threads
    for _ in 0..(MAX_FORK_FAMILY - 2) {
        app.fork_thread(&alice(), parent.id, replace(1, "echo again"))
            .await
            .unwrap();
    }
    let err = app
        .fork_thread(&alice(), parent.id, replace(1, "echo once more"))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Refused(_)), "{err:?}");
    // a fork of the other kind is not a branch and needs no room
    app.fork_thread(&alice(), parent.id, after(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_cut_the_thread_does_not_allow_writes_nothing() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let log = events(&app, &alice(), parent.id).await;
    let last = log.last().unwrap().seq;
    let agent_event = log
        .iter()
        .find(|e| e.kind() == EventKind::AgentStatus)
        .unwrap()
        .seq;

    for (req, want) in [
        (replace(agent_event, "x"), ForkError::NotAMessage),
        (replace(last + 1, "x"), ForkError::OutOfRange),
        (replace(0, "x"), ForkError::OutOfRange),
        (after(last + 1), ForkError::OutOfRange),
        (after(0), ForkError::OutOfRange),
        (after(-4), ForkError::OutOfRange),
    ] {
        let err = app
            .fork_thread(&alice(), parent.id, req.clone())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, AppError::Fork(e) if *e == want),
            "{req:?}: {err:?}"
        );
    }
    for text in ["", "   "] {
        let err = app
            .fork_thread(&alice(), parent.id, replace(1, text))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }
    let mut unknown = after(1);
    unknown.target = Some(target("nobody"));
    assert!(matches!(
        app.fork_thread(&alice(), parent.id, unknown)
            .await
            .unwrap_err(),
        AppError::Invalid(_)
    ));
    let mut release = after(1);
    release.target = Some(orch_core::AgentTarget {
        agent_id: orch_core::AgentId::new("plain"),
        release: Some("stable".to_owned()),
    });
    assert!(matches!(
        app.fork_thread(&alice(), parent.id, release)
            .await
            .unwrap_err(),
        AppError::Invalid(_)
    ));
    // somebody else's thread, and one that does not exist
    assert!(matches!(
        app.fork_thread(&bob(), parent.id, after(1))
            .await
            .unwrap_err(),
        AppError::NotFound
    ));
    let nothing = ThreadId(uuid::Uuid::from_u128(7));
    assert!(matches!(
        app.fork_thread(&alice(), nothing, after(1))
            .await
            .unwrap_err(),
        AppError::NotFound
    ));
    // none of it made a thread or touched the parent
    assert_eq!(
        app.list_threads(&alice(), None, 10, true)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        app.list_threads(&bob(), None, 10, true)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(app.get_thread(&alice(), parent.id).await.unwrap(), parent);
}

#[tokio::test]
async fn a_repeat_with_the_same_id_answers_the_fork_it_made() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let id = ThreadId(uuid::Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0042,
    ));
    let mut req = after(1);
    req.id = Some(id);
    let first = app
        .fork_thread(&alice(), parent.id, req.clone())
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(first.thread.id, id);
    let again = app
        .fork_thread(&alice(), parent.id, req.clone())
        .await
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.thread, first.thread);
    assert_eq!(
        events(&app, &alice(), id).await.len(),
        events(&app, &alice(), id).await.len()
    );
    assert_eq!(
        app.list_threads(&alice(), None, 10, true)
            .await
            .unwrap()
            .len(),
        2
    );

    // the id of another thread is taken: not this fork's
    let mut taken = after(1);
    taken.id = Some(parent.id);
    assert!(matches!(
        app.fork_thread(&alice(), parent.id, taken)
            .await
            .unwrap_err(),
        AppError::Refused(_)
    ));
    // and somebody else's looks like nothing
    let bobs = create(&app, &bob(), "plain", "echo hi").await;
    let mut theirs = after(1);
    theirs.id = Some(bobs.id);
    assert!(matches!(
        app.fork_thread(&alice(), parent.id, theirs)
            .await
            .unwrap_err(),
        AppError::NotFound
    ));
}

#[tokio::test]
async fn a_fork_keeps_the_parents_title_and_a_persons_rename_stays_theirs() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    app.rename_thread(&alice(), parent.id, "My chat")
        .await
        .unwrap();
    let fork = app
        .fork_thread(&alice(), parent.id, after(1))
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.title, "My chat");
    assert_eq!(fork.job.title.source(), TitleSource::User);
    let flog = events(&app, &alice(), fork.id).await;
    let EventBody::ThreadForked(data) = &flog.last().unwrap().body else {
        panic!()
    };
    assert_eq!(data.title, "My chat");

    // renaming the fork does not touch the parent
    app.rename_thread(&alice(), fork.id, "Another way")
        .await
        .unwrap();
    assert_eq!(
        app.get_thread(&alice(), parent.id).await.unwrap().title,
        "My chat"
    );
}

#[tokio::test]
async fn a_fork_can_continue_with_another_agent() {
    let w = World::new();
    let app = w.app();
    let parent = two_turns(&w, &app).await;
    let mut req = after(1);
    req.target = Some(target("coder"));
    let fork = app
        .fork_thread(&alice(), parent.id, req)
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.target, target("coder"));
    assert_eq!(parent.target, target("plain"));
    let binding = w.store.get_binding(fork.id).await.unwrap().unwrap();
    assert_eq!(binding.agent_id, orch_core::AgentId::new("coder"));
    let flog = events(&app, &alice(), fork.id).await;
    let EventBody::ThreadForked(data) = &flog.last().unwrap().body else {
        panic!()
    };
    assert_eq!(data.target, target("coder"));
}
