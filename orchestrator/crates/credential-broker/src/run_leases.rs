//! The run-lease broker (B1, ADR 0084): the orchestrator's binding, and the injector's lease,
//! over a [`RunBindingStore`] and the deployment's [`ModelCredentials`].

use std::time::Duration;

use jiff::Timestamp;
use orch_core::AccountId;
use orch_ports::{
    ACCOUNT_TOMBSTONE, ModelCredentials, RunBinding, RunBindingStore, RunCaller, RunId, RunLease,
    RunLeaseError, RunLeases, account_of,
};

/// The longest a lease lives (ADR 0084 decision 3).
pub const LEASE_TTL: Duration = Duration::from_secs(30);

/// The longest a binding may live, whatever the orchestrator asks (ADR 0084 decision 1).
pub const MAX_BINDING: Duration = Duration::from_secs(24 * 60 * 60);

/// The broker of run leases. `S` keeps the bindings, `C` the deployment's model credentials.
///
/// A run that ends keeps a tombstone (run, person, end time) for [`ACCOUNT_TOMBSTONE`], 24 hours by
/// default, which only the meter's account route reads (ADR 0108 decision 6); a lease is never lent
/// after the end.
#[derive(Debug)]
pub struct RunLeaseBroker<S, C> {
    bindings: S,
    credentials: C,
    retention: Duration,
}

impl<S, C> RunLeaseBroker<S, C> {
    /// A broker over `bindings` and `credentials`.
    pub fn new(bindings: S, credentials: C) -> Self {
        RunLeaseBroker {
            bindings,
            credentials,
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

    /// `now` less the bound, saturating at the beginning of time.
    fn cutoff(&self, now: Timestamp) -> Timestamp {
        now.checked_sub(self.retention).unwrap_or(Timestamp::MIN)
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
        let now = Timestamp::now();
        self.bindings.end(run, now, self.cutoff(now)).await
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
            RunCaller::Agent(_) | RunCaller::Meter => return Err(RunLeaseError::NotBound),
        }
        let account = account_of(&binding.person)?;
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
        Ok(RunLease::bearer(token, account, expires_at))
    }

    async fn account(&self, run: &RunId, caller: &RunCaller) -> Result<AccountId, RunLeaseError> {
        if *caller != RunCaller::Meter {
            return Err(RunLeaseError::NotBound);
        }
        let person = self
            .bindings
            .person_of(run, self.cutoff(Timestamp::now()))
            .await?
            .ok_or(RunLeaseError::NotBound)?;
        account_of(&person)
    }
}
