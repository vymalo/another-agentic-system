//! A person's own agent on the message path (ANR-806, part 2): a message to a provisioned kind
//! makes or resumes the person's agent and waits until it is ready **before** the message is
//! written or dispatched; the wait is bounded by `provisioner.readyTimeoutSecs`; two messages at
//! once make one agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_app::{AppConfig, AppError, Creation, Inbound, NewThread, OUTER_SLACK, Provisioning};
use orch_core::{Classify, ErrorClass, ThreadId};
use orch_ports::memory::MemoryProvisioner;
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError, ProvisionedAgent,
    ThreadListing,
};
use support::*;

const BOUND: Duration = Duration::from_secs(30);

fn app_with(w: &World, provisioner: impl AgentProvisioner, kinds: &[AgentKind]) -> TestApp {
    // `World::app_with` hands out an `Arc`; the config is what changes here.
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        provisioning: Some(Provisioning::new(provisioner, kinds.iter().copied(), BOUND)),
        ..AppConfig::default()
    });
    std::sync::Arc::try_unwrap(app)
        .unwrap_or_else(|_| unreachable!("a fresh application has one owner"))
}

fn owner(user: &orch_core::UserId) -> AgentOwner {
    Provisioning::owner_of(user)
}

async fn thread_count(app: &TestApp, user: &orch_core::UserId) -> usize {
    app.list_threads(user, ThreadListing::recent(None, 50, false))
        .await
        .unwrap()
        .len()
}

/// A provisioner that never gets the agent ready: the wait ends at once, as the adapter's does
/// when its own bound runs out.
struct NeverReady;

impl AgentProvisioner for NeverReady {
    async fn ensure_ready(
        &self,
        _owner: &AgentOwner,
        _kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        Err(ProvisionError::NotReady {
            last: AgentState::Saving,
        })
    }
    async fn state(
        &self,
        _owner: &AgentOwner,
        _kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        Ok(None)
    }
    async fn suspend(&self, _owner: &AgentOwner, _kind: AgentKind) -> Result<(), ProvisionError> {
        Ok(())
    }
    async fn delete(&self, _owner: &AgentOwner, _kind: AgentKind) -> Result<(), ProvisionError> {
        Ok(())
    }
}

/// A provisioner the platform refuses.
struct Refusing;

impl AgentProvisioner for Refusing {
    async fn ensure_ready(
        &self,
        _owner: &AgentOwner,
        _kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        Err(ProvisionError::Refused)
    }
    async fn state(
        &self,
        _owner: &AgentOwner,
        _kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        Ok(None)
    }
    async fn suspend(&self, _owner: &AgentOwner, _kind: AgentKind) -> Result<(), ProvisionError> {
        Ok(())
    }
    async fn delete(&self, _owner: &AgentOwner, _kind: AgentKind) -> Result<(), ProvisionError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_first_message_to_a_coder_makes_the_persons_agent() {
    let w = World::new();
    let memory = MemoryProvisioner::new();
    let app = app_with(&w, memory.clone(), &[AgentKind::Coder]);

    assert_eq!(
        memory
            .state(&owner(&alice()), AgentKind::Coder)
            .await
            .unwrap(),
        None
    );
    let _ = create(&app, &alice(), "coder", "hello").await;

    assert_eq!(
        memory
            .state(&owner(&alice()), AgentKind::Coder)
            .await
            .unwrap(),
        Some(AgentState::Ready),
        "the agent is ready by the time the thread exists"
    );
    assert_eq!(memory.created(), 1);
    assert_eq!(memory.len(), 1);
    assert_eq!(thread_count(&app, &alice()).await, 1);
    // bob has none of his own, and alice's is not his
    assert_eq!(
        memory
            .state(&owner(&bob()), AgentKind::Coder)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn other_agents_and_kinds_the_deployment_does_not_make_are_left_alone() {
    let w = World::new();
    let memory = MemoryProvisioner::new();
    // only the workspace kind is provisioned here
    let app = app_with(&w, memory.clone(), &[AgentKind::Workspace]);

    create(&app, &alice(), "coder", "not provisioned here").await;
    create(&app, &alice(), "plain", "the shared agent").await;

    assert!(
        memory.is_empty(),
        "nothing is made for an agent that is not provisioned"
    );
    assert_eq!(memory.created(), 0);
}

#[tokio::test]
async fn without_a_provisioner_nothing_changes() {
    let w = World::new();
    let app = w.app();
    let thread = create(&app, &alice(), "coder", "hello").await;
    assert_eq!(thread_count(&app, &alice()).await, 1);
    // and a later message is as it was
    app.post_message(&alice(), thread.id, "again".to_owned())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_message_to_a_suspended_agent_resumes_it_and_waits_until_ready() {
    let w = World::new();
    let memory = MemoryProvisioner::new();
    let app = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    let thread = create(&app, &alice(), "coder", "hello").await;

    let me = owner(&alice());
    memory.suspend(&me, AgentKind::Coder).await.unwrap();
    assert_eq!(
        memory.state(&me, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Suspended)
    );
    // it takes a while to come back: the message is accepted only after that
    memory.set_ready_delay(Duration::from_millis(200));

    let started = tokio::time::Instant::now();
    app.post_message(&alice(), thread.id, "wake up".to_owned())
        .await
        .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the message waited for the agent to be ready, took {:?}",
        started.elapsed()
    );
    assert_eq!(
        memory.state(&me, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Ready)
    );
    assert_eq!(memory.created(), 1, "a resume is not a second agent");
}

#[tokio::test]
async fn two_first_messages_at_once_make_one_agent() {
    let w = World::new();
    let memory = MemoryProvisioner::new().with_ready_delay(Duration::from_millis(100));
    let app = app_with(&w, memory.clone(), &[AgentKind::Coder]);

    let who = alice();
    let new = |text: &str| NewThread {
        title: None,
        target: target("coder"),
        text: text.to_owned(),
    };
    let (a, b) = tokio::join!(
        app.create_thread_as(
            &who,
            ThreadId(uuid::Uuid::now_v7()),
            new("one"),
            Inbound::default()
        ),
        app.create_thread_as(
            &who,
            ThreadId(uuid::Uuid::now_v7()),
            new("two"),
            Inbound::default()
        ),
    );
    assert!(matches!(a.unwrap(), Creation::Created { .. }));
    assert!(matches!(b.unwrap(), Creation::Created { .. }));

    assert_eq!(memory.created(), 1, "one owner and kind is one agent");
    assert_eq!(memory.len(), 1);
    assert_eq!(thread_count(&app, &alice()).await, 2);
}

#[tokio::test]
async fn two_messages_at_once_to_a_suspended_agent_make_no_second_one() {
    let w = World::new();
    let memory = MemoryProvisioner::new().with_ready_delay(Duration::from_millis(100));
    let app = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    let thread = create(&app, &alice(), "coder", "hello").await;
    memory
        .suspend(&owner(&alice()), AgentKind::Coder)
        .await
        .unwrap();

    let who = alice();
    let (a, b) = tokio::join!(
        app.post_message(&who, thread.id, "one".to_owned()),
        app.post_message(&who, thread.id, "two".to_owned()),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(memory.created(), 1);
    assert_eq!(memory.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_agent_that_is_never_ready_ends_the_wait_at_the_wait_ready_bound() {
    let w = World::new();
    // the platform leaves it saving for an hour
    let memory = MemoryProvisioner::new().with_ready_delay(Duration::from_secs(3600));
    let app = app_with(&w, memory.clone(), &[AgentKind::Coder]);

    let started = tokio::time::Instant::now();
    let err = app
        .create_thread(
            &alice(),
            NewThread {
                title: None,
                target: target("coder"),
                text: "hello".to_owned(),
            },
        )
        .await
        .unwrap_err();
    let waited = started.elapsed();

    let AppError::AgentNotReady {
        kind,
        waited: said,
        last,
    } = &err
    else {
        panic!("expected AgentNotReady, got {err:?}");
    };
    assert_eq!(*kind, "coder");
    assert_eq!(*said, BOUND, "the error names the wait-ready bound");
    assert_eq!(*last, None, "the platform said nothing in time");
    assert!(waited >= BOUND, "gave up early, after {waited:?}");
    assert!(
        waited <= BOUND + OUTER_SLACK,
        "the wait ran past the bound and its slack: {waited:?}"
    );
    assert_eq!(
        err.class(),
        ErrorClass::Transient,
        "the caller can try again"
    );
    // nothing was written: no thread, so nothing for the dispatcher
    assert_eq!(thread_count(&app, &alice()).await, 0);
}

#[tokio::test]
async fn a_not_ready_answer_is_a_timeout_that_names_the_bound_and_writes_nothing() {
    let w = World::new();
    let app = app_with(&w, NeverReady, &[AgentKind::Coder]);
    // a thread of their own to message (made under no provisioner, as before the deployment
    // turned it on)
    let plain = w.app();
    let thread = create(&plain, &alice(), "coder", "hello").await;
    let before = events(&plain, &alice(), thread.id).await.len();

    let err = app
        .post_message(&alice(), thread.id, "are you there".to_owned())
        .await
        .unwrap_err();
    let AppError::AgentNotReady { waited, last, .. } = &err else {
        panic!("expected AgentNotReady, got {err:?}");
    };
    assert_eq!(
        *waited, BOUND,
        "the bound of the wait for ready, not the dispatch's"
    );
    assert_eq!(*last, Some(AgentState::Saving));
    assert_eq!(err.class(), ErrorClass::Transient);
    // the message was not written: the dispatcher has nothing to send
    assert_eq!(events(&plain, &alice(), thread.id).await.len(), before);

    // a first message is refused the same way and leaves no second thread
    let err = app
        .create_thread(
            &alice(),
            NewThread {
                title: None,
                target: target("coder"),
                text: "hello again".to_owned(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::AgentNotReady { .. }));
    assert_eq!(thread_count(&app, &alice()).await, 1);
}

#[tokio::test]
async fn a_platform_refusal_is_the_deployments_fault_and_not_the_persons() {
    let w = World::new();
    let app = app_with(&w, Refusing, &[AgentKind::Coder]);
    let err = app
        .create_thread(
            &alice(),
            NewThread {
                title: None,
                target: target("coder"),
                text: "hello".to_owned(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::Provisioning { kind: "coder", .. }),
        "{err:?}"
    );
    assert_eq!(err.class(), ErrorClass::Internal);
    assert_eq!(thread_count(&app, &alice()).await, 0);
}

#[test]
fn the_owner_key_is_stable_distinct_and_short_enough_for_every_kind() {
    let a = Provisioning::owner_of(&alice());
    assert_eq!(
        a,
        Provisioning::owner_of(&orch_core::UserId::new(" Alice@Example.com "))
    );
    assert_ne!(a, Provisioning::owner_of(&bob()));
    assert!(!a.as_str().contains('@'), "the key names nobody");
    // the vector dev/provisioner-e2e.sh computes with `sha256sum | cut -c1-24`
    assert_eq!(
        Provisioning::owner_of(&orch_core::UserId::new("dev@example.com")).as_str(),
        "eb2b6c0d061bbd5caa545b6d"
    );
    for kind in [AgentKind::Coder, AgentKind::Workspace] {
        assert!(kind.agent_name(&a).len() <= 40, "{}", kind.agent_name(&a));
    }
}

#[tokio::test]
async fn an_agent_the_deployment_calls_coder_by_an_alias_is_provisioned_too() {
    // the dev stack's shape: the agent is `adam` and `coder` is its alias (ADR 0049); a thread is
    // created under the canonical id, so the kind must be read from the aliases as well
    let w = World::new();
    let memory = MemoryProvisioner::new();
    let entry = |id: &str| orch_app::AgentEntry {
        endpoint: orch_ports::AgentEndpoint::a2a(
            orch_core::AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
    };
    let directory = orch_app::AgentDirectory::new(vec![entry("adam"), entry("plain")])
        .with_aliases(std::collections::BTreeMap::from([(
            orch_core::AgentId::new("coder"),
            orch_core::AgentId::new("adam"),
        )]));
    let app = w.app_with_directory(
        AppConfig {
            stream_poll: Duration::from_millis(100),
            provisioning: Some(Provisioning::new(memory.clone(), [AgentKind::Coder], BOUND)),
            ..AppConfig::default()
        },
        directory,
    );

    create(&app, &alice(), "coder", "by the alias").await;
    assert_eq!(memory.created(), 1);
    // `plain` has neither the name nor an alias of a kind: nothing more is made for bob
    create(&app, &bob(), "plain", "not provisioned").await;
    assert_eq!(memory.created(), 1);
    create(&app, &bob(), "adam", "by the id").await;
    assert_eq!(
        memory.created(),
        2,
        "the agent's own id says the kind through its alias"
    );
}

// --- what else makes the agent work, and what a replay is -------------------------------------

use orch_app::{FirstMessage, StartChildSession};
use orch_core::{
    ChildSessionsOffer, EventKind, Input, Origin, ThreadState, UiActionData, UiVersion,
};
use orch_ports::ThreadStore;
use serde_json::json;

fn message(text: &str, id: &str) -> Input {
    Input::UserMessage {
        user: alice(),
        text: text.to_owned(),
        message_id: Some(id.to_owned()),
        run_id: Some(format!("run-{id}")),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
    }
}

fn card_action() -> Input {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), json!("a"));
    Input::UiAction {
        user: alice(),
        action: UiActionData {
            tier: None,
            tier_cap: None,
            surface_id: "s1".into(),
            name: "go".into(),
            source_component_id: "go".into(),
            context,
            version: UiVersion::V1_0,
            run_id: Some("run-ui".into()),
        },
        catalog: None,
        tier: None,
        tier_cap: None,
    }
}

fn thread_n(n: u128) -> ThreadId {
    ThreadId(uuid::Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

/// A thread of `coder` that has answered, made with no provisioner (as before the deployment
/// turned it on), the dispatcher stopped.
async fn answered_coder(w: &World) -> orch_core::ThreadRecord {
    let plain = w.app();
    let run = spawn_dispatcher(&plain, fast(), "d0");
    let thread = create(&plain, &alice(), "coder", "echo hi").await;
    wait_state(&plain, &alice(), thread.id, ThreadState::Done).await;
    run.shutdown().await;
    plain.get_thread(&alice(), thread.id).await.unwrap()
}

async fn log_len(w: &World, thread: ThreadId) -> usize {
    w.store.list_events(thread, 0, 500).await.unwrap().len()
}

#[tokio::test]
async fn an_action_on_a_card_waits_for_the_persons_agent_too() {
    let w = World::new();
    let thread = answered_coder(&w).await;
    let before = log_len(&w, thread.id).await;

    let app = app_with(&w, NeverReady, &[AgentKind::Coder]);
    let err = app
        .submit(&alice(), thread.id, card_action(), Some("ui:1".to_owned()))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::AgentNotReady { .. }), "{err:?}");
    assert_eq!(log_len(&w, thread.id).await, before, "nothing was written");

    // and with an agent that comes up, the platform was asked once
    let memory = MemoryProvisioner::new();
    let app = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    let _ = app
        .submit(&alice(), thread.id, card_action(), Some("ui:2".to_owned()))
        .await;
    assert_eq!(memory.created(), 1);
}

#[tokio::test]
async fn a_repeated_message_gets_its_first_answer_and_does_not_ask_the_platform() {
    let w = World::new();
    let thread = answered_coder(&w).await;
    // the first request: accepted, with the agent ready
    let memory = MemoryProvisioner::new();
    let ready = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    let key = Some("k:m-2".to_owned());
    let first = ready
        .submit(&alice(), thread.id, message("again", "m-2"), key.clone())
        .await
        .unwrap();
    assert!(matches!(first, orch_app::ApplyOutcome::Applied { .. }));
    assert_eq!(memory.created(), 1);
    let written = log_len(&w, thread.id).await;

    // the platform is down now: the same request, sent again, is still the acceptance it was
    let down = app_with(&w, NeverReady, &[AgentKind::Coder]);
    let again = down
        .submit(&alice(), thread.id, message("again", "m-2"), key)
        .await
        .unwrap();
    assert!(
        matches!(again, orch_app::ApplyOutcome::Duplicate),
        "{again:?}"
    );
    assert_eq!(log_len(&w, thread.id).await, written);
    // a message with another key is a first request and is refused
    let err = down
        .submit(
            &alice(),
            thread.id,
            message("another", "m-3"),
            Some("k:m-3".to_owned()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::AgentNotReady { .. }), "{err:?}");
}

#[tokio::test]
async fn a_repeated_first_message_is_not_a_reason_to_ask_the_platform() {
    let w = World::new();
    let memory = MemoryProvisioner::new();
    let ready = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    let id = thread_n(42);
    let new = || NewThread {
        title: None,
        target: target("coder"),
        text: "hello".to_owned(),
    };
    let first = ready
        .create_thread_as(&alice(), id, new(), Inbound::default())
        .await
        .unwrap();
    assert!(matches!(first, Creation::Created { .. }));
    assert_eq!(memory.created(), 1);

    let down = app_with(&w, NeverReady, &[AgentKind::Coder]);
    let again = down
        .create_thread_as(&alice(), id, new(), Inbound::default())
        .await
        .unwrap();
    assert!(matches!(again, Creation::Exists), "{again:?}");
    // a thread of somebody else's is not found
    let other = down
        .create_thread_as(&bob(), id, new(), Inbound::default())
        .await
        .unwrap_err();
    assert!(matches!(other, AppError::NotFound), "{other:?}");
}

#[tokio::test]
async fn a_repeated_fork_with_its_message_is_not_a_reason_to_ask_the_platform() {
    let w = World::new();
    let parent = answered_coder(&w).await;
    let memory = MemoryProvisioner::new();
    let ready = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    let first = || FirstMessage {
        text: "echo again".to_owned(),
        message_id: Some("m-fork".to_owned()),
        run_id: Some("run-fork".to_owned()),
        origin: Origin::Agui,
        ui_catalog: None,
        mentions: Vec::new(),
        tier: None,
        locale: None,
        pick: None,
    };
    let made = ready
        .fork_and_send(&alice(), parent.id, thread_n(43), 1, None, first())
        .await
        .unwrap();
    assert!(made.created);
    assert_eq!(memory.created(), 1);

    let down = app_with(&w, NeverReady, &[AgentKind::Coder]);
    let again = down
        .fork_and_send(&alice(), parent.id, thread_n(43), 1, None, first())
        .await
        .unwrap();
    assert!(!again.created, "the replay of the fork, not a second one");
    // a fork under another id is a first request and is refused
    let err = down
        .fork_and_send(&alice(), parent.id, thread_n(44), 1, None, first())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::AgentNotReady { .. }), "{err:?}");
}

#[tokio::test]
async fn a_helper_session_waits_for_the_persons_agent_and_a_repeat_does_not_ask() {
    let w = World::new();
    w.agent.set_child_sessions(
        "coder",
        Some(
            ChildSessionsOffer::from_params(&json!({
                "agents": [{"name": "planner", "description": "Plans"}],
                "maxOpen": 16
            }))
            .unwrap(),
        ),
    );
    let parent = answered_coder(&w).await;
    let start = |request: &str| StartChildSession {
        request: request.into(),
        agent: "planner".into(),
        description: Some("Find the writers".into()),
        text: "look".into(),
        keep_open: false,
        tier: None,
    };
    let asked = || async {
        w.store
            .list_events(parent.id, 0, 500)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind() == EventKind::ChildSessionStartAsked)
            .count()
    };

    // the platform does not answer: nothing is asked of the agent and nothing is written
    let down = app_with(&w, NeverReady, &[AgentKind::Coder]);
    let err = down
        .start_child_session(&alice(), parent.id, start("r1"))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::AgentNotReady { .. }), "{err:?}");
    assert_eq!(asked().await, 0);

    // with the agent ready the start is written, once
    let memory = MemoryProvisioner::new();
    let ready = app_with(&w, memory.clone(), &[AgentKind::Coder]);
    ready
        .start_child_session(&alice(), parent.id, start("r1"))
        .await
        .unwrap();
    assert_eq!(memory.created(), 1);
    assert_eq!(asked().await, 1);

    // the same request again, the platform down, is the acceptance it was
    down.start_child_session(&alice(), parent.id, start("r1"))
        .await
        .unwrap();
    assert_eq!(asked().await, 1);
}

// --- the message is stamped after the wait, not before it --------------------------------------

/// A provisioner that takes its time to get the agent ready, and notes when it said so.
struct Slow {
    ready_at: std::sync::Arc<std::sync::Mutex<Option<orch_core::Timestamp>>>,
}

impl AgentProvisioner for Slow {
    async fn ensure_ready(
        &self,
        _owner: &AgentOwner,
        _kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        *self.ready_at.lock().unwrap() = Some(orch_ports::Clock::now(&orch_ports::SystemClock));
        MemoryProvisioner::new()
            .ensure_ready(
                &AgentOwner::new("slow".to_owned()).unwrap(),
                AgentKind::Coder,
            )
            .await
    }
    async fn state(
        &self,
        _owner: &AgentOwner,
        _kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        Ok(None)
    }
    async fn suspend(&self, _owner: &AgentOwner, _kind: AgentKind) -> Result<(), ProvisionError> {
        Ok(())
    }
    async fn delete(&self, _owner: &AgentOwner, _kind: AgentKind) -> Result<(), ProvisionError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_first_message_is_stamped_when_the_agent_is_ready_and_not_before_the_wait() {
    let w = World::new();
    let ready_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let app = app_with(
        &w,
        Slow {
            ready_at: ready_at.clone(),
        },
        &[AgentKind::Coder],
    );
    let thread = create(&app, &alice(), "coder", "hello").await;
    let ready = ready_at.lock().unwrap().expect("the agent was asked for");
    let log = w.store.list_events(thread.id, 0, 10).await.unwrap();
    let first = log.first().expect("the message");
    assert!(
        first.at >= ready,
        "the first event is stamped {} before the agent was ready ({})",
        first.at,
        ready
    );
}

#[tokio::test]
async fn a_fork_message_is_stamped_when_the_agent_is_ready_and_not_before_the_wait() {
    let w = World::new();
    let parent = answered_coder(&w).await;
    let ready_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let app = app_with(
        &w,
        Slow {
            ready_at: ready_at.clone(),
        },
        &[AgentKind::Coder],
    );
    let made = app
        .fork_and_send(
            &alice(),
            parent.id,
            thread_n(45),
            1,
            None,
            FirstMessage {
                text: "echo again".to_owned(),
                message_id: Some("m-fork".to_owned()),
                run_id: Some("run-fork".to_owned()),
                origin: Origin::Agui,
                ui_catalog: None,
                mentions: Vec::new(),
                tier: None,
                locale: None,
                pick: None,
            },
        )
        .await
        .unwrap();
    let ready = ready_at.lock().unwrap().expect("the agent was asked for");
    let log = w.store.list_events(made.thread.id, 0, 50).await.unwrap();
    let message = log
        .iter()
        .find(|e| e.kind() == EventKind::UserMessage && e.at >= made.thread.created_at)
        .expect("the fork's own message");
    assert!(message.at >= ready, "stamped before the agent was ready");
}

// --- the message goes to the person's own agent (ANR-1061) ------------------------------------

use orch_core::AgentId;
use orch_ports::memory::Call;

/// The deployment's `coder` and `plain`, and the platform's agents of alice and bob, listed in the
/// registry under the names the provisioner gives them.
fn directory_with_own_agents() -> orch_app::AgentDirectory {
    let entry = |id: &str| orch_app::AgentEntry {
        endpoint: orch_ports::AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
    };
    let own = |user: &orch_core::UserId| AgentKind::Coder.agent_name(&owner(user));
    orch_app::AgentDirectory::new(vec![
        entry("coder"),
        entry("plain"),
        entry(&own(&alice())),
        entry(&own(&bob())),
    ])
}

fn app_over_own_agents(w: &World, provisioned: bool) -> std::sync::Arc<TestApp> {
    w.app_with_directory(
        AppConfig {
            stream_poll: Duration::from_millis(100),
            provisioning: provisioned
                .then(|| Provisioning::new(MemoryProvisioner::new(), [AgentKind::Coder], BOUND)),
            ..AppConfig::default()
        },
        directory_with_own_agents(),
    )
}

fn sent_to(w: &World) -> Vec<String> {
    w.agent
        .sends()
        .into_iter()
        .map(|c| match c {
            Call::Send(call) => call.agent.to_string(),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_message_to_a_provisioned_coder_goes_to_the_persons_own_agent_every_time() {
    let w = World::new();
    let app = app_over_own_agents(&w, true);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let mine = AgentKind::Coder.agent_name(&owner(&alice()));

    let t = create(&app, &alice(), "coder", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    app.post_message(&alice(), t.id, "echo again".to_owned())
        .await
        .unwrap();
    eventually("the second turn to finish", || async {
        (w.agent.sends().len() == 2).then_some(())
    })
    .await;

    assert_eq!(
        sent_to(&w),
        [mine.clone(), mine],
        "both messages reach the person's agent, never the configured `coder`"
    );
    // the thread is still the coder's as far as the person and the gates are concerned
    let thread = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    assert_eq!(thread.target.agent_id, AgentId::new("coder"));
    run.shutdown().await;
}

#[tokio::test]
async fn two_people_reach_two_agents_and_a_fork_keeps_the_agent() {
    let w = World::new();
    let app = app_over_own_agents(&w, true);
    let run = spawn_dispatcher(&app, fast(), "d1");

    let a = create(&app, &alice(), "coder", "echo a").await;
    wait_state(&app, &alice(), a.id, ThreadState::Done).await;
    let b = create(&app, &bob(), "coder", "echo b").await;
    wait_state(&app, &bob(), b.id, ThreadState::Done).await;
    let fork = app
        .fork_and_send(
            &alice(),
            a.id,
            thread_n(61),
            1,
            None,
            FirstMessage {
                text: "echo forked".to_owned(),
                message_id: Some("m-fork".to_owned()),
                run_id: Some("run-fork".to_owned()),
                origin: Origin::Agui,
                ui_catalog: None,
                mentions: Vec::new(),
                tier: None,
                locale: None,
                pick: None,
            },
        )
        .await
        .unwrap();
    eventually("the fork's turn", || async {
        (w.agent.sends().len() == 3).then_some(())
    })
    .await;
    let _ = fork;

    let alice_agent = AgentKind::Coder.agent_name(&owner(&alice()));
    let bob_agent = AgentKind::Coder.agent_name(&owner(&bob()));
    assert_ne!(alice_agent, bob_agent);
    assert_eq!(sent_to(&w), [alice_agent.clone(), bob_agent, alice_agent]);
    run.shutdown().await;
}

#[tokio::test]
async fn without_provisioning_the_message_still_goes_to_the_configured_agent() {
    let w = World::new();
    let app = app_over_own_agents(&w, false);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "coder", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(sent_to(&w), ["coder"]);
    run.shutdown().await;
}

#[tokio::test]
async fn an_agent_the_deployment_does_not_provision_is_not_redirected() {
    let w = World::new();
    let app = app_over_own_agents(&w, true);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(sent_to(&w), ["plain"]);
    run.shutdown().await;
}

#[tokio::test]
async fn an_own_agent_the_registry_never_lists_ends_the_delegation_after_its_retries() {
    let w = World::new();
    // the registry lists `coder` only: the person's agent is made but never appears
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        provisioning: Some(Provisioning::new(
            MemoryProvisioner::new(),
            [AgentKind::Coder],
            BOUND,
        )),
        ..AppConfig::default()
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "coder", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    assert!(
        w.agent.sends().is_empty(),
        "nothing reached the shared agent in its place"
    );
    run.shutdown().await;
}
