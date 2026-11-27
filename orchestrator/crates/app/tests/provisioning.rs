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
    // a thread of their own to message, routed to their own agent
    let plain = app_over_own_agents(&w, true);
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

/// A thread of `coder` that has answered, routed to the person's own agent (listed in the
/// registry), the dispatcher stopped.
async fn answered_coder(w: &World) -> orch_core::ThreadRecord {
    let plain = app_over_own_agents(w, true);
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
    let recorded = w
        .store
        .get_thread(None, thread_n(61))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        recorded.job.route,
        Some(AgentId::new(AgentKind::Coder.agent_name(&owner(&alice())))),
        "the fork records the route of its owner's own agent"
    );

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

// --- the route is recorded with the thread (ANR-1061, round two) --------------------------------

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use orch_app::{App, Dispatcher};
use orch_core::AgentSource;
use orch_ports::memory::{
    MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    AgentAudience, AgentEndpoint, CompositeRegistry, FixedRegistry, PortSet, RegistryEntry,
    SystemClock,
};

type RegistryApp = App<
    PortSet<
        MemoryStore,
        MemoryWakeup,
        ScriptedAgent,
        SystemClock,
        SeqIds,
        ScriptedModel,
        CompositeRegistry<FixedRegistry, MemoryRegistry>,
    >,
>;

fn own_entry(id: &str) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
        audience: AgentAudience::Everyone,
    }
}

/// A provisioner that lists the agent in the platform's registry once it has been asked
/// `list_from` times, as a platform whose registry lags the agent being ready does.
#[derive(Clone)]
struct Listing {
    inner: MemoryProvisioner,
    registry: MemoryRegistry,
    calls: Arc<AtomicUsize>,
    list_from: usize,
}

impl AgentProvisioner for Listing {
    async fn ensure_ready(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let agent = self.inner.ensure_ready(owner, kind).await?;
        if n >= self.list_from {
            self.registry.add(own_entry(&agent.name));
        }
        Ok(agent)
    }
    async fn state(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        self.inner.state(owner, kind).await
    }
    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.inner.suspend(owner, kind).await
    }
    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.inner.delete(owner, kind).await
    }
}

fn listing(registry: &MemoryRegistry, list_from: usize) -> Listing {
    Listing {
        inner: MemoryProvisioner::new(),
        registry: registry.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
        list_from,
    }
}

fn app_over(
    w: &World,
    registry: &MemoryRegistry,
    provisioning: Option<Provisioning>,
) -> Arc<RegistryApp> {
    let directory = directory();
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
                registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                provisioning,
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn spawn_over(
    app: &Arc<RegistryApp>,
) -> (
    tokio::task::JoinHandle<()>,
    tokio_util::sync::CancellationToken,
) {
    let token = tokio_util::sync::CancellationToken::new();
    let dispatcher = Dispatcher::new(Arc::clone(app), fast(), "d1");
    (tokio::spawn(dispatcher.run(token.clone())), token)
}

async fn new_coder_thread(app: &RegistryApp, text: &str) -> ThreadId {
    let id = ThreadId(uuid::Uuid::now_v7());
    app.create_thread_as(
        &alice(),
        id,
        NewThread {
            title: None,
            target: target("coder"),
            text: text.to_owned(),
        },
        Inbound::default(),
    )
    .await
    .unwrap();
    id
}

async fn wait_in(w: &World, id: ThreadId, want: ThreadState) {
    eventually(&format!("state {want:?}"), || async {
        (w.store.get_thread(None, id).await.unwrap().unwrap().state == want).then_some(())
    })
    .await;
}

fn mine() -> String {
    AgentKind::Coder.agent_name(&owner(&alice()))
}

fn sends_to(w: &World, agent: &str) -> usize {
    w.agent
        .sends()
        .into_iter()
        .filter(|c| matches!(c, Call::Send(call) if call.agent.as_str() == agent))
        .count()
}

#[tokio::test]
async fn the_route_is_recorded_with_the_thread_and_kept_by_the_next_job() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    let app = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (task, token) = spawn_over(&app);
    let id = new_coder_thread(&app, "echo hi").await;
    wait_in(&w, id, ThreadState::Done).await;
    let thread = w.store.get_thread(None, id).await.unwrap().unwrap();
    assert_eq!(thread.job.route, Some(AgentId::new(mine())));
    assert_eq!(thread.target.agent_id, AgentId::new("coder"));
    app.post_message(&alice(), id, "echo again".to_owned())
        .await
        .unwrap();
    eventually("the second job", || async {
        (sends_to(&w, &mine()) == 2).then_some(())
    })
    .await;
    let thread = w.store.get_thread(None, id).await.unwrap().unwrap();
    assert_eq!(
        thread.job.route,
        Some(AgentId::new(mine())),
        "a new job keeps the route"
    );
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn a_thread_begun_before_provisioning_stays_on_the_shared_agent_and_its_context() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    // provisioning off: the thread begins at `coder` and takes its context
    let off = app_over(&w, &registry, None);
    let (task, token) = spawn_over(&off);
    let id = new_coder_thread(&off, "echo hi").await;
    wait_in(&w, id, ThreadState::Done).await;
    token.cancel();
    let _ = task.await;
    assert_eq!(sends_to(&w, "coder"), 1);

    // provisioning on: the next message keeps the thread where it is, in the same context
    let memory = MemoryProvisioner::new();
    let on = app_over(
        &w,
        &registry,
        Some(Provisioning::new(memory.clone(), [AgentKind::Coder], BOUND)),
    );
    let (task, token) = spawn_over(&on);
    on.post_message(&alice(), id, "echo again".to_owned())
        .await
        .unwrap();
    eventually("the second message", || async {
        (sends_to(&w, "coder") == 2).then_some(())
    })
    .await;
    assert_eq!(
        sends_to(&w, &mine()),
        0,
        "nothing reaches an agent that never had the context"
    );
    assert_eq!(
        memory.created(),
        0,
        "an agent nobody uses is not made for it"
    );
    let contexts: Vec<Option<String>> = w
        .agent
        .sends()
        .into_iter()
        .map(|c| match c {
            Call::Send(call) => call.context_id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(contexts, [None, Some("ctx-1".to_owned())]);
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn the_dispatcher_wakes_the_routed_agent_and_waits_for_the_registry_to_list_it() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    // the platform lists the agent only when it is asked the second time: the first is the
    // person's request, the second the dispatcher's
    let provisioner = listing(&registry, 2);
    let calls = Arc::clone(&provisioner.calls);
    let app = app_over(
        &w,
        &registry,
        Some(Provisioning::new(provisioner, [AgentKind::Coder], BOUND)),
    );
    let (task, token) = spawn_over(&app);
    let id = new_coder_thread(&app, "echo hi").await;
    wait_in(&w, id, ThreadState::Done).await;
    assert_eq!(sends_to(&w, &mine()), 1);
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the dispatcher asked the platform too"
    );
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn a_stop_for_a_task_at_an_unlisted_own_agent_cancels_locally_after_the_listing_attempts() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    let provisioner = listing(&registry, 1);
    let memory = provisioner.inner.clone();
    let app = app_over(
        &w,
        &registry,
        Some(Provisioning::new(provisioner, [AgentKind::Coder], BOUND)),
    );
    w.agent
        .set_extensions(&mine(), &[orch_core::KnownExtension::Steer]);
    let (task, token) = spawn_over(&app);
    let id = new_coder_thread(&app, "gate refactor the parser").await;
    wait_in(&w, id, ThreadState::Working).await;
    // the platform suspends the agent and stops listing it
    memory
        .suspend(&owner(&alice()), AgentKind::Coder)
        .await
        .unwrap();
    registry.remove(&AgentId::new(mine()));

    app.cancel(&alice(), id).await.unwrap();
    let stop = eventually("the stop's row", || async {
        w.store
            .list_open_outbox(id)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.kind == orch_ports::OutboxKind::Cancel)
            .map(|r| r.id)
    })
    .await;
    wait_in(&w, id, ThreadState::Cancelled).await;
    assert_eq!(
        memory
            .state(&owner(&alice()), AgentKind::Coder)
            .await
            .unwrap(),
        Some(AgentState::Suspended),
        "a stop does not wake the agent"
    );
    assert!(
        !w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { .. })),
        "the unlisted agent was not asked to cancel"
    );
    let row = w.store.get_outbox(stop).await.unwrap().unwrap();
    assert_eq!(
        row.attempts,
        fast().own_agent_listing_attempts,
        "the stop waited for the listing, once per attempt, and then cancelled locally"
    );
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn a_helper_of_a_routed_thread_is_routed_to_the_same_own_agent() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    let app = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (task, token) = spawn_over(&app);
    // the parent's agent says it has a session (kept open), whose round is the child thread's
    let id = new_coder_thread(&app, "sessions planner keep").await;
    wait_in(&w, id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        w.store
            .child_threads(&alice(), id)
            .await
            .unwrap()
            .into_iter()
            .next()
    })
    .await;
    wait_in(&w, kid.id, ThreadState::Done).await;
    let kid = w.store.get_thread(None, kid.id).await.unwrap().unwrap();
    assert_eq!(
        kid.job.route,
        Some(AgentId::new(mine())),
        "the helper keeps its parent's route"
    );
    // what the person says in the helper's thread goes to their agent, never to the shared one
    app.post_message(&alice(), kid.id, "echo again".to_owned())
        .await
        .unwrap();
    eventually("the helper's message", || async {
        (sends_to(&w, &mine()) == 2).then_some(())
    })
    .await;
    assert_eq!(
        sends_to(&w, "coder"),
        0,
        "nothing of the helper reaches the shared agent"
    );
    token.cancel();
    let _ = task.await;
}

/// A provisioner that holds every `ensure_ready` until the test opens the gate.
#[derive(Clone)]
struct Gated {
    inner: MemoryProvisioner,
    open: tokio::sync::watch::Receiver<bool>,
    /// How many calls reached the gate.
    entered: Arc<AtomicUsize>,
}

impl AgentProvisioner for Gated {
    async fn ensure_ready(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.clone();
        let _ = open.wait_for(|v| *v).await;
        self.inner.ensure_ready(owner, kind).await
    }
    async fn state(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        self.inner.state(owner, kind).await
    }
    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.inner.suspend(owner, kind).await
    }
    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.inner.delete(owner, kind).await
    }
}

/// A stop pressed while a delegation is **in flight** (a claim holds it, at the wake or at the send)
/// is not answered behind the delegation's back and is not given up: it is tried again
/// (`max_cancel_attempts` of the dispatcher, plus what fits in the wake bound for a routed thread)
/// until the delegation has been sent, and then stops the task it started. The probe is longer than
/// any wait of this test (`own_agent_probe` 60 s), so the claim that holds the delegation holds it at
/// the gate, as it did before the probe (ANR-1078 PR 2): a delegation between two probes is
/// `pending` and unsent, and a stop skips it (see
/// `a_stop_between_two_probes_cancels_a_routed_delegation_before_it_starts`). `min_attempts` is how many claims the stop must
/// have had while the delegation still waited. `transient_cancels` is how many times the agent
/// then fails the stop's `CancelTask` with an error that passes (ANR-1078, gap A of ADR 0103: the
/// waits are refunded, so the stop is still given its own `max_cancel_attempts` for the cancel).
async fn a_stop_waits_for_a_waking_delegation(
    max_cancel_attempts: u32,
    min_attempts: u32,
    transient_cancels: usize,
) {
    let w = World::new();
    let registry = MemoryRegistry::new();
    // the request path (one application) makes the thread and lists the agent at once; the
    // dispatcher (another) wakes the agent through a provisioner that holds it
    let requests = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (open_tx, open_rx) = tokio::sync::watch::channel(false);
    let entered = Arc::new(AtomicUsize::new(0));
    let dispatching = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            Gated {
                inner: MemoryProvisioner::new(),
                open: open_rx,
                entered: Arc::clone(&entered),
            },
            [AgentKind::Coder],
            BOUND,
        )),
    );
    w.agent
        .set_extensions(&mine(), &[orch_core::KnownExtension::Steer]);
    w.agent.fail_next_cancels(transient_cancels, || {
        orch_ports::AgentError::unreachable("the agent is busy")
    });
    let token = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(
        Dispatcher::new(
            Arc::clone(&dispatching),
            orch_app::DispatcherConfig {
                max_cancel_attempts,
                own_agent_probe: Duration::from_secs(60),
                ..fast()
            },
            "d1",
        )
        .run(token.clone()),
    );
    let id = new_coder_thread(&requests, "gate refactor the parser").await;
    // the delegation is claimed and waits in `ensure_ready`; the agent then goes unlisted
    eventually("the delegation at the gate", || async {
        (entered.load(Ordering::SeqCst) >= 1).then_some(())
    })
    .await;
    registry.remove(&AgentId::new(mine()));
    requests.cancel(&alice(), id).await.unwrap();
    // the stop has been claimed again and again, the delegation is still in flight
    eventually("the stop retried behind the delegation", || async {
        let rows = w.store.list_open_outbox(id).await.unwrap();
        let stop_attempts = rows
            .iter()
            .find(|r| r.kind == orch_ports::OutboxKind::Cancel)
            .map(|r| r.attempts)?;
        let delegating = rows
            .iter()
            .any(|r| r.kind == orch_ports::OutboxKind::Delegate);
        (delegating && stop_attempts >= min_attempts).then_some(())
    })
    .await;
    let thread = w.store.get_thread(None, id).await.unwrap().unwrap();
    assert_ne!(
        thread.state,
        ThreadState::Cancelled,
        "a delegation that is waiting to be sent is not cancelled behind its back"
    );
    assert_eq!(sends_to(&w, &mine()), 0);

    // the agent is listed and awake: the delegation is sent, and the stop reaches its task
    registry.add(own_entry(&mine()));
    open_tx.send(true).unwrap();
    wait_in(&w, id, ThreadState::Cancelled).await;
    let cancels = w
        .agent
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Cancel { .. }))
        .count();
    assert!(
        cancels >= 1,
        "the task the delegation started was cancelled at the agent"
    );
    if transient_cancels > 0 {
        assert_eq!(
            cancels,
            1 + transient_cancels,
            "the stop that waited is tried again for a cancel that failed, and then stops the task"
        );
    }
    token.cancel();
    let _ = task.await;
}

/// A routed delegation whose agent is slow to wake is cut at the probe and waits, `pending`, unsent
/// and with its claim refunded (ANR-1078 PR 2). A stop pressed then finds nothing sent: it ends the
/// thread `CancelledBeforeStart` at its first claim and sends nothing, as for any delegation that
/// has not begun (ADR 0103, amended 2026-11-27). The stop's wait for a delegation in flight is
/// reached only by a delegation a claim holds: at its probe, or at the send.
async fn a_stop_between_two_probes_ends_the_thread_before_it_starts(max_cancel_attempts: u32) {
    let w = World::new();
    let registry = MemoryRegistry::new();
    let requests = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (open_tx, open_rx) = tokio::sync::watch::channel(false);
    let entered = Arc::new(AtomicUsize::new(0));
    let dispatching = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            Gated {
                inner: MemoryProvisioner::new(),
                open: open_rx,
                entered: Arc::clone(&entered),
            },
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let token = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(
        Dispatcher::new(
            Arc::clone(&dispatching),
            orch_app::DispatcherConfig {
                max_cancel_attempts,
                own_agent_probe: Duration::from_millis(100),
                // one probe, then a wait far longer than the test: the row stays between probes
                own_agent_wait_delay: Duration::from_secs(30),
                ..fast()
            },
            "d1",
        )
        .run(token.clone()),
    );
    let id = new_coder_thread(&requests, "echo stopped before the agent woke").await;
    let waiting = eventually("the delegation waits for the agent", || async {
        w.store
            .list_open_outbox(id)
            .await
            .unwrap()
            .into_iter()
            .find(|r| {
                r.kind == orch_ports::OutboxKind::Delegate
                    && r.status == orch_ports::OutboxStatus::Pending
                    && r.refunded >= 1
            })
    })
    .await;
    assert!(
        entered.load(Ordering::SeqCst) >= 1,
        "the probe reached the gate"
    );
    assert_eq!(waiting.sent_at, None, "nothing was sent");
    assert_eq!(
        waiting.counted_attempts(),
        0,
        "the probe was refunded: {waiting:?}"
    );

    requests.cancel(&alice(), id).await.unwrap();
    wait_in(&w, id, ThreadState::Cancelled).await;
    assert_eq!(
        sends_to(&w, &mine()),
        0,
        "nothing was sent to the person's agent"
    );
    assert!(
        w.agent.sends().is_empty(),
        "nothing was sent to any agent: {:?}",
        w.agent.sends()
    );
    assert!(
        !w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { .. })),
        "no task existed to cancel at the agent"
    );
    // the delegation was skipped by the stop, and the stop is over: nothing is left open
    eventually("no row is left open", || async {
        w.store
            .list_open_outbox(id)
            .await
            .unwrap()
            .is_empty()
            .then_some(())
    })
    .await;
    open_tx.send(true).unwrap();
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn a_stop_between_two_probes_cancels_a_routed_delegation_before_it_starts_and_sends_nothing()
{
    a_stop_between_two_probes_ends_the_thread_before_it_starts(10_000).await;
}

#[tokio::test]
async fn a_stop_needs_no_retries_to_cancel_a_routed_delegation_that_waits_between_probes() {
    // one attempt is all the stop is given, and all it needs: nothing is in flight to wait for
    a_stop_between_two_probes_ends_the_thread_before_it_starts(1).await;
}

#[tokio::test]
async fn a_stop_that_waited_longer_than_max_cancel_attempts_is_still_retried_for_a_cancel_that_failed()
 {
    // the stop waits for five claims behind the delegation (`max_cancel_attempts` is 2), then the
    // agent refuses the first `CancelTask` with an error that passes: only the claims that tried
    // something count, so the stop is claimed again and stops the task
    a_stop_waits_for_a_waking_delegation(2, 5, 1).await;
}

/// ANR-1078, gap B of ADR 0103: a stop that waited behind a waking delegation, then finds the task
/// running at an agent the registry does not list, still tries the listing
/// `own_agent_listing_attempts` times before it cancels the thread locally.
#[tokio::test]
async fn a_stop_that_waited_still_gets_its_listing_attempts_before_it_cancels_locally() {
    const LISTING: u32 = 3;
    const WAITS: u32 = 3;
    let w = World::new();
    let registry = MemoryRegistry::new();
    let requests = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (open_tx, open_rx) = tokio::sync::watch::channel(false);
    let entered = Arc::new(AtomicUsize::new(0));
    let dispatching = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            Gated {
                inner: MemoryProvisioner::new(),
                open: open_rx,
                entered: Arc::clone(&entered),
            },
            [AgentKind::Coder],
            BOUND,
        )),
    );
    w.agent
        .set_extensions(&mine(), &[orch_core::KnownExtension::Steer]);
    // the platform stops listing the agent inside the send, before the delegation has a task: the
    // stop can see the task running only after that, so it always finds the agent unlisted
    {
        let registry = registry.clone();
        let agent = AgentId::new(mine());
        w.agent.on_next_send(move || {
            registry.remove(&agent);
        });
    }
    let token = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(
        Dispatcher::new(
            Arc::clone(&dispatching),
            orch_app::DispatcherConfig {
                own_agent_listing_attempts: LISTING,
                own_agent_probe: Duration::from_secs(60),
                ..fast()
            },
            "d1",
        )
        .run(token.clone()),
    );
    let id = new_coder_thread(&requests, "gate refactor the parser").await;
    eventually("the delegation at the gate", || async {
        (entered.load(Ordering::SeqCst) >= 1).then_some(())
    })
    .await;
    requests.cancel(&alice(), id).await.unwrap();
    // the stop has waited WAITS times behind the delegation
    let stop = eventually("the stop waited behind the delegation", || async {
        w.store
            .list_open_outbox(id)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.kind == orch_ports::OutboxKind::Cancel && r.attempts >= WAITS)
    })
    .await;
    open_tx.send(true).unwrap();

    wait_in(&w, id, ThreadState::Cancelled).await;
    assert!(
        !w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { .. })),
        "the unlisted agent was not asked to cancel"
    );
    let row = eventually("the stop finished", || async {
        let row = w.store.get_outbox(stop.id).await.unwrap().unwrap();
        (row.status == orch_ports::OutboxStatus::Delivered).then_some(row)
    })
    .await;
    assert!(row.refunded >= WAITS, "the waits were refunded: {row:?}");
    assert_eq!(
        row.counted_attempts(),
        LISTING,
        "after the waits the stop made its listing attempts, each one claim, and then cancelled: {row:?}"
    );
    token.cancel();
    let _ = task.await;
}

/// ANR-1078 PR 2, the card's "Done when": rows routed to a person's agent that is slow to wake do
/// not hold a dispatcher slot for the whole wait-ready bound, and the rows of others are still
/// served meanwhile. The provisioner is gated (never opened until the end), `concurrency` is 2 and
/// `routed` coder threads of alice wait for it; then a thread to `plain`, an agent nobody
/// provisions, must reach `Done` within `eventually`'s 10 s.
///
/// On the commit before the probe (0e6d34f0) each routed row held its slot for the whole wait,
/// `BOUND` plus `OUTER_SLACK`, 40 s, so the first `concurrency` rows held every slot and the
/// unrelated row was never claimed in 10 s.
///
/// What else is asserted: the routed rows stay open and unsent (never dead, nothing sent to the
/// person's agent), each is cut at the probe more than once and its claims are refunded (a counted
/// attempt is at most the one claim that holds it), and when the gate opens they all finish with
/// exactly one counted attempt, the one that sent.
async fn routed_rows_that_wait_for_a_slow_agent_leave_the_slots_to_the_rest(
    routed: usize,
    unrelated_person: orch_core::UserId,
) {
    const SLOTS: usize = 2;
    let w = World::new();
    let registry = MemoryRegistry::new();
    let requests = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (open_tx, open_rx) = tokio::sync::watch::channel(false);
    let entered = Arc::new(AtomicUsize::new(0));
    let dispatching = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            Gated {
                inner: MemoryProvisioner::new(),
                open: open_rx,
                entered: Arc::clone(&entered),
            },
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let token = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(
        Dispatcher::new(
            Arc::clone(&dispatching),
            orch_app::DispatcherConfig {
                concurrency: SLOTS,
                own_agent_wait_delay: Duration::from_millis(500),
                ..fast()
            },
            "d1",
        )
        .run(token.clone()),
    );
    let mut coders = Vec::new();
    for n in 0..routed {
        coders.push(new_coder_thread(&requests, &format!("echo routed {n}")).await);
    }
    // every slot is at the gate
    eventually("the slots at the gate", || async {
        (entered.load(Ordering::SeqCst) >= SLOTS).then_some(())
    })
    .await;

    // an agent nobody provisions, asked by someone else's thread or alice's own
    let plain = ThreadId(uuid::Uuid::now_v7());
    requests
        .create_thread_as(
            &unrelated_person,
            plain,
            NewThread {
                title: None,
                target: target("plain"),
                text: "echo not routed".to_owned(),
            },
            Inbound::default(),
        )
        .await
        .unwrap();
    wait_in(&w, plain, ThreadState::Done).await;
    assert_eq!(
        sends_to(&w, "plain"),
        1,
        "the unrelated thread was served while the routed rows waited"
    );

    // the routed rows are still waiting, not dead, and nothing of theirs was sent
    let mut rows = Vec::new();
    for id in &coders {
        let row = eventually("a routed row cut at the probe", || async {
            w.store
                .list_open_outbox(*id)
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.kind == orch_ports::OutboxKind::Delegate && r.refunded >= 1)
        })
        .await;
        assert_eq!(row.sent_at, None, "{row:?}");
        assert!(
            row.counted_attempts() <= 1,
            "a probe is refunded, so the cap sees at most the claim that holds the row: {row:?}"
        );
        let thread = w.store.get_thread(None, *id).await.unwrap().unwrap();
        assert!(!thread.state.is_terminal(), "{:?}", thread.state);
        rows.push(row.id);
    }
    assert_eq!(
        sends_to(&w, &mine()),
        0,
        "nothing reached the person's agent"
    );

    // the agent wakes: every routed row sends once and ends
    open_tx.send(true).unwrap();
    for id in &coders {
        wait_in(&w, *id, ThreadState::Done).await;
    }
    assert_eq!(sends_to(&w, &mine()), routed);
    for id in rows {
        let row = eventually("a routed row delivered", || async {
            let row = w.store.get_outbox(id).await.unwrap().unwrap();
            (row.status == orch_ports::OutboxStatus::Delivered).then_some(row)
        })
        .await;
        assert_eq!(
            row.counted_attempts(),
            1,
            "only the claim that sent counts: {row:?}"
        );
        assert!(row.refunded >= 1, "{row:?}");
    }
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn routed_rows_waiting_for_a_slow_agent_leave_the_slots_to_the_next_row() {
    // as many routed rows as slots: between two probes every slot is free
    routed_rows_that_wait_for_a_slow_agent_leave_the_slots_to_the_rest(2, alice()).await;
}

#[tokio::test]
async fn routed_rows_at_twice_the_slots_waiting_for_a_slow_agent_leave_slots_to_another_person() {
    // twice as many as slots: the claim order is fair between people and oldest first within one
    // person, so the unrelated thread is another person's (see ADR 0103, amended 2026-11-27)
    routed_rows_that_wait_for_a_slow_agent_leave_the_slots_to_the_rest(4, bob()).await;
}

#[tokio::test]
async fn a_message_steered_into_a_suspended_own_agent_wakes_it_first() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    let provisioner = listing(&registry, 1);
    let memory = provisioner.inner.clone();
    let app = app_over(
        &w,
        &registry,
        Some(Provisioning::new(provisioner, [AgentKind::Coder], BOUND)),
    );
    w.agent
        .set_extensions(&mine(), &[orch_core::KnownExtension::Steer]);
    let (task, token) = spawn_over(&app);
    let id = new_coder_thread(&app, "gate refactor the parser").await;
    wait_in(&w, id, ThreadState::Working).await;
    memory
        .suspend(&owner(&alice()), AgentKind::Coder)
        .await
        .unwrap();
    registry.remove(&AgentId::new(mine()));

    app.post_message(&alice(), id, "you were wrong since line 1".to_owned())
        .await
        .unwrap();
    eventually("the steer reached the own agent", || async {
        w.agent
            .sends()
            .into_iter()
            .any(|c| matches!(&c, Call::Send(call) if call.steer && call.agent.as_str() == mine()))
            .then_some(())
    })
    .await;
    assert_eq!(
        memory
            .state(&owner(&alice()), AgentKind::Coder)
            .await
            .unwrap(),
        Some(AgentState::Ready)
    );
    assert_eq!(sends_to(&w, "coder"), 0);
    token.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn an_own_agent_that_lists_a_tier_gets_the_message_under_it() {
    use orch_core::{AccessTier, TierOffer};
    let w = World::new();
    let registry = MemoryRegistry::new();
    let offer =
        TierOffer::from_params(&json!({"tiers": ["plan", "edits", "auto"], "default": "plan"}))
            .unwrap();
    // the request checks the tier against the thread's agent, the send against the own agent's card
    w.agent.set_access_tier("coder", Some(offer.clone()));
    w.agent.set_access_tier(&mine(), Some(offer));
    let app = app_over(
        &w,
        &registry,
        Some(Provisioning::new(
            listing(&registry, 1),
            [AgentKind::Coder],
            BOUND,
        )),
    );
    let (task, token) = spawn_over(&app);
    let id = ThreadId(uuid::Uuid::now_v7());
    app.create_thread_as(
        &alice(),
        id,
        NewThread {
            title: None,
            target: target("coder"),
            text: "echo hi".to_owned(),
        },
        Inbound {
            tier: Some(AccessTier::Edits),
            ..Inbound::default()
        },
    )
    .await
    .unwrap();
    wait_in(&w, id, ThreadState::Done).await;
    let tiers: Vec<_> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|c| match c {
            Call::Send(call) if call.agent.as_str() == mine() => Some(call.tier),
            _ => None,
        })
        .collect();
    assert_eq!(tiers, [Some(AccessTier::Edits)]);
    token.cancel();
    let _ = task.await;
}
