//! The [`RunLeases`] conformance cases (ADR 0084): the binding, the lease, and every refusal.

use std::time::Duration;

use jiff::Timestamp;
use orch_core::{AgentId, UserId};

use crate::{RunBinding, RunCaller, RunId, RunLease, RunLeaseError, RunLeases, ToolSecret};

/// The agent `coder`.
pub fn coder() -> AgentId {
    AgentId("coder".to_owned())
}

/// The agent `chat`.
pub fn chat() -> AgentId {
    AgentId("chat".to_owned())
}

/// The person `person@example.com`.
pub fn person() -> UserId {
    UserId::new("person@example.com")
}

/// A binding of `run` to `coder` and `person` for the model `glm-5.3`, ending in `in_secs`.
pub fn binding(run: &str, in_secs: i64) -> RunBinding {
    RunBinding {
        run: RunId::new(run),
        agent: coder(),
        person: person(),
        model: Some("glm-5.3".to_owned()),
        expires_at: Timestamp::now() + Duration::from_secs(in_secs.unsigned_abs()),
    }
}

/// A binding whose deadline has passed.
pub fn expired(run: &str) -> RunBinding {
    RunBinding {
        expires_at: Timestamp::now() - Duration::from_secs(5),
        ..binding(run, 60)
    }
}

/// The token every fixture leases for `glm-5.3`.
pub fn token() -> ToolSecret {
    ToolSecret::new("provider-key-for-tests")
}

fn expect_not_bound(result: Result<RunLease, RunLeaseError>, case: &str) {
    match result {
        Err(RunLeaseError::NotBound) => {}
        other => panic!("{case}: expected run_not_bound, got {other:?}"),
    }
}

fn expect_unavailable(result: Result<RunLease, RunLeaseError>, case: &str) {
    match result {
        Err(RunLeaseError::Unavailable(_)) => {}
        other => panic!("{case}: expected unavailable, got {other:?}"),
    }
}

/// Binds a run and leases it to the bound agent.
pub async fn binds_then_leases_to_the_bound_agent(broker: &impl RunLeases) {
    broker.bind(binding("run-1", 60)).await.expect("bind");
    let lease = broker
        .lease(&RunId::new("run-1"), &RunCaller::Agent(coder()))
        .await
        .expect("lease");
    assert_eq!(lease.token(), &token(), "the deployment's key is leased");
    assert!(
        lease.expires_at() <= Timestamp::now() + Duration::from_secs(30),
        "a lease lives at most 30 s"
    );
}

/// A lookup from another agent is refused.
pub async fn refuses_another_agent(broker: &impl RunLeases) {
    broker.bind(binding("run-2", 60)).await.expect("bind");
    expect_not_bound(
        broker
            .lease(&RunId::new("run-2"), &RunCaller::Agent(chat()))
            .await,
        "another agent",
    );
}

/// The orchestrator binds and unbinds; it does not lease.
pub async fn refuses_the_orchestrator_lease(broker: &impl RunLeases) {
    broker.bind(binding("run-3", 60)).await.expect("bind");
    expect_not_bound(
        broker
            .lease(&RunId::new("run-3"), &RunCaller::Orchestrator)
            .await,
        "the orchestrator",
    );
}

/// An unknown run is refused.
pub async fn refuses_an_unknown_run(broker: &impl RunLeases) {
    expect_not_bound(
        broker
            .lease(&RunId::new("never-bound"), &RunCaller::Agent(coder()))
            .await,
        "unknown run",
    );
}

/// A binding past its deadline is expired, not unbound.
pub async fn refuses_an_expired_binding(broker: &impl RunLeases) {
    broker.bind(expired("run-4")).await.expect("bind");
    match broker
        .lease(&RunId::new("run-4"), &RunCaller::Agent(coder()))
        .await
    {
        Err(RunLeaseError::Expired) => {}
        other => panic!("expected lease_expired, got {other:?}"),
    }
}

/// Unbinding is idempotent and ends the lease.
pub async fn unbinds(broker: &impl RunLeases) {
    broker.bind(binding("run-5", 60)).await.expect("bind");
    broker.unbind(&RunId::new("run-5")).await.expect("unbind");
    expect_not_bound(
        broker
            .lease(&RunId::new("run-5"), &RunCaller::Agent(coder()))
            .await,
        "after unbind",
    );
    broker
        .unbind(&RunId::new("run-5"))
        .await
        .expect("unbind is idempotent");
}

/// A run is bound once: the same binding is idempotent, a different one is a conflict.
pub async fn binds_once(broker: &impl RunLeases) {
    let same = binding("run-6", 60);
    broker.bind(same.clone()).await.expect("bind");
    broker
        .bind(same)
        .await
        .expect("the same binding is idempotent");
    let mut different = binding("run-6", 60);
    different.agent = chat();
    match broker.bind(different).await {
        Err(RunLeaseError::Conflict) => {}
        other => panic!("expected run_already_bound, got {other:?}"),
    }
}

/// A binding that names no model cannot be leased.
pub async fn refuses_a_binding_with_no_model(broker: &impl RunLeases) {
    let mut no_model = binding("run-7", 60);
    no_model.model = None;
    broker.bind(no_model).await.expect("bind");
    expect_unavailable(
        broker
            .lease(&RunId::new("run-7"), &RunCaller::Agent(coder()))
            .await,
        "no model",
    );
}

/// A model with no deployment credential fails closed.
pub async fn refuses_a_model_with_no_credential(broker: &impl RunLeases) {
    let mut unknown = binding("run-8", 60);
    unknown.model = Some("unknown-model".to_owned());
    broker.bind(unknown).await.expect("bind");
    expect_unavailable(
        broker
            .lease(&RunId::new("run-8"), &RunCaller::Agent(coder()))
            .await,
        "no credential",
    );
}

/// A lease never outlives its binding, even when the binding ends sooner than 30 s.
pub async fn a_lease_never_outlives_its_binding(broker: &impl RunLeases) {
    broker.bind(binding("run-9", 5)).await.expect("bind");
    let lease = broker
        .lease(&RunId::new("run-9"), &RunCaller::Agent(coder()))
        .await
        .expect("lease");
    assert!(
        lease.expires_at() <= Timestamp::now() + Duration::from_secs(5),
        "the binding's deadline bounds the lease"
    );
}
