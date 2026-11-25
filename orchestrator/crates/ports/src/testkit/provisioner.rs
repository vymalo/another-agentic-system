//! `AgentProvisioner` conformance cases: what the orchestrator relies on to give a person their
//! own agent (first message makes it, a later one resumes it, nobody reaches another's).
//!
//! An implementation supplies a [`ProvisionerFixture`]: the provisioner, how many agents the
//! platform behind it holds, and a way to make an agent take `delay` to become ready.

use std::time::{Duration, Instant};

use crate::{AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError};

/// A provisioner under test, with a view of what is behind it.
pub trait ProvisionerFixture: Send + Sync + 'static {
    /// The provisioner under test.
    type Provisioner: AgentProvisioner;

    /// The provisioner.
    fn provisioner(&self) -> &Self::Provisioner;

    /// How many agents exist behind it, every owner and kind together.
    fn agent_count(&self) -> usize;

    /// From now on an agent that is made or resumed takes `delay` to be ready.
    fn set_ready_delay(&self, delay: Duration);
}

fn owner(key: &str) -> AgentOwner {
    AgentOwner::new(key).unwrap()
}

/// The first call for an owner makes the agent, named after the kind and the owner, ready.
pub async fn the_first_call_makes_the_agent_and_returns_it_ready<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    let o = owner("alice");
    assert_eq!(p.state(&o, AgentKind::Coder).await.unwrap(), None);
    let agent = p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    assert_eq!(agent.name, "coder-alice");
    assert_eq!(agent.state, AgentState::Ready);
    assert_eq!(
        p.state(&o, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Ready)
    );
    assert_eq!(f.agent_count(), 1);
}

/// Two first messages at once make exactly one agent, and both get it.
pub async fn two_first_calls_at_once_make_one_agent<F: ProvisionerFixture>(f: F) {
    f.set_ready_delay(Duration::from_millis(150));
    let p = f.provisioner();
    let o = owner("bob");
    let (a, b) = tokio::join!(
        p.ensure_ready(&o, AgentKind::Coder),
        p.ensure_ready(&o, AgentKind::Coder)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b);
    assert_eq!(a.state, AgentState::Ready);
    assert_eq!(f.agent_count(), 1);
}

/// A call for an agent that exists makes nothing more.
pub async fn a_second_call_makes_nothing_more<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    let o = owner("carol");
    let first = p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    let second = p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(f.agent_count(), 1);
}

/// A suspended agent is resumed, and the call returns only once it is ready, not before.
pub async fn a_suspended_agent_is_resumed_and_the_call_waits_for_ready<F: ProvisionerFixture>(
    f: F,
) {
    let p = f.provisioner();
    let o = owner("dave");
    p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    p.suspend(&o, AgentKind::Coder).await.unwrap();
    assert_eq!(
        p.state(&o, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Suspended)
    );

    let delay = Duration::from_millis(300);
    f.set_ready_delay(delay);
    let started = Instant::now();
    let agent = p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    assert!(
        started.elapsed() >= delay / 2,
        "the call returned before the agent was ready"
    );
    assert_eq!(agent.state, AgentState::Ready);
    assert_eq!(
        p.state(&o, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Ready)
    );
    assert_eq!(f.agent_count(), 1, "a resume makes no second agent");
}

/// One owner has one agent per kind.
pub async fn a_kind_has_its_own_agent<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    let o = owner("erin");
    let coder = p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    let ws = p.ensure_ready(&o, AgentKind::Workspace).await.unwrap();
    assert_ne!(coder.name, ws.name);
    assert_eq!(f.agent_count(), 2);
}

/// Suspending or deleting an owner's agent leaves another owner's alone.
pub async fn another_owners_agent_is_never_touched<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    let (a, b) = (owner("frank"), owner("grace"));
    p.ensure_ready(&a, AgentKind::Coder).await.unwrap();
    p.ensure_ready(&b, AgentKind::Coder).await.unwrap();
    p.suspend(&a, AgentKind::Coder).await.unwrap();
    assert_eq!(
        p.state(&b, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Ready)
    );
    p.delete(&a, AgentKind::Coder).await.unwrap();
    assert_eq!(p.state(&a, AgentKind::Coder).await.unwrap(), None);
    assert_eq!(
        p.state(&b, AgentKind::Coder).await.unwrap(),
        Some(AgentState::Ready)
    );
    assert_eq!(f.agent_count(), 1);
    // The owner whose agent is gone gets a fresh one and the other's is still the one agent.
    let again = p.ensure_ready(&a, AgentKind::Coder).await.unwrap();
    assert_eq!(again.name, "coder-frank");
    assert_eq!(f.agent_count(), 2);
}

/// Asking where an agent is never makes it.
pub async fn state_never_makes_an_agent<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    assert_eq!(
        p.state(&owner("heidi"), AgentKind::Coder).await.unwrap(),
        None
    );
    assert_eq!(f.agent_count(), 0);
}

/// Suspend and delete of an agent that is not there are not errors and make nothing.
pub async fn suspend_and_delete_of_an_absent_agent_change_nothing<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    let o = owner("ivan");
    p.suspend(&o, AgentKind::Coder).await.unwrap();
    p.delete(&o, AgentKind::Coder).await.unwrap();
    assert_eq!(f.agent_count(), 0);
}

/// An agent that was deleted is made again by the next call.
pub async fn a_deleted_agent_is_made_again<F: ProvisionerFixture>(f: F) {
    let p = f.provisioner();
    let o = owner("judy");
    p.ensure_ready(&o, AgentKind::Coder).await.unwrap();
    p.delete(&o, AgentKind::Coder).await.unwrap();
    match p.ensure_ready(&o, AgentKind::Coder).await {
        Ok(agent) => assert_eq!(agent.state, AgentState::Ready),
        // A platform that is still releasing the old one says so; the call is retryable.
        Err(ProvisionError::Deleting) => {}
        Err(e) => panic!("unexpected: {e}"),
    }
}
