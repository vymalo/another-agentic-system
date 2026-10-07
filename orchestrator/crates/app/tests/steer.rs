//! Steering a running task (`steer/v1`, ADR 0036) through the application, the dispatcher and the
//! scripted agent, and open question 33 (a message the agent took as a task of its own while the
//! thread was ending).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, GateLayer};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, EventBody, Input, KnownExtension, ThreadId, ThreadState,
};
use orch_ports::memory::{Call, VerdictScript};
use orch_ports::{
    AgentEndpoint, AgentError, Clock as _, OutboxKind, OutboxStatus, PortSet, SystemClock,
    ThreadStore,
};
use serde_json::json;
use support::*;

/// A world whose `plain` agent lists `steer/v1` on its card.
fn steerable() -> World {
    let w = World::new();
    w.agent.set_extensions("plain", &[KnownExtension::Steer]);
    w
}

/// The messages that reached the agent `plain` as messages (the first of a job, a follow-up).
fn messages(w: &World) -> Vec<(Option<String>, Vec<String>, String)> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                agent,
                task_id,
                reference_task_ids,
                text,
                steer: false,
                ..
            } if agent.as_str() == "plain" => Some((task_id, reference_task_ids, text)),
            _ => None,
        })
        .collect()
}

/// What was sent into a running task: the task, its context, the references and the text.
/// A steer as the agent got it: the task, the context, the references and the text.
type Steer = (Option<String>, Option<String>, Vec<String>, String);

fn steers(w: &World) -> Vec<Steer> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                task_id,
                context_id,
                reference_task_ids,
                text,
                steer: true,
                ..
            } => Some((task_id, context_id, reference_task_ids, text)),
            _ => None,
        })
        .collect()
}

/// The texts of the agent's messages in the log, in order.
async fn said(app: &TestApp, id: ThreadId) -> Vec<String> {
    events(app, &alice(), id)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AgentMessage(m) => Some(m.text),
            _ => None,
        })
        .collect()
}

async fn jobs_started(app: &TestApp, id: ThreadId) -> usize {
    shape(&events(app, &alice(), id).await)
        .iter()
        .filter(|k| *k == "job_started")
        .count()
}

#[tokio::test]
async fn a_message_sent_while_the_agent_works_is_read_by_its_running_task() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;

    let sent = app
        .post_message(&alice(), t.id, "you were wrong since line 1".into())
        .await
        .unwrap();
    let EventBody::UserMessage(data) = &sent.body else {
        panic!("{sent:?}");
    };
    assert_eq!(data.delivery, Some(orch_core::Delivery::Steer));
    // the running task reads it at its next step, while it is still running
    eventually("the task read the message", || async {
        said(&app, t.id)
            .await
            .contains(&"steered: you were wrong since line 1".to_owned())
            .then_some(())
    })
    .await;
    assert_eq!(state_of(&w, t.id).await, ThreadState::Working);
    // sent into the task the thread has, in its context, as a steer: no reference, no new task
    assert_eq!(
        steers(&w),
        [(
            Some("task-1".to_owned()),
            // the context the agent assigned to the thread's first message (ADR 0055)
            Some("ctx-1".to_owned()),
            vec![],
            "you were wrong since line 1".to_owned()
        )]
    );
    assert_eq!(
        messages(&w),
        [(None, vec![], "gate refactor the parser".to_owned())],
        "no second task"
    );
    eventually("the steer row is delivered", || async {
        w.store
            .list_open_outbox(t.id)
            .await
            .unwrap()
            .iter()
            .all(|r| r.kind != OutboxKind::Steer)
            .then_some(())
    })
    .await;

    // the job ends as one job: no second job, the message was not delivered again after the turn
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(jobs_started(&app, t.id).await, 0);
    assert_eq!(messages(&w).len(), 1);
    assert_eq!(steers(&w).len(), 1);
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "user_message",
            "agent_message",
            "artifact",
            "agent_status:completed",
            "thread_state:done",
        ]
    );
    run.shutdown().await;
}

/// An agent that reports its work only as steps (adam: `submitted` until a turn commits) is
/// working as soon as it reports one: the log says so, in front of the step, and the message a
/// person sends then is steered into the running task, not held until the turn ends.
#[tokio::test]
async fn an_agent_that_reports_only_steps_is_working_and_can_be_steered() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stepping refactor the parser").await;
    // no `working` status was ever sent: the step is the sign
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    assert_eq!(
        shape(&events(&app, &alice(), t.id).await),
        ["user_message", "agent_status:working", "agent_step"]
    );

    let sent = app
        .post_message(&alice(), t.id, "you were wrong since line 1".into())
        .await
        .unwrap();
    let EventBody::UserMessage(data) = &sent.body else {
        panic!("{sent:?}");
    };
    assert_eq!(data.delivery, Some(orch_core::Delivery::Steer));
    eventually("the task read the message", || async {
        said(&app, t.id)
            .await
            .contains(&"steered: you were wrong since line 1".to_owned())
            .then_some(())
    })
    .await;
    assert_eq!(
        steers(&w),
        [(
            Some("task-1".to_owned()),
            // the context the agent assigned to the thread's first message (ADR 0055)
            Some("ctx-1".to_owned()),
            vec![],
            "you were wrong since line 1".to_owned()
        )]
    );
    assert_eq!(messages(&w).len(), 1, "no second task");

    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(jobs_started(&app, t.id).await, 0, "one job");
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    let working = shape(&ev)
        .iter()
        .filter(|k| *k == "agent_status:working")
        .count();
    assert_eq!(working, 1, "{:?}", shape(&ev));
    run.shutdown().await;
}

#[tokio::test]
async fn two_steers_are_read_in_the_order_they_were_written() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate work").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    for text in ["first", "second", "third"] {
        app.post_message(&alice(), t.id, text.into()).await.unwrap();
    }
    eventually("the task read all three", || async {
        (said(&app, t.id).await.len() == 3).then_some(())
    })
    .await;
    assert_eq!(
        said(&app, t.id).await,
        ["steered: first", "steered: second", "steered: third"]
    );
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_does_not_list_the_extension_is_sent_the_message_after_its_turn() {
    let w = World::new(); // `plain` lists nothing
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    app.post_message(&alice(), t.id, "echo later".into())
        .await
        .unwrap();

    // the steer is refused (a card that does not list the extension is never sent a message for a
    // running task), the row becomes a delegation behind the one in flight and waits for the turn
    eventually("the steer fell back to a delegation", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        (open.len() == 2
            && open
                .iter()
                .filter(|r| r.kind == OutboxKind::Delegate)
                .count()
                == 2)
            .then_some(())
    })
    .await;
    assert_eq!(
        messages(&w).len(),
        1,
        "nothing reached the agent as a message"
    );

    w.agent.release_gate();
    eventually("the second job has run", || async {
        let ev = events(&app, &alice(), t.id).await;
        (shape(&ev)
            .iter()
            .filter(|k| *k == "thread_state:done")
            .count()
            == 2)
            .then_some(())
    })
    .await;
    assert_eq!(
        messages(&w),
        [
            (None, vec![], "gate refactor the parser".to_owned()),
            (None, vec!["task-1".to_owned()], "echo later".to_owned()),
        ],
        "after the turn, as a new task that names the first"
    );
    assert_eq!(jobs_started(&app, t.id).await, 1);
    assert!(
        steers(&w).iter().all(|(task, ..)| task.is_some()),
        "any attempt was for the running task"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_task_that_ended_under_the_steer_is_requeued_and_redelivered_as_the_next_job() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    // the agent's task ended at the instant of the steer: it answers UnsupportedOperation
    w.agent.fail_next_sends(1, || {
        AgentError::Unsupported("task task-1 is in a terminal state".to_owned())
    });
    app.post_message(&alice(), t.id, "echo one more thing".into())
        .await
        .unwrap();
    eventually("the steer was refused and requeued", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        (steers(&w).len() == 1
            && open
                .iter()
                .filter(|r| r.kind == OutboxKind::Delegate)
                .count()
                == 2)
            .then_some(())
    })
    .await;

    // the turn ends, and the message is redelivered: the thread is a conversation (ADR 0020)
    w.agent.release_gate();
    eventually("the redelivered message ran as job 2", || async {
        let ev = events(&app, &alice(), t.id).await;
        (shape(&ev)
            .iter()
            .filter(|k| *k == "thread_state:done")
            .count()
            == 2)
            .then_some(())
    })
    .await;
    assert_eq!(
        messages(&w),
        [
            (None, vec![], "gate refactor the parser".to_owned()),
            (
                None,
                vec!["task-1".to_owned()],
                "echo one more thing".to_owned()
            ),
        ]
    );
    assert_eq!(jobs_started(&app, t.id).await, 1);
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.number, 2);
    assert!(w.store.list_open_outbox(t.id).await.unwrap().is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn a_steer_that_cannot_reach_the_agent_is_retried_and_then_read() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate work").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    w.agent
        .fail_next_sends(2, || AgentError::unreachable("blip"));
    app.post_message(&alice(), t.id, "careful".into())
        .await
        .unwrap();
    eventually("the task read it", || async {
        said(&app, t.id)
            .await
            .contains(&"steered: careful".to_owned())
            .then_some(())
    })
    .await;
    // two attempts failed before the agent saw anything, the third was read, once
    assert_eq!(steers(&w).len(), 3);
    assert_eq!(
        said(&app, t.id)
            .await
            .iter()
            .filter(|m| *m == "steered: careful")
            .count(),
        1
    );
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(jobs_started(&app, t.id).await, 0, "delivered once");
    run.shutdown().await;
}

#[tokio::test]
async fn a_thread_that_is_not_working_is_not_steered() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    // an agent that asks: the thread is blocked, and the message answers it as plain A2A does
    let t = create(&app, &alice(), "plain", "ask which branch").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    app.post_message(&alice(), t.id, "main".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert!(steers(&w).is_empty(), "{:?}", steers(&w));
    run.shutdown().await;
}

// ---------------------------------------------------------------- mentions (ADR 0026)

/// What the person's message `echo @coder later` mentions: `@coder` at 5..11.
fn coder_mention() -> orch_core::Mention {
    orch_core::Mention {
        agent_id: AgentId::new("coder"),
        label: "@coder".to_owned(),
        start: 5,
        end: 11,
        card_url: None,
    }
}

/// The same, as the dispatcher resolves it from the directory when it sends.
fn coder_info() -> orch_ports::MentionInfo {
    orch_ports::MentionInfo {
        agent_id: AgentId::new("coder"),
        name: Some("Coder".to_owned()),
        label: "@coder".to_owned(),
        start: 5,
        end: 11,
        card_url: Some("https://coder.example.com/.well-known/agent-card.json".to_owned()),
    }
}

/// A message that mentions `coder`, sent through the door that checks mentions.
async fn mention_coder(app: &TestApp, id: ThreadId) {
    let outcome = app
        .submit(
            &alice(),
            id,
            Input::UserMessage {
                user: alice(),
                text: "echo @coder later".to_owned(),
                message_id: Some("m-2".to_owned()),
                run_id: Some("r-2".to_owned()),
                origin: orch_core::Origin::Agui,
                catalog: None,
                mentions: vec![coder_mention()],
            },
            Some("k:m-2".to_owned()),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, orch_app::ApplyOutcome::Applied { .. }));
}

/// The mentions each request carried, by whether it was a steer, for the texts that were sent.
fn mentions_sent(w: &World, steer: bool) -> Vec<(String, Vec<orch_ports::MentionInfo>)> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                text,
                steer: s,
                mentions,
                ..
            } if s == steer => Some((text, mentions.into_vec())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_steer_is_told_the_mentions_of_the_message_as_a_delegation_is() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    mention_coder(&app, t.id).await;
    eventually("the task read the message", || async {
        said(&app, t.id)
            .await
            .contains(&"steered: echo @coder later".to_owned())
            .then_some(())
    })
    .await;
    // into the running task, with the agent named as the directory gives it now
    assert_eq!(
        mentions_sent(&w, true),
        [("echo @coder later".to_owned(), vec![coder_info()])]
    );
    assert_eq!(
        mentions_sent(&w, false),
        [("gate refactor the parser".to_owned(), vec![])],
        "the first message mentions nobody"
    );
    // the log holds the references as sent, and the message is a steer
    let logged = events(&app, &alice(), t.id)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::UserMessage(m) if m.delivery.is_some() => Some(m),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].mentions, [coder_mention()]);
    assert_eq!(logged[0].delivery, Some(orch_core::Delivery::Steer));
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_steer_that_falls_back_is_the_delegation_with_the_same_mentions() {
    let w = World::new(); // `plain` lists nothing: the steer is refused
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    mention_coder(&app, t.id).await;

    // the row was a steer; it is now a delegation behind the one in flight, holding the references
    let rows = eventually("the steer fell back to a delegation", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        (open.len() == 2 && open.iter().all(|r| r.kind == OutboxKind::Delegate)).then_some(open)
    })
    .await;
    let mentioning: Vec<_> = rows
        .iter()
        .filter_map(|r| match &r.payload {
            orch_ports::OutboxPayload::Delegate {
                text,
                mentions,
                new_job,
                ..
            } => Some((text.clone(), mentions.clone(), *new_job)),
            _ => None,
        })
        .collect();
    assert!(
        mentioning.contains(&("echo @coder later".to_owned(), vec![coder_mention()], false)),
        "{mentioning:?}"
    );
    assert!(mentions_sent(&w, false).iter().all(|(_, m)| m.is_empty()));

    // after the turn it reaches the agent as a message, told the same references
    w.agent.release_gate();
    eventually("the second delegation was sent", || async {
        (mentions_sent(&w, false).len() == 2).then_some(())
    })
    .await;
    assert_eq!(
        mentions_sent(&w, false)[1],
        ("echo @coder later".to_owned(), vec![coder_info()])
    );
    // the attempt into the task was refused (the card lists no `steer/v1`): nothing steered was read
    assert!(
        said(&app, t.id)
            .await
            .iter()
            .all(|m| !m.starts_with("steered:")),
        "{:?}",
        said(&app, t.id).await
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_steered_message_that_becomes_the_next_job_keeps_its_mentions() {
    let w = steerable();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    // the task ended at the instant of the steer: the row is requeued and redelivered as job 2
    w.agent.fail_next_sends(1, || {
        AgentError::Unsupported("task task-1 is in a terminal state".to_owned())
    });
    mention_coder(&app, t.id).await;
    eventually("the steer was refused and requeued", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        (steers(&w).len() == 1
            && open
                .iter()
                .filter(|r| r.kind == OutboxKind::Delegate)
                .count()
                == 2)
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    eventually("job 2 ran", || async {
        let ev = events(&app, &alice(), t.id).await;
        (shape(&ev)
            .iter()
            .filter(|k| *k == "thread_state:done")
            .count()
            == 2)
            .then_some(())
    })
    .await;
    assert_eq!(
        mentions_sent(&w, false).last(),
        Some(&("echo @coder later".to_owned(), vec![coder_info()]))
    );
    // the job the message landed in may ask the agent it named
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert!(thread.job.mentioned.contains(&AgentId::new("coder")));
    run.shutdown().await;
}

// ---------------------------------------------------------------- open question 33

fn directory_with_reviewer() -> AgentDirectory {
    let entry = |id: &str, name: &str| AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
    };
    AgentDirectory::new(vec![
        entry("coder", "Coder"),
        entry("plain", "Plain"),
        entry("reviewer", "Reviewer"),
    ])
}

/// An application in which `plain` requires the verifier `reviewer`.
fn gated_app(w: &World) -> Arc<TestApp> {
    let gate = GateLayer::from_json(&json!({
        "require": ["verifier"], "verifier": "reviewer", "maxAttempts": 3,
    }))
    .unwrap()
    .unwrap();
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: directory_with_reviewer().fixed_registry(),
            },
            directory_with_reviewer(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                target_gates: [(AgentId::new("plain"), gate)].into(),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn from_worker(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

/// A message written while the job ran is an unsent delegation (its steer was not taken) when the
/// agent's task completes and the thread starts to verify it. The delegation is then sent as a task
/// of its own while the thread is `verifying`: that task is adopted, the thread works again, and its
/// updates are kept, not dropped as late.
#[tokio::test]
async fn a_task_the_agent_made_for_a_message_as_the_thread_ended_is_adopted_and_its_updates_kept() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Hang);
    let app = gated_app(&w);
    let t = create(&app, &alice(), "plain", "fix the login").await;
    // the worker's delegation is played by the test: nothing delivers it
    w.store
        .skip_unsent_delegates(t.id, SystemClock.now())
        .await
        .unwrap();
    app.apply(
        t.id,
        from_worker(AgentUpdate::Status {
            state: AgentTaskState::Working,
            detail: None,
        }),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // a message while the job runs: a steer row, which falls back to the delegation it stands for
    app.post_message(&alice(), t.id, "echo mind the style".into())
        .await
        .unwrap();
    let rows = w
        .store
        .claim_outbox("test", SystemClock.now(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, OutboxKind::Steer);
    assert!(
        w.store
            .requeue_as_delegate(&rows[0].lease().unwrap(), SystemClock.now())
            .await
            .unwrap()
    );
    // the task completes and the thread starts to verify
    for update in [
        AgentUpdate::Artifact {
            name: "branch".into(),
            mime_type: None,
            uri: None,
            text: Some(
                json!({"repository": "https://github.com/acme/demo.git", "branch": "agent/fix",
                       "commit": "a".repeat(40)})
                .to_string(),
            ),
        },
        AgentUpdate::Message {
            message_id: "m1".into(),
            text: "I fixed the login.".into(),
            is_final: true,
            purpose: None,
        },
        AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: None,
        },
    ] {
        app.apply(t.id, from_worker(update), None, None, None)
            .await
            .unwrap();
    }
    assert_eq!(state_of(&w, t.id).await, ThreadState::Verifying);

    let run = spawn_dispatcher(&app, fast(), "d1");
    // the agent makes a task for the delegation, and the thread works again (without the adoption
    // its updates would be dropped as late and the thread would stay `verifying`)
    eventually("the new task's updates are kept", || async {
        let ev = events(&app, &alice(), t.id).await;
        let shapes = shape(&ev);
        let after = shapes
            .iter()
            .skip_while(|k| *k != "agent_status:completed")
            .collect::<Vec<_>>();
        (after.iter().any(|k| *k == "agent_status:working")
            && after.iter().filter(|k| **k == "artifact").count() >= 1
            && after
                .iter()
                .filter(|k| **k == "agent_status:completed")
                .count()
                >= 2)
            .then_some(())
    })
    .await;
    // the message was sent once, as the task the agent made: the redelivery wrote no second one
    assert_eq!(
        messages(&w)
            .into_iter()
            .map(|(_, _, text)| text)
            .collect::<Vec<_>>(),
        ["echo mind the style"]
    );
    // it is the same job (the thread was verifying, not finished), now verifying the new work
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.number, 1);
    assert_eq!(thread.job.verification, 2);
    assert_eq!(thread.state, ThreadState::Verifying);
    assert!(
        events(&app, &alice(), t.id)
            .await
            .iter()
            .all(|e| !matches!(e.body, EventBody::Error(_))),
        "nothing was dropped or failed"
    );
    assert!(
        w.store
            .list_open_outbox(t.id)
            .await
            .unwrap()
            .iter()
            .all(|r| r.kind != OutboxKind::Delegate && r.status != OutboxStatus::Dead)
    );
    run.shutdown().await;
}
