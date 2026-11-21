//! The migrations of the sealed secret store on a connection of their own (issue 290), for a pool that
//! goes through a transaction-mode pooler. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::{Duration, Instant};

use orch_pg_connect::PgOptions;
use orch_secrets_pg::{MIGRATIONS_TABLE, PgSecretStore};
use support::{Pool, TestDb, fetch_one, ring_k1};

/// A pool whose server is not there: any use of it fails.
fn unreachable_pool() -> Pool {
    "postgres://127.0.0.1:1/none"
        .parse::<PgOptions>()
        .unwrap()
        .pool(1, Duration::from_millis(500))
        .unwrap()
}

#[tokio::test]
async fn the_migrations_run_on_the_direct_connection_and_touch_no_pool() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = PgSecretStore::from_pool(unreachable_pool(), ring_k1());
    assert!(store.migrate().await.is_err(), "the pool is unreachable");
    // a migration that could not reach the database is an outage (the connection refused, tried
    // again for the pool's wait, as sqlx's pool did)
    let error = store.migrate().await.unwrap_err();
    assert!(orch_secrets_pg::is_outage(&error), "{error:?}");

    PgSecretStore::migrate_direct(&db.pinned_direct_url())
        .await
        .expect("migrate on the direct connection");
    // a second run finds nothing to do
    PgSecretStore::migrate_direct(&db.pinned_direct_url())
        .await
        .expect("again");

    let raw = db.pool("orch-secrets-raw", 2).await;
    let (applied,): (i64,) = fetch_one(
        &raw,
        &format!("SELECT count(*) FROM {MIGRATIONS_TABLE} WHERE success"),
        &[],
    )
    .await;
    assert!(applied > 0);
    // the session lock of the migrator went with its connection
    let (held,): (i64,) = fetch_one(
        &raw,
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND database = \
         (SELECT oid FROM pg_database WHERE datname = current_database())",
        &[],
    )
    .await;
    assert_eq!(held, 0, "no advisory lock is left held");
}

#[tokio::test]
async fn a_connection_string_that_is_not_one_is_refused_without_its_text() {
    let secret = "hunter2-the-password";
    let error = PgSecretStore::migrate_direct(&format!("postgres://u:{secret}@h:notaport/db"))
        .await
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains(secret), "{error:?}");
}

/// `host:port` of a connection string, and the string with `127.0.0.1:port` in its place.
fn readdressed(url: &str, port: u16) -> (String, String) {
    let start = url.find("://").map_or(0, |i| i + 3);
    let end = url[start..]
        .find(['/', '?'])
        .map_or(url.len(), |i| start + i);
    let host_start = url[start..end].rfind('@').map_or(start, |i| start + i + 1);
    let mut target = url[host_start..end].to_owned();
    if !target.contains(':') {
        target.push_str(":5432");
    }
    let moved = format!("{}127.0.0.1:{port}{}", &url[..host_start], &url[end..]);
    (target, moved)
}

/// A port nobody listens on (bound and let go), on which, `after` from now, a forwarder starts to
/// listen and passes every connection on to `target`: a database that comes up while a call waits.
fn late_forwarder(target: String, after: Duration) -> u16 {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        while let Ok((mut inbound, _)) = listener.accept().await {
            let target = target.clone();
            tokio::spawn(async move {
                if let Ok(mut outbound) = tokio::net::TcpStream::connect(&target).await {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }
            });
        }
    });
    port
}

/// A refused connection is tried again while the wait lasts, as sqlx's pool did: `connect` and
/// `migrate_direct` both reach a database that starts listening 300 ms after they are called.
#[tokio::test]
async fn a_database_that_comes_up_during_the_wait_is_reached() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let after = Duration::from_millis(300);

    let (target, _) = readdressed(&db.pinned_direct_url(), 0);
    let port = late_forwarder(target, after);
    let (_, url) = readdressed(&db.pinned_direct_url(), port);
    let started = Instant::now();
    PgSecretStore::migrate_direct(&url)
        .await
        .expect("migrated once the server listened");
    assert!(started.elapsed() >= after - Duration::from_millis(50));

    let (target, _) = readdressed(&db.pinned_url(), 0);
    let port = late_forwarder(target, after);
    let (_, url) = readdressed(&db.pinned_url(), port);
    let started = Instant::now();
    let store = PgSecretStore::connect_with(&url, ring_k1(), 1)
        .await
        .expect("connected once the server listened");
    assert!(started.elapsed() >= after - Duration::from_millis(50));
    // and the store it made serves
    let alice = db.owner("alice");
    let server = orch_ports::SecretServer::credential(
        &orch_ports::UserToolServerId::parse("u-0123456789").unwrap(),
    );
    let got = orch_ports::SecretStore::get(&store, &alice, &server).await;
    assert!(
        matches!(got, Ok(None)),
        "{:?}",
        got.map(|found| found.is_some())
    );
}
