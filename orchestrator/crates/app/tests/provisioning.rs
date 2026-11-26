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
