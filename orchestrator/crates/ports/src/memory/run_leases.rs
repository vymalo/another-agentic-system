//! [`RunLeases`] in memory: the bindings in a map and the deployment's tokens by model name.
//! Behind the `testkit` feature, like the other memory ports.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use jiff::Timestamp;

use crate::{RunBinding, RunCaller, RunId, RunLease, RunLeaseError, RunLeases, ToolSecret};

/// The longest a lease lives (ADR 0084 decision 3).
pub const LEASE_TTL: Duration = Duration::from_secs(30);

/// A run-lease broker in memory. The tokens are the deployment's provider keys by model name; a
/// binding that names no model, or a model with no token, leases [`RunLeaseError::Unavailable`].
#[derive(Debug, Default)]
pub struct MemoryRunLeases {
    tokens: BTreeMap<String, ToolSecret>,
    bindings: Mutex<BTreeMap<RunId, RunBinding>>,
}

impl MemoryRunLeases {
    /// A broker with the deployment's tokens by model name.
    pub fn new(tokens: BTreeMap<String, ToolSecret>) -> Self {
        MemoryRunLeases {
            tokens,
            bindings: Mutex::new(BTreeMap::new()),
        }
    }

    /// A broker with one model's token.
    pub fn with_token(model: impl Into<String>, token: ToolSecret) -> Self {
        Self::new(BTreeMap::from([(model.into(), token)]))
    }

    /// The binding of `run`, if any: what the orchestrator's call left, for a test to assert.
    pub fn binding(&self, run: &RunId) -> Option<RunBinding> {
        self.bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .cloned()
    }
}

impl RunLeases for MemoryRunLeases {
    async fn bind(&self, binding: RunBinding) -> Result<(), RunLeaseError> {
        let mut bindings = self.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        match bindings.get(&binding.run) {
            Some(existing)
                if existing.agent != binding.agent
                    || existing.person != binding.person
                    || existing.model != binding.model =>
            {
                Err(RunLeaseError::Conflict)
            }
            Some(_) => {
                bindings.insert(binding.run.clone(), binding);
                Ok(())
            }
            None => {
                bindings.insert(binding.run.clone(), binding);
                Ok(())
            }
        }
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunLeaseError> {
        self.bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(run);
        Ok(())
    }

    async fn lease(&self, run: &RunId, caller: &RunCaller) -> Result<RunLease, RunLeaseError> {
        let binding = self
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .cloned()
            .ok_or(RunLeaseError::NotBound)?;
        let now = Timestamp::now();
        if binding.expires_at <= now {
            return Err(RunLeaseError::Expired);
        }
        match caller {
            // As the broker: the orchestrator and the injector lease, an agent never does, its own
            // run included (ADR 0084 decision 2, amended 2026-11-24).
            RunCaller::Orchestrator | RunCaller::Injector => {}
            RunCaller::Agent(_) => return Err(RunLeaseError::NotBound),
        }
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
        Ok(RunLease::bearer(token, binding.person.clone(), expires_at))
    }
}

/// The binding store in memory, for the broker's tests.
#[derive(Debug, Default)]
pub struct MemoryRunBindingStore {
    bindings: Mutex<BTreeMap<RunId, RunBinding>>,
}

impl crate::RunBindingStore for MemoryRunBindingStore {
    async fn upsert(&self, binding: &RunBinding) -> Result<(), RunLeaseError> {
        let mut bindings = self.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        match bindings.get(&binding.run) {
            Some(existing)
                if existing.agent != binding.agent
                    || existing.person != binding.person
                    || existing.model != binding.model =>
            {
                Err(RunLeaseError::Conflict)
            }
            _ => {
                bindings.insert(binding.run.clone(), binding.clone());
                Ok(())
            }
        }
    }

    async fn delete(&self, run: &RunId) -> Result<(), RunLeaseError> {
        self.bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(run);
        Ok(())
    }

    async fn get(&self, run: &RunId) -> Result<Option<RunBinding>, RunLeaseError> {
        Ok(self
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)
            .cloned())
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
