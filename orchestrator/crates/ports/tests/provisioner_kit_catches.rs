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
use tokio::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    /// Nothing: the control. An agent coming up is ready by the clock, whoever waits.
    Nothing,
    /// A first call checks for the agent, then makes it later, so two at once make two.
    CreatesTwiceOnConcurrentFirstCalls,
    /// A suspended agent is marked ready and returned at once, before it is.
    ResumeDoesNotWait,
    /// A suspend reaches every owner's agent of the kind.
    SuspendsAnotherOwner,
    /// Only the caller's wait turns `Saving` into `Ready`: a call that is dropped leaves the agent
    /// `Saving` for good, and the next call returns it without it being ready.
    DroppedCallStrandsTheAgent,
}

struct Entry {
    owner: AgentOwner,
    kind: AgentKind,
    state: AgentState,
    /// When the agent is ready by the clock; `None` for one that is not coming up by itself.
    ready_at: Option<Instant>,
}

struct Fake {
    how: Break,
    agents: Mutex<Vec<Entry>>,
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

    /// Whether readiness is the clock's (every fake but the one that strands).
    fn by_the_clock(&self) -> bool {
        self.how != Break::DroppedCallStrandsTheAgent
    }

    /// Reads the clock: an agent coming up whose instant has come is ready.
    fn settle(&self) {
        let now = Instant::now();
        for a in self.agents.lock().unwrap().iter_mut() {
            if a.state == AgentState::Saving && a.ready_at.is_some_and(|at| at <= now) {
                a.state = AgentState::Ready;
                a.ready_at = None;
            }
        }
    }

    fn set(&self, owner: &AgentOwner, kind: AgentKind, state: AgentState) {
        for a in self.agents.lock().unwrap().iter_mut() {
            if a.owner == *owner && a.kind == kind {
                a.state = state;
                a.ready_at = None;
            }
        }
    }

    /// The agent is coming up: ready at the instant `delay` from now, when the clock says so.
    fn start(&self, owner: &AgentOwner, kind: AgentKind) -> Instant {
        let at = Instant::now() + self.delay();
        for a in self.agents.lock().unwrap().iter_mut() {
            if a.owner == *owner && a.kind == kind {
                a.state = AgentState::Saving;
                a.ready_at = self.by_the_clock().then_some(at);
            }
        }
        at
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
        self.settle();
        let found = self
            .agents
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.owner == *owner && a.kind == kind)
            .map(|a| (a.state, a.ready_at));
        match found {
            None => {
                if self.how == Break::CreatesTwiceOnConcurrentFirstCalls {
                    // The check and the making are apart: both calls see "none".
                    tokio::time::sleep(self.delay()).await;
                    self.agents.lock().unwrap().push(Entry {
                        owner: owner.clone(),
                        kind,
                        state: AgentState::Ready,
                        ready_at: None,
                    });
                } else {
                    // Check and make together, then the wait.
                    {
                        let mut agents = self.agents.lock().unwrap();
                        if !agents.iter().any(|a| a.owner == *owner && a.kind == kind) {
                            agents.push(Entry {
                                owner: owner.clone(),
                                kind,
                                state: AgentState::Saving,
                                ready_at: None,
                            });
                        }
                    }
                    let at = self.start(owner, kind);
                    tokio::time::sleep_until(at).await;
                    self.set(owner, kind, AgentState::Ready);
                }
            }
            Some((AgentState::Suspended, _)) => {
                if self.how == Break::ResumeDoesNotWait {
                    self.set(owner, kind, AgentState::Ready);
                } else {
                    let at = self.start(owner, kind);
                    tokio::time::sleep_until(at).await;
                    self.set(owner, kind, AgentState::Ready);
                }
            }
            // Coming up by the clock: wait for its instant, whoever started it.
            Some((AgentState::Saving, Some(at))) => {
                tokio::time::sleep_until(at).await;
                self.settle();
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
        self.settle();
        Ok(self
            .agents
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.owner == *owner && a.kind == kind)
            .map(|a| a.state))
    }

    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        for a in self.agents.lock().unwrap().iter_mut() {
            let mine = a.owner == *owner && a.kind == kind;
            if mine || (self.how == Break::SuspendsAnotherOwner && a.kind == kind) {
                a.state = AgentState::Suspended;
                a.ready_at = None;
            }
        }
        Ok(())
    }

    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.agents
            .lock()
            .unwrap()
            .retain(|a| !(a.owner == *owner && a.kind == kind));
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
    kit::a_dropped_call_leaves_the_agent_coming_up_and_the_next_call_finds_it(fixture(b)).await;
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

#[tokio::test]
async fn a_fake_whose_dropped_call_strands_the_agent_fails_the_dropped_call_case() {
    let f = fixture(Break::DroppedCallStrandsTheAgent);
    assert!(
        goes_red(kit::a_dropped_call_leaves_the_agent_coming_up_and_the_next_call_finds_it(f))
            .await
    );
}
