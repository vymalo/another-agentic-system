//! The broker's internal run routes of this process (ADR 0084, the orchestrator's side): the one
//! place that knows [`RunBindingClient`], chosen by `RUN_BINDING_URL` and handed to the dispatcher,
//! which binds each run when its stream first names the task id and unbinds it when the run ends.
//!
//! A deployment with no broker configured gets [`ConfiguredRunBindings::Off`], which accepts every
//! call and stores nothing: the switch is the URL, and nothing is bound without it.

use orch_ports::{NoRunBindings, RunBinding, RunBindingError, RunBindings, RunId, ToolSecret};
use secrecy::ExposeSecret as _;

use crate::config::{ConfigError, RunBindingSettings};

/// The one `RunBindings` type of the binary's dispatcher: nothing, or the broker the configuration
/// names. Static dispatch over the two variants (ADR 0009), so a deployment with a broker and one
/// without are the same build.
#[derive(Debug)]
pub enum ConfiguredRunBindings {
    /// No broker: every bind and unbind succeeds and nothing is stored.
    Off(NoRunBindings),
    /// The broker's internal listener.
    Client(orch_credential_broker::RunBindingClient),
}

impl ConfiguredRunBindings {
    /// The client `settings` name, or none.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] for a URL the client refuses, naming the variable and never the
    /// token.
    pub fn build(settings: Option<&RunBindingSettings>) -> Result<Self, ConfigError> {
        let Some(settings) = settings else {
            return Ok(ConfiguredRunBindings::Off(NoRunBindings));
        };
        let client = orch_credential_broker::RunBindingClient::with_timeout(
            &settings.url,
            ToolSecret::new(settings.token.expose_secret().to_owned()),
            settings.timeout,
        )
        .map_err(|e| ConfigError::Invalid {
            var: "RUN_BINDING_URL",
            reason: e.to_string(),
        })?;
        tracing::info!(
            url = %settings.url,
            timeout_secs = settings.timeout.as_secs(),
            "a run is bound at the broker when its task id is seen, and unbound when it ends"
        );
        Ok(ConfiguredRunBindings::Client(client))
    }
}

impl RunBindings for ConfiguredRunBindings {
    async fn bind(&self, binding: RunBinding) -> Result<(), RunBindingError> {
        match self {
            ConfiguredRunBindings::Off(off) => off.bind(binding).await,
            ConfiguredRunBindings::Client(client) => client.bind(binding).await,
        }
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunBindingError> {
        match self {
            ConfiguredRunBindings::Off(off) => off.unbind(run).await,
            ConfiguredRunBindings::Client(client) => client.unbind(run).await,
        }
    }
}
