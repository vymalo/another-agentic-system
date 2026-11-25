//! The `AgentProvisioner` testkit catches what it claims to: each fake below breaks **one** rule
//! of the port and the case of that rule must fail on it. The control fake, which breaks nothing,
//! passes every case, so a red case is the rule's and not the fake's. A case that passed a broken
//! fake would be a check that checks nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use orch_ports::testkit::provisioner as kit;
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError, ProvisionedAgent,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    /// Nothing: the control.
    Nothing,
    /// A first call checks for the agent, then makes it later, so two at once make two.
    CreatesTwiceOnConcurrentFirstCalls,
    /// A suspended agent is marked ready and returned at once, before it is.
    ResumeDoesNotWait,
    /// A suspend reaches every owner's agent of the kind.
    SuspendsAnotherOwner,
}

struct Fake {
    how: Break,
    agents: Mutex<Vec<(AgentOwner, AgentKind, AgentState)>>,
    delay_ms: AtomicU64,
}

impl Fake {
    fn new(how: Break) -> Self {
        Fake {
            how,
            agents: Mutex::default(),
            delay_ms: AtomicU64::new(0),
        }
    }

    fn delay(&self) -> Duration {
        Duration::from_millis(self.delay_ms.load(Ordering::SeqCst))
    }

    fn set(&self, owner: &AgentOwner, kind: AgentKind, state: AgentState) {
        for a in self.agents.lock().unwrap().iter_mut() {
            if a.0 == *owner && a.1 == kind {
                a.2 = state;
            }
        }
    }

    fn ready(owner: &AgentOwner, kind: AgentKind) -> ProvisionedAgent {
        ProvisionedAgent {
            name: kind.agent_name(owner),
            state: AgentState::Ready,
        }
    }
}

impl AgentProvisioner for Fake {
    async fn ensure_ready(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        let found = self
            .agents
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.0 == *owner && a.1 == kind)
            .map(|a| a.2);
        match found {
            None => {
                if self.how == Break::CreatesTwiceOnConcurrentFirstCalls {
                    // The check and the making are apart: both calls see "none".
                    tokio::time::sleep(self.delay()).await;
                    self.agents
                        .lock()
                        .unwrap()
                        .push((owner.clone(), kind, AgentState::Ready));
                } else {
                    // Check and make together, then the wait.
                    {
                        let mut agents = self.agents.lock().unwrap();
                        if !agents.iter().any(|a| a.0 == *owner && a.1 == kind) {
                            agents.push((owner.clone(), kind, AgentState::Saving));
                        }
                    }
                    tokio::time::sleep(self.delay()).await;
                    self.set(owner, kind, AgentState::Ready);
                }
            }
            Some(AgentState::Suspended) => {
                if self.how == Break::ResumeDoesNotWait {
                    self.set(owner, kind, AgentState::Ready);
                } else {
                    self.set(owner, kind, AgentState::Saving);
                    tokio::time::sleep(self.delay()).await;
                    self.set(owner, kind, AgentState::Ready);
                }
            }
            Some(_) => {}
        }
        Ok(Self::ready(owner, kind))
    }

    async fn state(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        Ok(self
            .agents
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.0 == *owner && a.1 == kind)
            .map(|a| a.2))
    }

    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        for a in self.agents.lock().unwrap().iter_mut() {
            let mine = a.0 == *owner && a.1 == kind;
            if mine || (self.how == Break::SuspendsAnotherOwner && a.1 == kind) {
                a.2 = AgentState::Suspended;
            }
        }
        Ok(())
    }

    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.agents
            .lock()
            .unwrap()
            .retain(|a| !(a.0 == *owner && a.1 == kind));
        Ok(())
    }
}

struct Fixture(Fake);

impl kit::ProvisionerFixture for Fixture {
    type Provisioner = Fake;
    fn provisioner(&self) -> &Fake {
        &self.0
    }
    fn agent_count(&self) -> usize {
        self.0.agents.lock().unwrap().len()
    }
    fn set_ready_delay(&self, delay: Duration) {
        self.0
            .delay_ms
            .store(u64::try_from(delay.as_millis()).unwrap(), Ordering::SeqCst);
    }
}

fn fixture(how: Break) -> Fixture {
    Fixture(Fake::new(how))
}

/// Whether the case panicked (its assertion failed) on the fake.
async fn goes_red<Fut>(case: Fut) -> bool
where
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(case).await.is_err()
}

#[tokio::test]
async fn the_control_fake_passes_every_case() {
    let b = Break::Nothing;
    kit::the_first_call_makes_the_agent_and_returns_it_ready(fixture(b)).await;
    kit::two_first_calls_at_once_make_one_agent(fixture(b)).await;
    kit::a_second_call_makes_nothing_more(fixture(b)).await;
    kit::a_suspended_agent_is_resumed_and_the_call_waits_for_ready(fixture(b)).await;
    kit::a_kind_has_its_own_agent(fixture(b)).await;
    kit::another_owners_agent_is_never_touched(fixture(b)).await;
    kit::state_never_makes_an_agent(fixture(b)).await;
    kit::suspend_and_delete_of_an_absent_agent_change_nothing(fixture(b)).await;
    kit::a_deleted_agent_is_made_again(fixture(b)).await;
}

#[tokio::test]
async fn a_fake_that_creates_twice_on_concurrent_first_calls_fails_the_one_agent_case() {
    let f = fixture(Break::CreatesTwiceOnConcurrentFirstCalls);
    assert!(goes_red(kit::two_first_calls_at_once_make_one_agent(f)).await);
}

#[tokio::test]
async fn a_fake_that_resumes_without_waiting_fails_the_wait_for_ready_case() {
    let f = fixture(Break::ResumeDoesNotWait);
    assert!(goes_red(kit::a_suspended_agent_is_resumed_and_the_call_waits_for_ready(f)).await);
}

#[tokio::test]
async fn a_fake_that_suspends_another_owners_agent_fails_the_isolation_case() {
    let f = fixture(Break::SuspendsAnotherOwner);
    assert!(goes_red(kit::another_owners_agent_is_never_touched(f)).await);
}
