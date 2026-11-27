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
/// What the orchestrator presents to the platform: a bearer token or its workload certificate.
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
    /// The workload certificate from an OpenBao PKI role (`credentials.type: openbao-pki`).
    OpenbaoPki {
        /// OpenBao's address.
        address: String,
        /// The PEM certificates OpenBao's certificate is checked against, read.
        ca_pem: Option<String>,
        /// The Kubernetes auth mount.
        kubernetes_mount: String,
        /// The Kubernetes auth role.
        auth_role: String,
        /// The service-account token file, read at each login; `None`: the pod's own.
        service_account_token_file: Option<String>,
        /// The PKI mount.
        pki_mount: String,
        /// The PKI role.
        pki_role: String,
        /// The SPIFFE id the certificate carries.
        spiffe_id: String,
        /// The life asked of each certificate, in seconds.
        ttl_secs: u64,
        /// The renewal margin, in seconds.
        renewal_margin_secs: u64,
        /// The PEM certificates the platform's listener is checked against, read.
        server_ca_pem: Option<String>,
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
        let credentials = match &section.credentials {
            ProvisionerCredentials::DevOidc {
                token_url,
                client_id,
                user,
                audience,
                roles,
                ..
            } => {
                let secret = valid.secrets.provisioner_client_secret.as_ref()?;
                CredentialSettings::DevOidc {
                    token_url: token_url.clone(),
                    client_id: client_id.clone(),
                    client_secret: SecretString::from(secret.expose().to_owned()),
                    user: user.clone(),
                    audience: audience.clone(),
                    roles: roles.clone(),
                }
            }
            ProvisionerCredentials::OpenbaoPki {
                address,
                kubernetes_mount,
                auth_role,
                service_account_token_file,
                pki_mount,
                pki_role,
                spiffe_id,
                ttl_secs,
                renewal_margin_secs,
                ..
            } => CredentialSettings::OpenbaoPki {
                address: address.clone(),
                ca_pem: valid
                    .secrets
                    .provisioner_openbao_ca
                    .as_ref()
                    .map(|ca| ca.expose().to_owned()),
                kubernetes_mount: kubernetes_mount.clone(),
                auth_role: auth_role.clone(),
                service_account_token_file: service_account_token_file.clone(),
                pki_mount: pki_mount.clone(),
                pki_role: pki_role.clone(),
                spiffe_id: spiffe_id.clone(),
                ttl_secs: *ttl_secs,
                renewal_margin_secs: *renewal_margin_secs,
                server_ca_pem: valid
                    .secrets
                    .provisioner_server_ca
                    .as_ref()
                    .map(|ca| ca.expose().to_owned()),
            },
        };
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
            credentials,
            request_timeout: Duration::from_secs(section.request_timeout_secs),
            ready_timeout: Duration::from_secs(section.ready_timeout_secs),
        })
    }
}

/// What the application's message path holds: the adapter of [`build`], the kinds the file lists
/// and the wait-ready bound (`provisioner.readyTimeoutSecs`), the same one the adapter waits by.
#[cfg(feature = "provisioner-platform")]
pub fn provisioning(settings: &ProvisionerSettings) -> Result<orch_app::Provisioning, ConfigError> {
    use orch_ports::AgentKind;

    let provisioner = build(settings)?;
    let kinds = settings.forms.iter().map(|(kind, _)| match kind {
        orch_config::ProvisionedKind::Coder => AgentKind::Coder,
        orch_config::ProvisionedKind::Workspace => AgentKind::Workspace,
    });
    Ok(orch_app::Provisioning::new(
        provisioner,
        kinds,
        settings.ready_timeout,
    ))
}

/// The adapter, built from the settings. Whatever is wrong is a configuration error naming the key.
#[cfg(feature = "provisioner-platform")]
pub fn build(
    settings: &ProvisionerSettings,
) -> Result<orch_provisioner_platform::PlatformProvisioner, ConfigError> {
    use std::sync::Arc;

    use orch_platform_credentials_openbao::{OpenBaoClientCertificates, OpenBaoPkiConfig};
    use orch_ports::AgentKind;
    use orch_provisioner_platform::{
        BuildError, OidcClientConfig, OidcClientCredentials, PlatformConfig, PlatformProvisioner,
    };

    let invalid = |var: &'static str, e: BuildError| ConfigError::Invalid {
        var,
        reason: e.to_string(),
    };
    let mut config = match &settings.credentials {
        CredentialSettings::DevOidc {
            token_url,
            client_id,
            client_secret,
            user,
            audience,
            roles,
        } => {
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
            PlatformConfig::with_credentials(settings.url.clone(), Arc::new(credentials))
        }
        CredentialSettings::OpenbaoPki {
            address,
            ca_pem,
            kubernetes_mount,
            auth_role,
            service_account_token_file,
            pki_mount,
            pki_role,
            spiffe_id,
            ttl_secs,
            renewal_margin_secs,
            server_ca_pem,
        } => {
            let mut pki = OpenBaoPkiConfig::new(
                address.clone(),
                auth_role.clone(),
                pki_mount.clone(),
                pki_role.clone(),
                spiffe_id.clone(),
            );
            pki.ca_pem = ca_pem.clone();
            pki.kubernetes_mount = kubernetes_mount.clone();
            if let Some(file) = service_account_token_file {
                pki.service_account_token_file = file.into();
            }
            pki.ttl = Duration::from_secs(*ttl_secs);
            pki.renewal_margin = Duration::from_secs(*renewal_margin_secs);
            pki.timeout = settings.request_timeout;
            let certificates =
                OpenBaoClientCertificates::new(pki).map_err(|e| ConfigError::Invalid {
                    var: "provisioner.credentials",
                    reason: e.to_string(),
                })?;
            let mut config = PlatformConfig::with_client_certificate(
                settings.url.clone(),
                Arc::new(certificates),
            );
            if let Some(pem) = server_ca_pem {
                config = config.with_server_ca_pem(pem.clone());
            }
            config
        }
    }
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
