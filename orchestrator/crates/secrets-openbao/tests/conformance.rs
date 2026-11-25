//! The `SecretStore` conformance testkit against a dev-mode OpenBao. Skipped unless
//! `ORCH_TEST_OPENBAO_URL` is set; `ORCH_TEST_OPENBAO_TOKEN` is its root token.
#![allow(missing_docs)]

use std::time::Duration;

use orch_secrets_openbao::{OpenBaoConfig, OpenBaoSecretStore};
use secrecy::SecretString;

async fn make_store() -> Option<OpenBaoSecretStore> {
    let address = std::env::var("ORCH_TEST_OPENBAO_URL").ok()?;
    let token =
        std::env::var("ORCH_TEST_OPENBAO_TOKEN").unwrap_or_else(|_| "dev-only-token".into());
    OpenBaoSecretStore::new(OpenBaoConfig {
        address,
        mount: "secret".into(),
        // one prefix per case, so cases never meet
        prefix: format!("test/{}", uuid::Uuid::now_v7()),
        token: SecretString::from(token),
        timeout: Duration::from_secs(10),
        ca_pem: None,
    })
    .ok()
}

async fn break_the_store(store: &OpenBaoSecretStore) {
    store.close().await;
}

orch_ports::secret_store_conformance!(make_store, breaker = break_the_store);
