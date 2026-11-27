use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError, ProvisionedAgent,
};

/// What the provisioner keeps of one agent: its state, and for one that is coming up the instant
/// it is ready. Readiness is a function of the clock, as on a platform that drives the state
/// itself: no caller's wait turns `Saving` into `Ready`, so a call that is dropped (cut by a
/// timeout) leaves the agent coming up, and the next call finds it ready or still coming up.
#[derive(Clone, Copy)]
struct Entry {
    state: AgentState,
    ready_at: Option<Instant>,
}

impl Entry {
    /// An agent that is ready `delay` from `now`.
    fn coming_up(now: Instant, delay: Duration) -> Self {
        Entry {
            state: AgentState::Saving,
            ready_at: Some(now + delay),
        }
    }

    /// Reads the clock: an agent whose instant has come is `Ready`.
    fn settle(&mut self, now: Instant) {
        if self.state == AgentState::Saving && self.ready_at.is_some_and(|at| at <= now) {
            self.state = AgentState::Ready;
            self.ready_at = None;
        }
    }
}

/// An in-memory [`AgentProvisioner`]. An agent it makes is [`AgentState::Saving`] for the delay
/// given to [`with_ready_delay`](Self::with_ready_delay) (none by default), then ready: by the
/// clock, so that a dropped [`ensure_ready`](AgentProvisioner::ensure_ready) strands nothing.
#[derive(Clone, Default)]
pub struct MemoryProvisioner {
    agents: Arc<Mutex<HashMap<(AgentOwner, AgentKind), Entry>>>,
    delay_ms: Arc<AtomicU64>,
    created: Arc<AtomicU64>,
}

impl MemoryProvisioner {
    /// An empty provisioner whose agents are ready at once.
    pub fn new() -> Self {
        Self::default()
    }

    /// Agents stay `Saving` this long after they are made or resumed.
    #[must_use]
    pub fn with_ready_delay(self, delay: Duration) -> Self {
        self.set_ready_delay(delay);
        self
    }

    /// Changes the delay for what is made or resumed from now on.
    pub fn set_ready_delay(&self, delay: Duration) {
        let ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        self.delay_ms.store(ms, Ordering::SeqCst);
    }

    /// How many agents exist, every owner and kind together.
    pub fn len(&self) -> usize {
        self.agents.try_lock().map_or(0, |a| a.len())
    }

    /// Whether no agent exists.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many agents were made (a resume is not one).
    pub fn created(&self) -> u64 {
        self.created.load(Ordering::SeqCst)
    }
}

impl AgentProvisioner for MemoryProvisioner {
    async fn ensure_ready(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        let key = (owner.clone(), kind);
        let ready_at = {
            let mut agents = self.agents.lock().await;
            let now = Instant::now();
            let delay = Duration::from_millis(self.delay_ms.load(Ordering::SeqCst));
            match agents.get(&key).map(|entry| entry.state) {
                Some(AgentState::Deleting) => return Err(ProvisionError::Deleting),
                Some(AgentState::Suspended) => {
                    agents.insert(key.clone(), Entry::coming_up(now, delay));
                }
                Some(_) => {}
                None => {
                    self.created.fetch_add(1, Ordering::SeqCst);
                    agents.insert(key.clone(), Entry::coming_up(now, delay));
                }
            }
            agents.get_mut(&key).and_then(|entry| {
                entry.settle(now);
                entry.ready_at
            })
        };
        // The agent is ready at its instant whoever waits for it; the wait is the caller's, and
        // dropping it leaves the agent as it is.
        if let Some(at) = ready_at {
            tokio::time::sleep_until(at).await;
        }
        let mut agents = self.agents.lock().await;
        let seen = agents.get_mut(&key).map(|entry| {
            entry.settle(Instant::now());
            entry.state
        });
        match seen {
            Some(AgentState::Ready) => Ok(ProvisionedAgent {
                name: kind.agent_name(owner),
                state: AgentState::Ready,
            }),
            Some(AgentState::Deleting) | None => Err(ProvisionError::Deleting),
            Some(last) => Err(ProvisionError::NotReady { last }),
        }
    }

    async fn state(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        let mut agents = self.agents.lock().await;
        Ok(agents.get_mut(&(owner.clone(), kind)).map(|entry| {
            entry.settle(Instant::now());
            entry.state
        }))
    }

    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        if let Some(entry) = self.agents.lock().await.get_mut(&(owner.clone(), kind))
            && entry.state != AgentState::Deleting
        {
            *entry = Entry {
                state: AgentState::Suspended,
                ready_at: None,
            };
        }
        Ok(())
    }

    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.agents.lock().await.remove(&(owner.clone(), kind));
        Ok(())
    }
}
