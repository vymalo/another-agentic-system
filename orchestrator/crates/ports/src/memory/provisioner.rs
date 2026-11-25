use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;

use crate::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError, ProvisionedAgent,
};

/// An in-memory [`AgentProvisioner`]. An agent it makes is [`AgentState::Saving`] for the delay
/// given to [`with_ready_delay`](Self::with_ready_delay) (none by default), then ready.
#[derive(Clone, Default)]
pub struct MemoryProvisioner {
    agents: Arc<Mutex<HashMap<(AgentOwner, AgentKind), AgentState>>>,
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
        {
            let mut agents = self.agents.lock().await;
            match agents.get(&key).copied() {
                Some(AgentState::Deleting) => return Err(ProvisionError::Deleting),
                Some(AgentState::Suspended) | None => {
                    if !agents.contains_key(&key) {
                        self.created.fetch_add(1, Ordering::SeqCst);
                    }
                    agents.insert(key.clone(), AgentState::Saving);
                }
                Some(_) => {}
            }
        }
        // The agent becomes ready after the delay; the wait is the caller's.
        tokio::time::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst))).await;
        let mut agents = self.agents.lock().await;
        if let Some(state) = agents.get_mut(&key)
            && *state == AgentState::Saving
        {
            *state = AgentState::Ready;
        }
        match agents.get(&key).copied() {
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
        Ok(self
            .agents
            .lock()
            .await
            .get(&(owner.clone(), kind))
            .copied())
    }

    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        if let Some(state) = self.agents.lock().await.get_mut(&(owner.clone(), kind))
            && *state != AgentState::Deleting
        {
            *state = AgentState::Suspended;
        }
        Ok(())
    }

    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        self.agents.lock().await.remove(&(owner.clone(), kind));
        Ok(())
    }
}
