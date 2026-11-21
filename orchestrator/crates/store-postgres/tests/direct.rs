//! What must be direct when the request pool goes through a transaction-mode pooler (issue 290):
//! the migrations and the `LISTEN` connection. Each test proves its routing from the database's own
//! point of view (`pg_stat_activity`, `pg_locks`, the event log of DDL), with connections told apart by
//! `application_name`, and needs no pooler to do it. The two tests that need one (`ORCH_TEST_DIRECT_
//! DATABASE_URL`) show what the pooler does to a session-scoped construct.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;
use orch_store_postgres::PgPool;
use orch_store_postgres::sql::{query, query_as, query_scalar};

use std::time::Duration;

use futures::StreamExt;
use orch_ports::{Topic, Wakeup};
use orch_store_postgres::{PgStore, PgWakeup};
use support::TestDb;

/// How long to wait for something that has to happen: a bound, never a pause.
const WITHIN: Duration = Duration::from_secs(10);

macro_rules! database_or_skip {
    () => {
        match TestDb::isolated_database().await {
            Some(db) => db,
            None => {
                eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
                return;
            }
        }
    };
}

fn named(url: &str, app: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}application_name={app}")
}

/// A direct connection of the test for looking at the database, named so that it is not mistaken for
/// what is being proven.
async fn observer(db: &TestDb) -> tokio_postgres::Client {
    support::connect(&named(&db.direct_url, "orch-observer")).await
}

/// A pool whose server is not there: any use of it fails. What a request pool is, to a boot that must
/// not need it.
fn unreachable_pool() -> PgPool {
    let options = "postgres://127.0.0.1:1/none".parse().unwrap();
    PgPool::with_wait(options, 10, Duration::from_millis(500)).unwrap()
}

async fn advisory_locks(conn: &tokio_postgres::Client) -> i64 {
    query_scalar::<i64>(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND database = \
         (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn count_applied(conn: &tokio_postgres::Client) -> i64 {
    query_scalar::<i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(conn)
        .await
        .expect("count migrations")
}

#[tokio::test]
async fn the_migrations_run_on_the_direct_connection_and_touch_no_pool() {
    let db = database_or_skip!();
    // the request pool cannot connect: a migration that used it would fail
    let store = PgStore::from_pool(unreachable_pool());
    assert!(store.migrate().await.is_err(), "the pool is unreachable");

    PgStore::migrate_direct(&named(&db.direct_url, "orch-direct"))
        .await
        .expect("migrate on the direct connection");

    let observer = observer(&db).await;
    assert!(count_applied(&observer).await > 0);
    // the session lock of the migrator went with its connection
    let held = advisory_locks(&observer).await;
    assert_eq!(held, 0, "no advisory lock is left held");
    // a second run finds nothing to do
    PgStore::migrate_direct(&db.direct_url).await.unwrap();
}

#[tokio::test]
async fn every_ddl_statement_of_the_migrations_is_run_by_the_connection_that_was_named_direct() {
    let db = database_or_skip!();
    let mut observer = observer(&db).await;
    // Every DDL statement of the database, with the name of the session that ran it.
    for statement in [
        "CREATE TABLE ddl_audit (app text NOT NULL)",
        "CREATE FUNCTION audit_ddl() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN \
         INSERT INTO public.ddl_audit VALUES (current_setting('application_name')); END $$",
        "CREATE EVENT TRIGGER audit_ddl ON ddl_command_end EXECUTE FUNCTION audit_ddl()",
    ] {
        query(statement).execute(&mut observer).await.unwrap();
    }
    query("DELETE FROM ddl_audit")
        .execute(&mut observer)
        .await
        .unwrap();

    PgStore::migrate_direct(&named(&db.direct_url, "orch-direct"))
        .await
        .unwrap();

    let apps: Vec<String> = query_scalar("SELECT DISTINCT app FROM ddl_audit")
        .fetch_all(&mut observer)
        .await
        .unwrap();
    assert_eq!(apps, ["orch-direct"], "the migrations ran on one session");
}

#[tokio::test]
async fn a_connection_string_that_is_not_one_is_refused_without_its_text() {
    let secret = "hunter2-the-password";
    let url = format!("postgres://u:{secret}@h:notaport/db");
    let error = PgStore::migrate_direct(&url).await.unwrap_err();
    assert!(!format!("{error} {error:?}").contains(secret), "{error:?}");
    let pool = unreachable_pool();
    let error = PgWakeup::start_direct(pool, &url).unwrap_err();
    assert!(!format!("{error} {error:?}").contains(secret), "{error:?}");
}

/// The state and last statement of the sessions named `app` that are not the one asking.
async fn sessions(conn: &tokio_postgres::Client, app: &str) -> Vec<(String, String)> {
    query_as::<(String, String)>(
        "SELECT state, query FROM pg_stat_activity \
         WHERE application_name = $1 AND pid <> pg_backend_pid() AND datname = current_database()",
    )
    .bind(app)
    .fetch_all(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_listener_is_a_session_of_the_direct_connection_and_the_notify_goes_by_the_pool() {
    let db = database_or_skip!();
    PgStore::migrate_direct(&db.direct_url).await.unwrap();
    let pool = db.pool("orch-pooled", 4).await;
    let wakeup = PgWakeup::start_direct(pool, &named(&db.direct_url, "orch-direct")).unwrap();
    assert!(wakeup.wait_listening(WITHIN).await, "the listener attached");
    let mut events = wakeup.subscribe();

    let observer = observer(&db).await;
    let direct = sessions(&observer, "orch-direct").await;
    assert_eq!(direct.len(), 1, "one listener session: {direct:?}");
    assert!(
        direct[0].1.contains("LISTEN"),
        "it is the session that listens: {direct:?}"
    );
    let pooled = sessions(&observer, "orch-pooled").await;
    assert!(
        pooled.iter().all(|(_, query)| !query.contains("LISTEN")),
        "the request pool never listens: {pooled:?}"
    );

    // a NOTIFY sent by the request pool reaches the listener of the other connection
    wakeup.notify(Topic::Outbox).await.unwrap();
    let heard = tokio::time::timeout(WITHIN, events.next())
        .await
        .expect("the notification arrived");
    assert_eq!(heard, Some(Topic::Outbox));
}

#[tokio::test]
async fn a_listener_that_cannot_connect_is_retried_not_fatal() {
    let db = database_or_skip!();
    let pool = db.pool("orch-pooled", 2).await;
    let wakeup =
        PgWakeup::start_direct(pool, "postgres://nobody@127.0.0.1:1/none").expect("a valid URL");
    // not attached, and the process goes on: the dispatcher polls
    assert!(!wakeup.wait_listening(Duration::from_millis(300)).await);
}

/// Only with a pooler in front (`ORCH_TEST_DIRECT_DATABASE_URL`): a `LISTEN` taken from the pool hears
/// nothing, and the direct one hears the same `NOTIFY`. This is why the listener is not the pool's.
#[tokio::test]
async fn behind_a_pooler_a_listener_taken_from_the_pool_hears_nothing() {
    let db = database_or_skip!();
    if support::direct_database_url().is_none() {
        eprintln!("skipped: no pooler (ORCH_TEST_DIRECT_DATABASE_URL unset)");
        return;
    }
    PgStore::migrate_direct(&db.direct_url).await.unwrap();
    let pooled = PgWakeup::start(db.pool("orch-pooled", 4).await);
    let direct =
        PgWakeup::start_direct(db.pool("orch-pooled-notify", 2).await, &db.direct_url).unwrap();
    assert!(direct.wait_listening(WITHIN).await);
    // the pooler lets the LISTEN statement through, so the listener believes it is attached
    assert!(pooled.wait_listening(WITHIN).await);
    let mut hears_pooled = pooled.subscribe();
    let mut hears_direct = direct.subscribe();

    // sent on a connection that is not the pooler's, so that nothing of the pool is involved
    let mut sender = observer(&db).await;
    query("SELECT pg_notify('orch_outbox', '')")
        .execute(&mut sender)
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(WITHIN, hears_direct.next())
            .await
            .unwrap(),
        Some(Topic::Outbox)
    );
    // the direct listener has heard it, so a listener that was going to hear it has by now: the
    // pooled one has not
    let silence = tokio::time::timeout(Duration::from_millis(500), hears_pooled.next()).await;
    assert!(silence.is_err(), "the pooled listener heard {silence:?}");
}

/// Only with a pooler in front: the migrator's advisory lock is a session lock held across statements,
/// which a transaction-mode pooler cannot keep, so replicas that migrate through the pool race. A round
/// is eight replicas on a fresh database; the witness is a migration that fails (it ran twice) or a
/// lock left held by a session nobody owns. The race is not won every time, so the rounds go on until
/// it shows (at most twelve). This is why the migrations are not the pool's.
#[tokio::test]
async fn behind_a_pooler_replicas_migrating_through_the_pool_race() {
    if support::direct_database_url().is_none() {
        eprintln!("skipped: no pooler (ORCH_TEST_DIRECT_DATABASE_URL unset)");
        return;
    }
    let mut witnessed = Vec::new();
    for round in 0..12 {
        let db = database_or_skip!();
        let mut tasks = Vec::new();
        for replica in 0..8 {
            let pool = db.pool(&format!("orch-replica-{replica}"), 2).await;
            tasks.push(tokio::spawn(async move {
                tokio::time::timeout(WITHIN, PgStore::from_pool(pool).migrate()).await
            }));
        }
        let mut failed = 0;
        for task in tasks {
            if !matches!(task.await.unwrap(), Ok(Ok(()))) {
                failed += 1;
            }
        }
        let observer = observer(&db).await;
        let left = advisory_locks(&observer).await;
        if failed > 0 || left > 0 {
            witnessed.push((round, failed, left));
            break;
        }
    }
    assert!(
        !witnessed.is_empty(),
        "twelve rounds of eight replicas migrated through a transaction-mode pooler without a failure \
         or a leftover lock: the premise of PgStore::migrate_direct no longer holds"
    );
    eprintln!("(round, failed migrations, advisory locks left): {witnessed:?}");
}

#[tokio::test]
async fn replicas_migrating_on_the_direct_connection_are_serialized_and_leave_no_lock() {
    for _ in 0..3 {
        let db = database_or_skip!();
        let tasks: Vec<_> = (0..8)
            .map(|replica| {
                let url = named(&db.direct_url, &format!("orch-direct-{replica}"));
                tokio::spawn(async move { PgStore::migrate_direct(&url).await })
            })
            .collect();
        for task in tasks {
            task.await.unwrap().expect("every replica migrates");
        }
        let observer = observer(&db).await;
        assert!(count_applied(&observer).await > 0);
        assert_eq!(advisory_locks(&observer).await, 0);
    }
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

/// A refused connection is tried again while the wait lasts, as sqlx's pool did: `migrate_direct`, a
/// pool's first connection (`connect_with`) and the store it made all reach a database that starts
/// listening 300 ms after they are called.
#[tokio::test]
async fn a_database_that_comes_up_during_the_wait_is_reached() {
    let db = database_or_skip!();
    let after = Duration::from_millis(300);

    let (target, _) = readdressed(&db.direct_url, 0);
    let port = late_forwarder(target, after);
    let (_, url) = readdressed(&db.direct_url, port);
    let started = std::time::Instant::now();
    PgStore::migrate_direct(&url)
        .await
        .expect("migrated once the server listened");
    assert!(started.elapsed() >= after - Duration::from_millis(50));

    let (target, _) = readdressed(&db.url, 0);
    let port = late_forwarder(target, after);
    let (_, url) = readdressed(&db.url, port);
    let started = std::time::Instant::now();
    let store = PgStore::connect_with(&url, 2)
        .await
        .expect("connected once the server listened");
    assert!(started.elapsed() >= after - Duration::from_millis(50));
    // and the store it made serves
    orch_ports::ThreadStore::ping(&store).await.expect("ping");
}

/// A run of `migrate` that is given up (its future dropped, as a timeout or a shutdown does) while it
/// waits for the migrator's session lock leaves nothing with the pool: its connection was taken out of
/// the pool and is closed with the future, so no session of the pool goes on to take the lock once it
/// is free and keep it. (A connection given back to the pool would still be waiting, and would hold
/// the lock, which no later run of any replica could then get.)
#[tokio::test]
async fn a_migration_that_is_given_up_leaves_no_lock_with_the_pool() {
    let db = database_or_skip!();
    if support::direct_database_url().is_some() {
        // through a pooler the waiting statement runs on the pooler's server connection whoever
        // closes, which is why `migrate_direct` exists
        eprintln!("skipped: a pooler is in front (ORCH_TEST_DIRECT_DATABASE_URL)");
        return;
    }
    PgStore::migrate_direct(&db.direct_url).await.unwrap();
    let holder = observer(&db).await;
    let name: String = query_scalar("SELECT current_database()::text")
        .fetch_one(&holder)
        .await
        .unwrap();
    let key = orch_pg_migrate::lock_id(&name);
    query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&holder)
        .await
        .unwrap();
    let store = PgStore::from_pool(db.pool("orch-given-up", 2).await);
    let given_up = tokio::time::timeout(Duration::from_millis(500), store.migrate()).await;
    assert!(
        given_up.is_err(),
        "the run waited for the lock: {given_up:?}"
    );
    query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&holder)
        .await
        .unwrap();
    let held = || async {
        query_scalar::<i64>(
            "SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE l.locktype = 'advisory' AND a.application_name = 'orch-given-up'",
        )
        .fetch_one(&holder)
        .await
        .unwrap()
    };
    // give a session that was left waiting the time to take the lock
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        held().await,
        0,
        "a session of the pool holds or waits for the lock"
    );
    // and the pool serves, and a later run migrates
    orch_ports::ThreadStore::ping(&store).await.unwrap();
    store.migrate().await.unwrap();
    assert_eq!(held().await, 0);
}

/// What one connection through a [`recorder`] sent, and whether it has ended its side.
#[derive(Default)]
struct Sent {
    bytes: Vec<u8>,
    ended: bool,
}

type Seen = std::sync::Arc<std::sync::Mutex<Vec<std::sync::Arc<std::sync::Mutex<Sent>>>>>;

/// A forwarder on a port of its own to `target`, recording what every client sends.
async fn recorder(target: String) -> (u16, Seen) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Seen::default();
    let all = seen.clone();
    tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            let sent = std::sync::Arc::new(std::sync::Mutex::new(Sent::default()));
            all.lock().unwrap().push(sent.clone());
            let outbound = tokio::net::TcpStream::connect(&target).await.unwrap();
            let (mut from_client, mut to_client) = inbound.into_split();
            let (mut from_server, mut to_server) = outbound.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut from_server, &mut to_client).await;
            });
            tokio::spawn(async move {
                let mut buf = [0_u8; 8192];
                loop {
                    let n = from_client.read(&mut buf).await.unwrap_or(0);
                    if n == 0 || to_server.write_all(&buf[..n]).await.is_err() {
                        sent.lock().unwrap().ended = true;
                        let _ = to_server.shutdown().await;
                        return;
                    }
                    sent.lock().unwrap().bytes.extend_from_slice(&buf[..n]);
                }
            });
        }
    });
    (port, seen)
}

/// A store's pool and the listener of a `PgWakeup::start` on it, used on a runtime of their own,
/// the pool closed and that runtime dropped at once, as a process that exits after `close` drops
/// it: every connection told the server it was leaving (`Terminate`), so the server logs nothing
/// for it (a connection cut under TLS is a `could not receive data from client` line).
#[test]
fn a_closed_pool_ends_its_connections_before_the_process_exits() {
    let Some(url) = support::direct_database_url().or_else(support::database_url) else {
        eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
        return;
    };
    let outer = tokio::runtime::Runtime::new().unwrap();
    let (target, _) = readdressed(&url, 0);
    let (port, seen) = outer.block_on(recorder(target));
    let (_, moved) = readdressed(&url, port);
    let moved = named(&moved, "orch-goodbye");
    let sep = if moved.contains('?') { '&' } else { '?' };
    let moved = format!("{moved}{sep}sslmode=disable");

    let inner = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    inner.block_on(async {
        let store = PgStore::connect_with(&moved, 3).await.unwrap();
        // a pool of its own, which makes no connection: closing it waits for the listener's alone
        let listening = PgPool::new(moved.parse().unwrap(), 1).unwrap();
        let wakeup = PgWakeup::start(listening.clone());
        assert!(wakeup.wait_listening(WITHIN).await, "the listener attached");
        let (a, b) = tokio::join!(
            orch_ports::ThreadStore::ping(&store),
            orch_ports::ThreadStore::ping(&store)
        );
        a.unwrap();
        b.unwrap();
        store.pool().close().await;
        listening.close().await;
    });
    drop(inner);

    let all_ended = |seen: &Seen| {
        let all = seen.lock().unwrap();
        all.len() >= 3 && all.iter().all(|s| s.lock().unwrap().ended)
    };
    let ended = outer.block_on(async {
        tokio::time::timeout(WITHIN, async {
            while !all_ended(&seen) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    });
    assert!(ended.is_ok(), "the connections ended");
    for sent in seen.lock().unwrap().iter() {
        let sent = sent.lock().unwrap();
        assert!(
            sent.bytes.ends_with(&[b'X', 0, 0, 0, 4]),
            "a connection ended without Terminate: {:?}",
            &sent.bytes[sent.bytes.len().saturating_sub(16)..]
        );
    }
}

/// A transaction dropped as its runtime shuts down: the `ROLLBACK` it spawned never runs, and its
/// connection is closed rather than given back to the pool inside the transaction.
#[test]
fn a_transaction_dropped_as_the_runtime_shuts_down_never_goes_back_to_the_pool() {
    let Some(url) = support::database_url() else {
        eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pool = runtime.block_on(async {
        let pool = PgPool::new(url.parse().unwrap(), 1).unwrap();
        let tx = pool.begin().await.unwrap();
        query("SELECT 1").execute(&tx).await.unwrap();
        assert_eq!(pool.size(), 1);
        // its rollback is spawned, and this runtime never runs it
        drop(tx);
        pool
    });
    drop(runtime);
    assert_eq!(pool.size(), 0, "the connection went back to the pool");
}

/// A transaction dropped on a runtime that goes on: rolled back, and its connection given back to
/// the pool (not closed).
#[tokio::test]
async fn a_dropped_transaction_is_rolled_back_and_its_connection_kept() {
    let Some(url) = support::database_url() else {
        eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
        return;
    };
    let pool = PgPool::new(url.parse().unwrap(), 1).unwrap();
    let tx = pool.begin().await.unwrap();
    query("SET LOCAL application_name = 'orch-in-tx'")
        .execute(&tx)
        .await
        .unwrap();
    drop(tx);
    tokio::time::timeout(WITHIN, async {
        while pool.size() == 1 && pool.deadpool().status().available == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(pool.size(), 1, "the connection was closed");
    let name: String = query_scalar("SELECT current_setting('application_name')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(name, "orch-in-tx", "the transaction was not rolled back");
}
