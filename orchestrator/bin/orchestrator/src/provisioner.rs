//! `provisioner` (a person's own agents): the settings read from the file and the credentials the
//! provisioner presents to the Platform API.
//!
//! Off unless the section is present. Only the credentials are built here; the path that makes an
//! agent on a person's first message is not wired yet, so nothing calls the platform.

use std::time::Duration;

use orch_config::{ProvisionedKind, Validated};
use orch_platform_credentials_dev::{DevConfig, DevCredentials};
use orch_ports::AgentKind;

use crate::config::ConfigError;

/// `provisioner`, with the secret read.
pub struct ProvisionerSettings {
    /// The Platform API's base URL.
    pub url: String,
    /// The kinds made per person.
    pub kinds: Vec<AgentKind>,
    /// How long a first message waits for the agent to be ready.
    pub ready_timeout: Duration,
    /// Where the token comes from.
    pub credentials: DevConfig,
}

impl std::fmt::Debug for ProvisionerSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionerSettings")
            .field("url", &self.url)
            .field("kinds", &self.kinds)
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

impl ProvisionerSettings {
    /// The settings of a valid file, or `None` when `provisioner` is absent.
    ///
    /// The file's rules have checked the URLs and resolved the secret, so an error here is not a
    /// path a valid file takes: it is refused, never ignored.
    ///
    /// # Errors
    /// The words for the operator, naming the key.
    pub fn from_file(valid: &Validated) -> Result<Option<Self>, String> {
        let Some(section) = valid.config.provisioner.as_ref() else {
            return Ok(None);
        };
        let dev = section
            .credentials
            .dev
            .as_ref()
            .ok_or("provisioner.credentials: no source of the token")?;
        let client_secret = valid
            .secrets
            .provisioner_dev_client_secret
            .as_ref()
            .ok_or("provisioner.credentials.dev.clientSecret: not read")?;
        let request_timeout = Duration::from_secs(section.request_timeout_secs);
        Ok(Some(ProvisionerSettings {
            url: section.url.clone(),
            kinds: section
                .kinds
                .iter()
                .map(|kind| match kind {
                    ProvisionedKind::Coder => AgentKind::Coder,
                    ProvisionedKind::Workspace => AgentKind::Workspace,
                })
                .collect(),
            ready_timeout: Duration::from_secs(section.ready_timeout_secs),
            credentials: DevConfig {
                url: dev.url.clone(),
                client_id: dev.client_id.clone(),
                client_secret: secrecy::SecretString::from(client_secret.expose().to_owned()),
                audience: dev.audience.clone(),
                timeout: request_timeout,
            },
        }))
    }
}

/// Builds what the provisioner presents to the platform, when the section is present.
///
/// # Errors
/// A [`ConfigError`] naming the key, for a URL the adapter refuses.
pub fn build(
    settings: Option<&ProvisionerSettings>,
) -> Result<Option<DevCredentials>, ConfigError> {
    let Some(settings) = settings else {
        return Ok(None);
    };
    let credentials = DevCredentials::new(settings.credentials.clone()).map_err(|e| {
        ConfigError::Document(vec![format!("provisioner.credentials.dev.url: {e}")])
    })?;
    tracing::info!(
        kinds = ?settings.kinds,
        ready_timeout_secs = settings.ready_timeout.as_secs(),
        "the provisioner's credentials are built from the development issuer (the message path is not wired yet)"
    );
    Ok(Some(credentials))
}
