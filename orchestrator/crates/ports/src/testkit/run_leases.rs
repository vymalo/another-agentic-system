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

/// The person whose identity-provider `sub` is `4f2a9c1e-0b7d-4c55-9e1a-6d3f8a2b7c10`: the one
/// kind of person a lease or the meter is answered an account for (ADR 0106).
pub fn person() -> UserId {
    UserId::new("4f2a9c1e-0b7d-4c55-9e1a-6d3f8a2b7c10")
}

/// A person whose id is an e-mail address, as a deployment that reads `userClaim: email` has:
/// never answered as an account.
pub fn email_person() -> UserId {
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

/// Binds a run and leases it to the injector.
pub async fn binds_then_leases_to_the_injector(broker: &impl RunLeases) {
    broker.bind(binding("run-1", 60)).await.expect("bind");
    let lease = broker
        .lease(&RunId::new("run-1"), &RunCaller::Injector)
        .await
        .expect("lease");
    assert_eq!(lease.token(), &token(), "the deployment's key is leased");
    assert_eq!(
        lease.account().as_str(),
        person().as_str(),
        "the lease names the person's account"
    );
    let shown = format!("{lease:?}");
    assert!(
        !shown.contains(person().as_str()) && !shown.contains("provider-key-for-tests"),
        "Debug shows neither the account nor the key"
    );
    assert!(
        lease.expires_at() <= Timestamp::now() + Duration::from_secs(30),
        "a lease lives at most 30 s"
    );
}

/// Every agent is refused, **its own run included**: the binding names the agent, it does not
/// authorise it (ADR 0084 decision 2, amended 2026-11-24).
pub async fn refuses_every_agent_its_own_run_included(broker: &impl RunLeases) {
    broker.bind(binding("run-2", 60)).await.expect("bind");
    expect_not_bound(
        broker
            .lease(&RunId::new("run-2"), &RunCaller::Agent(coder()))
            .await,
        "the bound agent, for its own run",
    );
    expect_not_bound(
        broker
            .lease(&RunId::new("run-2"), &RunCaller::Agent(chat()))
            .await,
        "another agent",
    );
}

/// The orchestrator leases too: with the injector it is one of the two callers the lookup answers.
pub async fn leases_to_the_orchestrator_too(broker: &impl RunLeases) {
    broker.bind(binding("run-3", 60)).await.expect("bind");
    broker
        .lease(&RunId::new("run-3"), &RunCaller::Orchestrator)
        .await
        .expect("the orchestrator leases");
}

/// An unknown run is refused.
pub async fn refuses_an_unknown_run(broker: &impl RunLeases) {
    expect_not_bound(
        broker
            .lease(&RunId::new("never-bound"), &RunCaller::Injector)
            .await,
        "unknown run",
    );
}

/// A binding past its deadline is expired, not unbound.
pub async fn refuses_an_expired_binding(broker: &impl RunLeases) {
    broker.bind(expired("run-4")).await.expect("bind");
    match broker
        .lease(&RunId::new("run-4"), &RunCaller::Injector)
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
            .lease(&RunId::new("run-5"), &RunCaller::Injector)
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
            .lease(&RunId::new("run-7"), &RunCaller::Injector)
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
            .lease(&RunId::new("run-8"), &RunCaller::Injector)
            .await,
        "no credential",
    );
}

/// A lease never outlives its binding, even when the binding ends sooner than 30 s.
pub async fn a_lease_never_outlives_its_binding(broker: &impl RunLeases) {
    broker.bind(binding("run-9", 5)).await.expect("bind");
    let lease = broker
        .lease(&RunId::new("run-9"), &RunCaller::Injector)
        .await
        .expect("lease");
    assert!(
        lease.expires_at() <= Timestamp::now() + Duration::from_secs(5),
        "the binding's deadline bounds the lease"
    );
}

/// The meter alone is answered the account of a bound run; every other caller, and an unknown run,
/// is `run_not_bound`, and the meter is refused the lease.
pub async fn answers_the_account_to_the_meter_alone(broker: &impl RunLeases) {
    broker.bind(binding("run-10", 60)).await.expect("bind");
    let run = RunId::new("run-10");
    let account = broker
        .account(&run, &RunCaller::Meter)
        .await
        .expect("the meter reads the account");
    assert_eq!(
        account.as_str(),
        person().as_str(),
        "the account is the binding's person"
    );
    for (caller, name) in [
        (RunCaller::Orchestrator, "the orchestrator"),
        (RunCaller::Injector, "the injector"),
        (RunCaller::Agent(coder()), "the bound agent"),
        (RunCaller::Agent(chat()), "another agent"),
    ] {
        match broker.account(&run, &caller).await {
            Err(RunLeaseError::NotBound) => {}
            other => panic!("{name}: expected run_not_bound, got {other:?}"),
        }
    }
    match broker
        .account(&RunId::new("never-bound"), &RunCaller::Meter)
        .await
    {
        Err(RunLeaseError::NotBound) => {}
        other => panic!("unknown run: expected run_not_bound, got {other:?}"),
    }
    expect_not_bound(
        broker.lease(&run, &RunCaller::Meter).await,
        "the meter, for the lease",
    );
}

/// One `sub` bound under two runs is the same account, byte for byte, to the meter and in the
/// lease: the account is the person's, not the run's, so a person's runs add up under one id.
pub async fn one_sub_under_two_runs_is_one_account_byte_for_byte(broker: &impl RunLeases) {
    broker.bind(binding("run-11", 60)).await.expect("bind");
    broker.bind(binding("run-12", 60)).await.expect("bind");
    let first = broker
        .account(&RunId::new("run-11"), &RunCaller::Meter)
        .await
        .expect("the first run's account");
    let second = broker
        .account(&RunId::new("run-12"), &RunCaller::Meter)
        .await
        .expect("the second run's account");
    assert_eq!(first.as_str().as_bytes(), second.as_str().as_bytes());
    assert_eq!(
        first.as_str(),
        person().as_str(),
        "the sub itself, unchanged"
    );
    let leased = broker
        .lease(&RunId::new("run-12"), &RunCaller::Injector)
        .await
        .expect("lease");
    assert_eq!(
        leased.account().as_str().as_bytes(),
        first.as_str().as_bytes(),
        "the lease names the account the meter is told"
    );
}

/// A run whose person is an e-mail address is never answered an account: not to the meter, not in a
/// lease; nothing with an `@` leaves, and the refusal quotes nothing.
pub async fn never_answers_an_email_person(broker: &impl RunLeases) {
    let mut bound = binding("run-13", 60);
    bound.person = email_person();
    broker
        .bind(bound)
        .await
        .expect("a binding is accepted: the check is on the answer");
    let run = RunId::new("run-13");
    match broker.account(&run, &RunCaller::Meter).await {
        Err(RunLeaseError::AccountNotOpaque) => {}
        other => panic!("the meter: expected account_not_opaque, got {other:?}"),
    }
    for caller in [RunCaller::Injector, RunCaller::Orchestrator] {
        match broker.lease(&run, &caller).await {
            Err(RunLeaseError::AccountNotOpaque) => {}
            other => panic!("{caller:?}: expected account_not_opaque, got {other:?}"),
        }
    }
    let shown = RunLeaseError::AccountNotOpaque.to_string();
    assert!(
        !shown.contains('@') && !shown.contains("example"),
        "{shown}"
    );
    // The check does not make the run known to anyone else: every other caller is still not bound.
    match broker.account(&run, &RunCaller::Agent(coder())).await {
        Err(RunLeaseError::NotBound) => {}
        other => panic!("an agent: expected run_not_bound, got {other:?}"),
    }
}

/// A run that ended is still answered to the meter (within the bound, 24 hours by default) and is
/// never leased again: no credential outlives the job. A run never bound leaves nothing behind, and
/// ending twice is harmless.
pub async fn a_run_that_ended_is_answered_to_the_meter_and_never_leased(broker: &impl RunLeases) {
    broker.bind(binding("run-14", 60)).await.expect("bind");
    let run = RunId::new("run-14");
    broker.unbind(&run).await.expect("unbind");
    broker.unbind(&run).await.expect("unbind twice");
    let account = broker
        .account(&run, &RunCaller::Meter)
        .await
        .expect("an ended run is still answered to the meter within the bound");
    assert_eq!(account.as_str(), person().as_str());
    expect_not_bound(
        broker.lease(&run, &RunCaller::Injector).await,
        "an ended run is not leased",
    );
    for caller in [RunCaller::Orchestrator, RunCaller::Injector] {
        match broker.account(&run, &caller).await {
            Err(RunLeaseError::NotBound) => {}
            other => panic!("{caller:?}: expected run_not_bound, got {other:?}"),
        }
    }
    broker
        .unbind(&RunId::new("run-never"))
        .await
        .expect("unbind of a run never bound");
    match broker
        .account(&RunId::new("run-never"), &RunCaller::Meter)
        .await
    {
        Err(RunLeaseError::NotBound) => {}
        other => panic!("a run never bound keeps no tombstone, got {other:?}"),
    }
    // Bound again, the run is live: leased, and its tombstone is gone.
    broker
        .bind(binding("run-14", 60))
        .await
        .expect("bind again");
    broker
        .lease(&run, &RunCaller::Injector)
        .await
        .expect("a run bound anew is leased");
}

/// For a broker built with a tombstone bound of **zero**: a run that ended is `run_not_bound` to the
/// meter at once, the answer after the bound.
pub async fn an_ended_run_is_not_found_after_the_bound(broker: &impl RunLeases) {
    broker.bind(binding("run-15", 60)).await.expect("bind");
    let run = RunId::new("run-15");
    broker
        .account(&run, &RunCaller::Meter)
        .await
        .expect("a live run is answered, whatever the bound");
    broker.unbind(&run).await.expect("unbind");
    match broker.account(&run, &RunCaller::Meter).await {
        Err(RunLeaseError::NotBound) => {}
        other => panic!("past the bound: expected run_not_bound, got {other:?}"),
    }
}
