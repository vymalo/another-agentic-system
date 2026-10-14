//! The composition root of people's own MCP servers (ADR 0046, slice 4a): the three cases of the
//! module's table, built for real. The ones that need a database need `ORCH_TEST_DATABASE_URL` (they
//! skip without).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use orch_core::{Classify as _, ErrorClass};
use orch_ports::{UserAuth, UserToolPorts as _};
use sqlx::postgres::PgPoolOptions;

use super::*;

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../../crates/store-postgres/tests/support/mod.rs"]
mod pgdb;

const SENTINEL: &str = "sentinel-0b6e41-token-do-not-print";

fn owner() -> UserId {
    UserId::new("alice@example.test")
}

fn bearer_server(suffix: &str) -> UserToolServer {
    let target = EgressPolicy::new()
        .check_url("https://mcp.example.com/mcp")
        .unwrap();
    UserToolServer::new(
        UserToolServerId::parse(&format!("u-{suffix}")).unwrap(),
        "Mine",
        &target,
        UserAuth::Bearer,
    )
    .unwrap()
}

/// The settings of a file that enables the feature, with the keys of a `secrets` section or not.
fn settings(keyring: bool) -> UserToolsSettings {
    let _ = keyring;
    UserToolsSettings {
        egress: EgressPolicy::new(),
        allow_http: false,
        client_name: "another-agentic-system".to_owned(),
        client_metadata_url: None,
        #[cfg(feature = "secrets-pg")]
        keyring: keyring.then(|| {
            use orch_secrets_pg::{KeyBytes, KeyId, Keyring};
            Keyring::new(
                KeyId::parse("k1").unwrap(),
                [(KeyId::parse("k1").unwrap(), KeyBytes::from_bytes([7; 32]))],
            )
            .unwrap()
        }),
    }
}

/// The database of a test schema, as a URL the secret store connects to.
fn database_url_of(db: &pgdb::TestDb) -> String {
    let sep = if db.url.contains('?') { '&' } else { '?' };
    format!("{}{sep}options=-c%20search_path%3D{}", db.url, db.schema)
}

#[tokio::test]
async fn the_feature_off_builds_the_refusing_bundle_and_opens_nothing() {
    // a pool that has not connected, to a database that is not there: building must not ask it
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/nothing")
        .unwrap();
    let store = PgStore::from_pool(pool.clone());
    let tools = build(None, "postgres://nobody@127.0.0.1:1/nothing", &store)
        .await
        .unwrap();
    assert_eq!(pool.size(), 0, "no connection was opened");
    assert_eq!(tools.mode(), UserToolsMode::Off);
    assert!(!tools.mode().is_enabled());
    assert_eq!(*tools.egress(), EgressPolicy::new());

    let id = UserToolServerId::parse("u-0123456789").unwrap();
    let error = tools.servers().list(&owner()).await.unwrap_err();
    assert!(
        error.to_string().contains("userToolServers.enabled"),
        "{error}"
    );
    assert!(
        tools
            .secrets()
            .get(&owner(), &SecretServer::credential(&id))
            .await
            .is_err()
    );
    let error = tools.broker().lease(&owner(), &id).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Transient);
    assert_eq!(pool.size(), 0, "and refusing opened none either");
}

#[tokio::test]
async fn enabled_without_secrets_keeps_definitions_and_refuses_every_credential() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let tools = build(Some(&settings(false)), &database_url_of(&db), &store)
        .await
        .unwrap();
    assert_eq!(tools.mode(), UserToolsMode::WithoutCredentials);
    assert!(tools.mode().is_enabled() && !tools.mode().keeps_credentials());

    // the definitions are the table of the thread store's database
    let server = bearer_server("0123456789");
    let revision = tools.servers().insert(&owner(), &server).await.unwrap();
    let kept = tools
        .servers()
        .get(&owner(), server.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((kept.server(), kept.revision()), (&server, revision));

    // and a credential has nowhere to be kept, nor does anything try
    let secret = ToolSecret::new(SENTINEL);
    let error = tools
        .broker()
        .store(&owner(), server.id(), &secret)
        .await
        .unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(!format!("{error} {error:?}").contains(SENTINEL));
    let material = SecretMaterial::new(SENTINEL.as_bytes().to_vec()).unwrap();
    assert!(
        tools
            .secrets()
            .put(&owner(), &SecretServer::credential(server.id()), &material)
            .await
            .is_err()
    );
    // the secret store's table was not even made
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = $1 AND table_name = 'user_secrets'",
    )
    .bind(&db.schema)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(tables, 0, "no secrets section, no secret store");
}

#[cfg(feature = "secrets-pg")]
#[tokio::test]
async fn enabled_with_secrets_keeps_credentials_sealed_in_a_pool_of_its_own() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    // the thread store's pool is two connections, and the test holds both further down
    let migrated = db.store().await;
    drop(migrated);
    let store = PgStore::from_pool(db.pool("thread-store", 2).await);
    let tools = build(Some(&settings(true)), &database_url_of(&db), &store)
        .await
        .unwrap();
    assert_eq!(tools.mode(), UserToolsMode::Full);
    assert!(tools.mode().keeps_credentials());

    let server = bearer_server("0123456789");
    tools.servers().insert(&owner(), &server).await.unwrap();
    let reference = tools
        .broker()
        .store(&owner(), server.id(), &ToolSecret::new(SENTINEL))
        .await
        .unwrap();
    assert!(
        reference.as_str().starts_with("cred-u-0123456789-v"),
        "{reference}"
    );
    let lease = tools.broker().lease(&owner(), server.id()).await.unwrap();
    assert!(lease.is_bearer());
    assert_eq!(lease.secret().expose(), SENTINEL);

    // it is sealed at rest: the row holds no part of the token
    let (ciphertext,): (Vec<u8>,) =
        sqlx::query_as("SELECT ciphertext FROM user_secrets WHERE owner = $1")
            .bind(owner().as_str())
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert!(
        !ciphertext
            .windows(SENTINEL.len())
            .any(|w| w == SENTINEL.as_bytes()),
        "the token is in the row in the clear"
    );

    // a pool of its own: with every connection of the thread store's held, the secret store still
    // answers, so a lock that is held in it can never starve the thread store
    let held = (
        store.pool().acquire().await.unwrap(),
        store.pool().acquire().await.unwrap(),
    );
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tools
            .secrets()
            .get(&owner(), &SecretServer::credential(server.id())),
    )
    .await
    .expect("the secret store waited for the thread store's pool")
    .unwrap();
    assert!(got.is_some());
    drop(held);
}

#[cfg(feature = "secrets-pg")]
#[tokio::test]
async fn a_client_metadata_url_the_broker_refuses_stops_startup_as_a_configuration_error() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let mut bad = settings(true);
    // the file's rules refuse this; the broker does too, and a build that got here is refused
    bad.client_metadata_url = Some("http://chat.example.com/client.json".to_owned());
    let error = build(Some(&bad), &database_url_of(&db), &store)
        .await
        .unwrap_err();
    assert_eq!(crate::exit_code(&error), 78, "{error:#}");
}

#[cfg(feature = "secrets-pg")]
#[tokio::test]
async fn a_secret_store_whose_database_is_down_stops_startup_as_an_outage() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let error = build(
        Some(&settings(true)),
        "postgres://nobody@127.0.0.1:1/nothing",
        &store,
    )
    .await
    .unwrap_err();
    assert_eq!(crate::exit_code(&error), 69, "{error:#}");
    assert!(
        !format!("{error:#}").contains("nobody"),
        "the URL is not echoed: {error:#}"
    );
}
