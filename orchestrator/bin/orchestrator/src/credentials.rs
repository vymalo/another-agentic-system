//! The deployment's model credentials for the broker's run routes (ADR 0084 decision 4): the
//! bearer the injector adds upstream for a run's model, read from the keys the configuration
//! already resolved (`models.endpoints.<name>.apiKey`). A composition detail of the binary: the
//! broker sees only the `ModelCredentials` port.
//!
//! Fail closed: a model the deployment has no key for, and a key that is blank, is
//! `RunLeaseError::Unavailable` — never an empty `Authorization` header. A vault is S9's (ADR 0084
//! decision 4); this adapter reads the configuration and nothing else.

use std::collections::BTreeMap;

use orch_ports::{ModelCredentials, RunLeaseError, ToolSecret};
use secrecy::ExposeSecret as _;

use crate::config::ModelsSettings;

/// The credentials of the configured endpoints, by endpoint name (the name a run's binding
/// carries). Its `Debug` shows the names, never a key.
#[derive(Default)]
pub struct ConfiguredCredentials {
    tokens: BTreeMap<String, ToolSecret>,
}

impl std::fmt::Debug for ConfiguredCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfiguredCredentials")
            .field("models", &self.tokens.keys().collect::<Vec<&String>>())
            .finish()
    }
}

impl ConfiguredCredentials {
    /// The keys of `settings`' endpoints: an endpoint without a key, and one whose key is blank,
    /// has no credential to lend.
    pub fn build(settings: &ModelsSettings) -> Self {
        let tokens = settings
            .endpoints
            .iter()
            .filter_map(|(name, endpoint)| {
                let key = endpoint.api_key.as_ref()?.expose_secret().trim();
                (!key.is_empty()).then(|| (name.clone(), ToolSecret::new(key)))
            })
            .collect();
        ConfiguredCredentials { tokens }
    }
}

impl ModelCredentials for ConfiguredCredentials {
    async fn bearer(&self, model: &str) -> Result<Option<ToolSecret>, RunLeaseError> {
        match self.tokens.get(model) {
            Some(token) => Ok(Some(token.clone())),
            None => Err(RunLeaseError::Unavailable(format!(
                "no credential for the model {model}"
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::time::Duration;

    use secrecy::SecretString;

    use super::*;
    use crate::config::EndpointSettings;

    fn settings(endpoints: &[(&str, Option<&str>)]) -> ModelsSettings {
        ModelsSettings {
            endpoints: endpoints
                .iter()
                .map(|(name, key)| {
                    (
                        (*name).to_owned(),
                        EndpointSettings {
                            base_url: "http://127.0.0.1:1/v1".to_owned(),
                            base_url_ref: None,
                            api_key: key.map(|key| SecretString::from(key.to_owned())),
                            timeout: Duration::from_secs(1),
                        },
                    )
                })
                .collect(),
            tasks: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn a_key_the_deployment_has_is_lent() {
        let credentials = ConfiguredCredentials::build(&settings(&[("model-a", Some("token-a"))]));
        let token = credentials.bearer("model-a").await.unwrap().unwrap();
        assert_eq!(token.expose(), "token-a");
        // The `Debug` names the model and never the key.
        let shown = format!("{credentials:?}");
        assert!(shown.contains("model-a"), "{shown}");
        assert!(!shown.contains("token-a"), "{shown}");
    }

    #[tokio::test]
    async fn a_model_with_no_key_is_unavailable() {
        let credentials = ConfiguredCredentials::build(&settings(&[("model-a", Some("token-a"))]));
        let error = credentials.bearer("model-b").await.unwrap_err();
        assert!(matches!(&error, RunLeaseError::Unavailable(_)), "{error:?}");
        assert!(!error.to_string().contains("token-a"), "{error}");
        // An endpoint the file lists with no key is the same answer.
        let without = ConfiguredCredentials::build(&settings(&[("model-b", None)]));
        assert!(matches!(
            without.bearer("model-b").await.unwrap_err(),
            RunLeaseError::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn a_blank_key_is_never_an_empty_header() {
        for blank in ["", " ", "\n", "  \t "] {
            let credentials = ConfiguredCredentials::build(&settings(&[("model-a", Some(blank))]));
            let error = credentials.bearer("model-a").await.unwrap_err();
            assert!(
                matches!(&error, RunLeaseError::Unavailable(_)),
                "{blank:?}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_key_is_lent_without_the_white_space_of_its_file() {
        let credentials =
            ConfiguredCredentials::build(&settings(&[("model-a", Some("  token-a \n"))]));
        let token = credentials.bearer("model-a").await.unwrap().unwrap();
        assert_eq!(token.expose(), "token-a");
    }
}
