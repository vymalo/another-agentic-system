//! The `provisioner` section of the configuration, as the composition root uses it: what the
//! platform's adapter is built from, and the adapter. Off unless the section is there.

use std::fmt;
use std::time::Duration;

use orch_config::{Provisioner, ProvisionerCredentials, Validated};
use secrecy::SecretString;

#[cfg(feature = "provisioner-platform")]
use crate::config::ConfigError;

// Without the feature the settings are read and refused, never used.
#[cfg_attr(not(feature = "provisioner-platform"), allow(dead_code))]
/// Where the orchestrator gets the token it presents to the platform.
#[derive(Clone)]
pub enum CredentialSettings {
    /// A development issuer's `client_credentials` grant.
    DevOidc {
        /// The token endpoint.
        token_url: String,
        /// The client id.
        client_id: String,
        /// The client secret.
        client_secret: SecretString,
        /// The `mock-oidc` extension parameters.
        user: Option<String>,
        /// The audience.
        audience: Option<String>,
        /// The roles of the token.
        roles: Option<Vec<String>>,
    },
}

#[cfg_attr(not(feature = "provisioner-platform"), allow(dead_code))]
/// `provisioner`, read and resolved. `Debug` shows no secret.
#[derive(Clone)]
pub struct ProvisionerSettings {
    /// The Platform API's base URL.
    pub url: String,
    /// The form of each provisioned kind, in the file's order of kinds.
    pub forms: Vec<(orch_config::ProvisionedKind, serde_json::Value)>,
    /// What to present.
    pub credentials: CredentialSettings,
    /// One request.
    pub request_timeout: Duration,
    /// Waiting for `ready`.
    pub ready_timeout: Duration,
}

impl fmt::Debug for ProvisionerSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProvisionerSettings")
            .field("url", &self.url)
            .field(
                "kinds",
                &self
                    .forms
                    .iter()
                    .map(|(kind, _)| kind.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("credentials", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ProvisionerSettings {
    /// The section of a valid file, with its secret; `None` without the section.
    pub fn from_file(valid: &Validated) -> Option<Self> {
        let section: &Provisioner = valid.config.provisioner.as_ref()?;
        let secret = valid.secrets.provisioner_client_secret.as_ref()?;
        let ProvisionerCredentials::DevOidc {
            token_url,
            client_id,
            user,
            audience,
            roles,
            ..
        } = &section.credentials;
        Some(Self {
            url: section.url.clone(),
            forms: section
                .kinds
                .iter()
                .filter_map(|kind| {
                    let form = section.forms.get(kind)?;
                    Some((*kind, serde_json::Value::Object(form.clone())))
                })
                .collect(),
            credentials: CredentialSettings::DevOidc {
                token_url: token_url.clone(),
                client_id: client_id.clone(),
                client_secret: SecretString::from(secret.expose().to_owned()),
                user: user.clone(),
                audience: audience.clone(),
                roles: roles.clone(),
            },
            request_timeout: Duration::from_secs(section.request_timeout_secs),
            ready_timeout: Duration::from_secs(section.ready_timeout_secs),
        })
    }
}

/// The adapter, built from the settings. Whatever is wrong is a configuration error naming the key.
#[cfg(feature = "provisioner-platform")]
pub fn build(
    settings: &ProvisionerSettings,
) -> Result<orch_provisioner_platform::PlatformProvisioner, ConfigError> {
    use std::sync::Arc;

    use orch_ports::AgentKind;
    use orch_provisioner_platform::{
        BuildError, OidcClientConfig, OidcClientCredentials, PlatformConfig, PlatformProvisioner,
    };

    let invalid = |var: &'static str, e: BuildError| ConfigError::Invalid {
        var,
        reason: e.to_string(),
    };
    let CredentialSettings::DevOidc {
        token_url,
        client_id,
        client_secret,
        user,
        audience,
        roles,
    } = &settings.credentials;
    let credentials = OidcClientCredentials::new(OidcClientConfig {
        token_url: token_url.clone(),
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
        user: user.clone(),
        audience: audience.clone(),
        roles: roles.clone(),
        timeout: settings.request_timeout,
    })
    .map_err(|e| invalid("provisioner.credentials.tokenUrl", e))?;
    let mut config = PlatformConfig::with_credentials(settings.url.clone(), Arc::new(credentials))
        .with_ready_timeout(settings.ready_timeout);
    config.timeout = settings.request_timeout;
    for (kind, form) in &settings.forms {
        let kind = match kind {
            orch_config::ProvisionedKind::Coder => AgentKind::Coder,
            orch_config::ProvisionedKind::Workspace => AgentKind::Workspace,
        };
        config = config.with_form(kind, form.clone());
    }
    let provisioner =
        PlatformProvisioner::new(config).map_err(|e| invalid("provisioner.url", e))?;
    tracing::info!(
        url = %settings.url,
        kinds = settings.forms.len(),
        "the platform provisioner is built"
    );
    Ok(provisioner)
}
