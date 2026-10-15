//! The composition root of people's own MCP servers (ADR 0046, slice 4a): the three cases of the
//! module's table, built for real. The ones that need a database need `ORCH_TEST_DATABASE_URL` (they
//! skip without).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use orch_core::{Classify as _, ErrorClass};
use orch_ports::{CredentialRef, UserAuth, UserToolPorts as _, UserToolsMode};
#[cfg(feature = "secrets-pg")]
use orch_ports::{SecretServer, SecretStore as _};
use sqlx::postgres::PgPoolOptions;
use tracing::instrument::WithSubscriber as _;

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
        callback: Some(
            RedirectUri::parse("https://chat.example.com/api/user-tool-servers/oauth/callback")
                .unwrap(),
        ),
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

/// Where the browser comes back to is built from the file's address, and a path other than the one
/// the router serves is refused again here (the file's rules refuse it first; this is the second
/// look at the place where the address is turned into the redirect URI and the document's only
/// redirect), with the path that is served in the words.
#[test]
fn the_redirect_is_built_only_for_the_path_the_router_serves() {
    let served = orch_api::OAUTH_CALLBACK_PATH;
    assert_eq!(served, orch_config::OAUTH_CALLBACK_PATH);
    let built = callback_redirect(Some(format!("https://chat.example.com{served}")))
        .unwrap()
        .unwrap();
    assert_eq!(
        built.as_str(),
        "https://chat.example.com/api/user-tool-servers/oauth/callback"
    );
    assert!(callback_redirect(None).unwrap().is_none());
    for bad in [
        "https://chat.example.com/cb",
        "https://chat.example.com/",
        "https://chat.example.com/api/user-tool-servers/oauth/callback/",
        "https://chat.example.com/API/user-tool-servers/oauth/callback",
        "https://chat.example.com/api/user-tool-servers/oauth%2Fcallback",
    ] {
        let words = callback_redirect(Some(bad.to_owned())).unwrap_err();
        assert!(
            words.contains(served) && words.starts_with("userToolServers.oauth.callbackUrl"),
            "{bad}: {words}"
        );
    }
    // an address the port does not take at all is still the port's refusal
    assert!(callback_redirect(Some("not a url".to_owned())).is_err());
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
    // and no authorization can be pending: the store refuses, and nothing names a state
    let state = OAuthState::new("0123456789abcdef0123456789abcdef").unwrap();
    let now = jiff::Timestamp::from_second(1_790_000_000).unwrap();
    let error = tools
        .pending()
        .put(&state, &owner(), server.id(), now, now)
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient);
    assert!(!format!("{error} {error:?}").contains("0123456789abcdef"));
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
    // the sealed store is the broker's alone (the bundle has none to give); `assemble` shows it
    let (tools, sealed) = assemble(Some(&settings(true)), &database_url_of(&db), &store)
        .await
        .unwrap();
    let sealed = sealed.expect("a secrets section builds a sealed store");
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

    // the authorizations that have begun are the table of the thread store's database, taken once
    let state = OAuthState::new("0123456789abcdef0123456789abcdef").unwrap();
    let (now, later) = (
        jiff::Timestamp::from_second(1_790_000_000).unwrap(),
        jiff::Timestamp::from_second(1_790_000_600).unwrap(),
    );
    tools
        .pending()
        .put(&state, &owner(), server.id(), later, now)
        .await
        .unwrap();
    let taken = tools.pending().take(&state, now).await.unwrap();
    assert!(taken.is_some_and(|p| p.owner() == &owner() && p.server() == server.id()));
    assert!(
        tools.pending().take(&state, now).await.unwrap().is_none(),
        "once"
    );
    let (kept,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_tool_oauth_pending")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(kept, 0);

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
        sealed.get(&owner(), &SecretServer::credential(server.id())),
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

#[cfg(feature = "secrets-pg")]
#[tokio::test]
async fn a_secret_store_whose_database_refuses_its_migrations_is_not_an_outage() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    // sessions that may write nothing, as a role without the privilege to create the table: the
    // database answers, and refuses; waiting for it to come back would not help, so the process
    // exits 1 like the thread store's rejected statement, and not 69 as for an outage
    let url = format!(
        "{}%20-c%20default_transaction_read_only%3Don",
        database_url_of(&db)
    );
    let error = build(Some(&settings(true)), &url, &store)
        .await
        .unwrap_err();
    assert_eq!(crate::exit_code(&error), 1, "{error:#}");
    assert!(
        format!("{error:#}").contains("cannot apply the migrations of the secret store"),
        "{error:#}"
    );
}

// ------------------------------------------------------------------------------- the role

#[test]
fn only_a_process_that_serves_routes_is_given_the_settings() {
    let settings = settings(true);
    for (role, serves) in [
        (Role::All, true),
        (Role::ControlPlane, true),
        (Role::Worker, false),
    ] {
        assert_eq!(
            settings_for(role, Some(&settings)).is_some(),
            serves,
            "{role}"
        );
        assert!(
            settings_for(role, None).is_none(),
            "{role}: nothing to give"
        );
    }
}

/// A worker serves neither the API nor the thread-tools endpoint, so it builds the refusing bundle and
/// opens nothing: the database here is not there, and a worker is not told so, while a process that
/// serves routes asks it and stops as for an outage.
#[tokio::test]
async fn a_worker_builds_the_refusing_bundle_and_opens_no_connection() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/nothing")
        .unwrap();
    let store = PgStore::from_pool(pool.clone());
    let nowhere = "postgres://nobody@127.0.0.1:1/nothing";

    let (tools, log) = logging(build_for_role(
        Role::Worker,
        Some(&settings(true)),
        nowhere,
        &store,
    ))
    .await;
    let tools = tools.unwrap();
    assert_eq!(tools.mode(), UserToolsMode::Off);
    assert_eq!(pool.size(), 0, "no connection of the thread store's pool");
    assert!(
        log.contains("serves no routes") && log.contains("builds nothing"),
        "a worker says once why the feature is not there: {log}"
    );
    assert!(!log.contains("kept sealed"), "{log}");

    // a worker of a deployment that did not enable the feature has nothing to say
    let (tools, log) = logging(build_for_role(Role::Worker, None, nowhere, &store)).await;
    assert_eq!(tools.unwrap().mode(), UserToolsMode::Off);
    assert!(log.is_empty(), "{log}");

    // and a process that serves routes does open the secret store, and finds no database
    #[cfg(feature = "secrets-pg")]
    for role in [Role::All, Role::ControlPlane] {
        let error = build_for_role(role, Some(&settings(true)), nowhere, &store)
            .await
            .unwrap_err();
        assert_eq!(crate::exit_code(&error), 69, "{role}: {error:#}");
    }
}

/// The roles that serve routes migrate the secret store and hold a broker; a worker leaves the secret
/// store's table unmade.
#[cfg(feature = "secrets-pg")]
#[tokio::test]
async fn a_process_that_serves_routes_builds_the_ports_and_a_worker_does_not() {
    for (role, mode) in [
        (Role::All, UserToolsMode::Full),
        (Role::ControlPlane, UserToolsMode::Full),
        (Role::Worker, UserToolsMode::Off),
    ] {
        let Some(db) = pgdb::TestDb::new().await else {
            return;
        };
        let store = db.store().await;
        let tools = build_for_role(role, Some(&settings(true)), &database_url_of(&db), &store)
            .await
            .unwrap();
        assert_eq!(tools.mode(), mode, "{role}");
        let tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = $1 AND table_name = 'user_secrets'",
        )
        .bind(&db.schema)
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(
            tables,
            i64::from(mode.keeps_credentials()),
            "{role}: the secret store is migrated by the roles that use it"
        );
    }
}

// ------------------------------------------------------- a deployment that lost its secrets section

/// A definition that holds the broker's reference to a credential, as it was left when the
/// `secrets` section was still there.
fn holding_a_credential(suffix: &str) -> UserToolServer {
    bearer_server(suffix)
        .with_credential(CredentialRef::new(&format!("cred-u-{suffix}-v1")).expect("a reference"))
}

/// What the owner of a deployment that took the `secrets` section away reads in the log: how many
/// servers hold a credential that cannot be opened, as a number and nothing else.
#[tokio::test]
async fn a_deployment_without_secrets_says_how_many_servers_hold_a_credential_and_who_not() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let servers = PgUserToolServers::from_pool(store.pool().clone());
    let alice = UserId::new("alice-sentinel-owner@example.test");
    let bob = UserId::new("bob-sentinel-owner@example.test");

    // none stranded: nothing to say about it
    let (tools, log) = logging(build(Some(&settings(false)), &database_url_of(&db), &store)).await;
    assert_eq!(tools.unwrap().mode(), UserToolsMode::WithoutCredentials);
    assert!(!log.contains("hold a credential"), "{log}");

    servers
        .insert(&alice, &holding_a_credential("0123456789"))
        .await
        .unwrap();
    servers
        .insert(&alice, &bearer_server("0123456780"))
        .await
        .unwrap();
    servers
        .insert(&bob, &holding_a_credential("0123456789"))
        .await
        .unwrap();

    let (tools, log) = logging(build(Some(&settings(false)), &database_url_of(&db), &store)).await;
    let tools = tools.unwrap();
    assert!(
        log.contains("2 of people's own MCP servers hold a credential"),
        "{log}"
    );
    assert!(log.contains("can still delete them"), "{log}");
    assert_eq!(log.matches("hold a credential").count(), 1, "once: {log}");
    for private in [
        "sentinel-owner",
        "alice",
        "bob",
        "u-0123456789",
        "u-0123456780",
        "cred-u-",
        "mcp.example.com",
    ] {
        assert!(
            !log.contains(private),
            "the count only, not {private}: {log}"
        );
    }

    // and a person can delete one: the definition goes, the broker is not asked (it would refuse)
    let id = UserToolServerId::parse("u-0123456789").unwrap();
    assert!(tools.broker().revoke(&alice, &id).await.is_err());
    tools.servers().delete(&alice, &id).await.unwrap();
    assert!(tools.servers().get(&alice, &id).await.unwrap().is_none());
    assert_eq!(tools.servers().list(&alice).await.unwrap().len(), 1);
    // bob's, under the same id, is still there
    assert!(tools.servers().get(&bob, &id).await.unwrap().is_some());
    let (_, log) = logging(build(Some(&settings(false)), &database_url_of(&db), &store)).await;
    assert!(
        log.contains("1 of people's own MCP servers hold a credential"),
        "{log}"
    );
}

/// The count is for a deployment that keeps none: with the feature off nothing is read, and with a
/// secret store the credentials are not stranded.
#[tokio::test]
async fn the_stranded_servers_are_counted_only_where_no_credentials_are_kept() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let servers = PgUserToolServers::from_pool(store.pool().clone());
    servers
        .insert(&owner(), &holding_a_credential("0123456789"))
        .await
        .unwrap();

    let (tools, log) = logging(build(None, &database_url_of(&db), &store)).await;
    assert_eq!(tools.unwrap().mode(), UserToolsMode::Off);
    assert!(log.is_empty(), "off says nothing: {log}");

    #[cfg(feature = "secrets-pg")]
    {
        let (tools, log) =
            logging(build(Some(&settings(true)), &database_url_of(&db), &store)).await;
        assert_eq!(tools.unwrap().mode(), UserToolsMode::Full);
        assert!(!log.contains("hold a credential"), "{log}");
    }
}

/// The count is a courtesy: a database that cannot answer it does not stop the process (the thread
/// store's migrations ran a moment before, and the next call says what is wrong).
#[tokio::test]
async fn a_count_that_fails_is_a_warning_and_not_a_stop() {
    let Some(db) = pgdb::TestDb::new().await else {
        return;
    };
    let _migrated = db.store().await;
    // a thread store whose pool is closed: the count cannot be read
    let closed = PgStore::from_pool(db.pool("closed", 1).await);
    closed.pool().close().await;
    let (tools, log) = logging(build(
        Some(&settings(false)),
        &database_url_of(&db),
        &closed,
    ))
    .await;
    assert_eq!(tools.unwrap().mode(), UserToolsMode::WithoutCredentials);
    assert!(log.contains("cannot count"), "{log}");
}

/// A `MakeWriter` that appends to a shared buffer, to read what was logged.
#[derive(Clone, Default)]
struct Logged(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Logged {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logged {
    type Writer = Logged;

    fn make_writer(&'a self) -> Logged {
        self.clone()
    }
}

/// Runs `work` under a subscriber that keeps its output, and returns both.
async fn logging<T>(work: impl std::future::Future<Output = T>) -> (T, String) {
    let buf = Logged::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_ansi(false)
        .finish();
    let out = work.with_subscriber(subscriber).await;
    let bytes = buf
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    (out, String::from_utf8(bytes).unwrap())
}
