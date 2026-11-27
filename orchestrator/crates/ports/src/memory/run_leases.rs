//! [`RunLeases`] in memory: the bindings in a map and the deployment's tokens by model name.
//! Behind the `testkit` feature, like the other memory ports.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use jiff::Timestamp;
use orch_core::{AccountId, UserId};

use crate::{
    ACCOUNT_TOMBSTONE, RunBinding, RunCaller, RunId, RunLease, RunLeaseError, RunLeases,
    ToolSecret, account_of,
};

/// The longest a lease lives (ADR 0084 decision 3).
pub const LEASE_TTL: Duration = Duration::from_secs(30);

/// A binding and, once its run ended, when: the tombstone of ADR 0107 decision 6.
#[derive(Clone, Debug)]
struct Entry {
    binding: RunBinding,
    ended_at: Option<Timestamp>,
}

/// `now` less `retention`, saturating at the beginning of time rather than panicking.
fn cutoff(now: Timestamp, retention: Duration) -> Timestamp {
    now.checked_sub(retention).unwrap_or(Timestamp::MIN)
}

/// A run-lease broker in memory. The tokens are the deployment's provider keys by model name; a
/// binding that names no model, or a model with no token, leases [`RunLeaseError::Unavailable`].
/// A run that ended is answered to the meter for [`ACCOUNT_TOMBSTONE`] (24 hours), or the bound
/// given to [`with_tombstone_retention`](Self::with_tombstone_retention).
#[derive(Debug)]
pub struct MemoryRunLeases {
    tokens: BTreeMap<String, ToolSecret>,
    bindings: Mutex<BTreeMap<RunId, Entry>>,
    retention: Duration,
}

impl Default for MemoryRunLeases {
    fn default() -> Self {
        Self::new(BTreeMap::new())
    }
}

impl MemoryRunLeases {
    /// A broker with the deployment's tokens by model name.
    pub fn new(tokens: BTreeMap<String, ToolSecret>) -> Self {
        MemoryRunLeases {
            tokens,
            bindings: Mutex::new(BTreeMap::new()),
            retention: ACCOUNT_TOMBSTONE,
        }
    }

    /// The same broker with another tombstone bound: how long after its end a run is still
    /// answered to the meter. `Duration::ZERO` forgets a run the moment it ends.
    #[must_use]
    pub fn with_tombstone_retention(mut self, retention: Duration) -> Self {
        self.retention = retention;
        self
    }

    /// A broker with one model's token.
    pub fn with_token(model: impl Into<String>, token: ToolSecret) -> Self {
        Self::new(BTreeMap::from([(model.into(), token)]))
    }

    /// The **live** binding of `run`, if any: what the orchestrator's call left, for a test to
    /// assert. A run that ended has none, whatever its tombstone says.
    pub fn binding(&self, run: &RunId) -> Option<RunBinding> {
        self.bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .filter(|entry| entry.ended_at.is_none())
            .map(|entry| entry.binding.clone())
    }
}

impl RunLeases for MemoryRunLeases {
    async fn bind(&self, binding: RunBinding) -> Result<(), RunLeaseError> {
        let mut bindings = self.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        match bindings.get(&binding.run) {
            // A live binding is bound once: the same binding again is idempotent.
            Some(Entry {
                binding: existing,
                ended_at: None,
            }) if existing.agent != binding.agent
                || existing.person != binding.person
                || existing.model != binding.model =>
            {
                Err(RunLeaseError::Conflict)
            }
            // The same binding again, or a run that ended and is bound anew.
            _ => {
                bindings.insert(
                    binding.run.clone(),
                    Entry {
                        binding,
                        ended_at: None,
                    },
                );
                Ok(())
            }
        }
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunLeaseError> {
        let now = Timestamp::now();
        let keep_after = cutoff(now, self.retention);
        let mut bindings = self.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = bindings.get_mut(run) {
            entry.ended_at.get_or_insert(now);
        }
        bindings.retain(|_, entry| entry.ended_at.is_none_or(|ended| ended >= keep_after));
        Ok(())
    }

    async fn lease(&self, run: &RunId, caller: &RunCaller) -> Result<RunLease, RunLeaseError> {
        let binding = self
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .filter(|entry| entry.ended_at.is_none())
            .map(|entry| entry.binding.clone())
            .ok_or(RunLeaseError::NotBound)?;
        let now = Timestamp::now();
        if binding.expires_at <= now {
            return Err(RunLeaseError::Expired);
        }
        match caller {
            // As the broker: the orchestrator and the injector lease, an agent never does, its own
            // run included (ADR 0084 decision 2, amended 2026-11-24).
            RunCaller::Orchestrator | RunCaller::Injector => {}
            RunCaller::Agent(_) | RunCaller::Meter => return Err(RunLeaseError::NotBound),
        }
        let account = account_of(&binding.person)?;
        let model = binding
            .model
            .clone()
            .ok_or_else(|| RunLeaseError::Unavailable("the binding names no model".to_owned()))?;
        let token = self.tokens.get(&model).cloned().ok_or_else(|| {
            RunLeaseError::Unavailable(format!("no credential for the model {model}"))
        })?;
        let expires_at = now
            .checked_add(LEASE_TTL)
            .unwrap_or(binding.expires_at)
            .min(binding.expires_at);
        Ok(RunLease::bearer(token, account, expires_at))
    }

    async fn account(&self, run: &RunId, caller: &RunCaller) -> Result<AccountId, RunLeaseError> {
        if *caller != RunCaller::Meter {
            return Err(RunLeaseError::NotBound);
        }
        let keep_after = cutoff(Timestamp::now(), self.retention);
        let person: UserId = self
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .filter(|entry| entry.ended_at.is_none_or(|ended| ended > keep_after))
            .map(|entry| entry.binding.person.clone())
            .ok_or(RunLeaseError::NotBound)?;
        account_of(&person)
    }
}

/// The binding store in memory, for the broker's tests.
#[derive(Debug, Default)]
pub struct MemoryRunBindingStore {
    bindings: Mutex<BTreeMap<RunId, Entry>>,
}

impl crate::RunBindingStore for MemoryRunBindingStore {
    async fn upsert(&self, binding: &RunBinding) -> Result<(), RunLeaseError> {
        let mut bindings = self.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        match bindings.get(&binding.run) {
            Some(Entry {
                binding: existing,
                ended_at: None,
            }) if existing.agent != binding.agent
                || existing.person != binding.person
                || existing.model != binding.model =>
            {
                Err(RunLeaseError::Conflict)
            }
            _ => {
                bindings.insert(
                    binding.run.clone(),
                    Entry {
                        binding: binding.clone(),
                        ended_at: None,
                    },
                );
                Ok(())
            }
        }
    }

    async fn end(
        &self,
        run: &RunId,
        ended_at: Timestamp,
        sweep_before: Timestamp,
    ) -> Result<(), RunLeaseError> {
        let mut bindings = self.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = bindings.get_mut(run) {
            entry.ended_at.get_or_insert(ended_at);
        }
        bindings.retain(|_, entry| entry.ended_at.is_none_or(|ended| ended >= sweep_before));
        Ok(())
    }

    async fn get(&self, run: &RunId) -> Result<Option<RunBinding>, RunLeaseError> {
        Ok(self
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .filter(|entry| entry.ended_at.is_none())
            .map(|entry| entry.binding.clone()))
    }

    async fn person_of(
        &self,
        run: &RunId,
        ended_after: Timestamp,
    ) -> Result<Option<UserId>, RunLeaseError> {
        Ok(self
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .filter(|entry| entry.ended_at.is_none_or(|ended| ended > ended_after))
            .map(|entry| entry.binding.person.clone()))
    }
}

/// The deployment's model credentials in memory, by model name.
#[derive(Debug, Default)]
pub struct MemoryModelCredentials {
    tokens: BTreeMap<String, ToolSecret>,
}

impl MemoryModelCredentials {
    /// The credentials by model name.
    pub fn new(tokens: BTreeMap<String, ToolSecret>) -> Self {
        MemoryModelCredentials { tokens }
    }
}

impl crate::ModelCredentials for MemoryModelCredentials {
    async fn bearer(&self, model: &str) -> Result<Option<ToolSecret>, RunLeaseError> {
        Ok(self.tokens.get(model).cloned())
    }
}
