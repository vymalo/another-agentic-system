//! The run-lease broker (B1, ADR 0084): the orchestrator's binding, and the injector's lease,
//! over a [`RunBindingStore`] and the deployment's [`ModelCredentials`].

use std::time::Duration;

use jiff::Timestamp;
use orch_ports::{
    ModelCredentials, RunBinding, RunBindingStore, RunCaller, RunId, RunLease, RunLeaseError,
    RunLeases,
};

/// The longest a lease lives (ADR 0084 decision 3).
pub const LEASE_TTL: Duration = Duration::from_secs(30);

/// The longest a binding may live, whatever the orchestrator asks (ADR 0084 decision 1).
pub const MAX_BINDING: Duration = Duration::from_secs(24 * 60 * 60);

/// The broker of run leases. `S` keeps the bindings, `C` the deployment's model credentials.
#[derive(Debug)]
pub struct RunLeaseBroker<S, C> {
    bindings: S,
    credentials: C,
}

impl<S, C> RunLeaseBroker<S, C> {
    /// A broker over `bindings` and `credentials`.
    pub fn new(bindings: S, credentials: C) -> Self {
        RunLeaseBroker {
            bindings,
            credentials,
        }
    }
}

impl<S, C> RunLeases for RunLeaseBroker<S, C>
where
    S: RunBindingStore,
    C: ModelCredentials,
{
    async fn bind(&self, binding: RunBinding) -> Result<(), RunLeaseError> {
        let now = Timestamp::now();
        let capped = now
            .checked_add(MAX_BINDING)
            .map_or(binding.expires_at, |end| binding.expires_at.min(end));
        self.bindings
            .upsert(&RunBinding {
                expires_at: capped,
                ..binding
            })
            .await
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunLeaseError> {
        self.bindings.delete(run).await
    }

    async fn lease(&self, run: &RunId, caller: &RunCaller) -> Result<RunLease, RunLeaseError> {
        let binding = self
            .bindings
            .get(run)
            .await?
            .ok_or(RunLeaseError::NotBound)?;
        let now = Timestamp::now();
        if binding.expires_at <= now {
            return Err(RunLeaseError::Expired);
        }
        match caller {
            // The orchestrator and the injector read the lease; an agent never does, its own run
            // included (ADR 0084 decision 2, amended 2026-11-24): the binding names the agent, it
            // does not authorise it.
            RunCaller::Orchestrator | RunCaller::Injector => {}
            RunCaller::Agent(_) => return Err(RunLeaseError::NotBound),
        }
        let model = binding
            .model
            .as_deref()
            .ok_or_else(|| RunLeaseError::Unavailable("the binding names no model".to_owned()))?;
        let token = self.credentials.bearer(model).await?.ok_or_else(|| {
            RunLeaseError::Unavailable(format!("no credential for the model {model}"))
        })?;
        let expires_at = now
            .checked_add(LEASE_TTL)
            .unwrap_or(binding.expires_at)
            .min(binding.expires_at);
        Ok(RunLease::bearer(token, expires_at))
    }
}
