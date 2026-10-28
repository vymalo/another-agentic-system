//! What must be direct when the request pool goes through a transaction-mode pooler (issue 290):
//! the migrations and the `LISTEN` connection. Each test proves its routing from the database's own
//! point of view (`pg_stat_activity`, `pg_locks`, the event log of DDL), with connections told apart by
//! `application_name`, and needs no pooler to do it. The two tests that need one (`ORCH_TEST_DIRECT_
//! DATABASE_URL`) show what the pooler does to a session-scoped construct.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use futures::StreamExt;
use orch_ports::{Topic, Wakeup};
use orch_store_postgres::{PgStore, PgWakeup};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection};
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
async fn observer(db: &TestDb) -> PgConnection {
    PgConnection::connect(&named(&db.direct_url, "orch-observer"))
        .await
        .expect("observer")
}

/// A pool whose server is not there: any use of it fails. What a request pool is, to a boot that must
/// not need it.
fn unreachable_pool() -> sqlx::PgPool {
    let options = PgConnectOptions::new()
        .host("127.0.0.1")
        .port(1)
        .database("none");
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(500))
        .connect_lazy_with(options)
}

async fn advisory_locks(conn: &mut PgConnection) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND database = \
         (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn count_applied(conn: &mut PgConnection) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
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

    let mut observer = observer(&db).await;
    assert!(count_applied(&mut observer).await > 0);
    // the session lock of the migrator went with its connection
    let held = advisory_locks(&mut observer).await;
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
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut observer)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM ddl_audit")
        .execute(&mut observer)
        .await
        .unwrap();

    PgStore::migrate_direct(&named(&db.direct_url, "orch-direct"))
        .await
        .unwrap();

    let apps: Vec<String> = sqlx::query_scalar("SELECT DISTINCT app FROM ddl_audit")
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
async fn sessions(conn: &mut PgConnection, app: &str) -> Vec<(String, String)> {
    sqlx::query_as(
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

    let mut observer = observer(&db).await;
    let direct = sessions(&mut observer, "orch-direct").await;
    assert_eq!(direct.len(), 1, "one listener session: {direct:?}");
    assert!(
        direct[0].1.contains("LISTEN"),
        "it is the session that listens: {direct:?}"
    );
    let pooled = sessions(&mut observer, "orch-pooled").await;
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
    sqlx::query("SELECT pg_notify('orch_outbox', '')")
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
        let mut observer = observer(&db).await;
        let left = advisory_locks(&mut observer).await;
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
        let mut observer = observer(&db).await;
        assert!(count_applied(&mut observer).await > 0);
        assert_eq!(advisory_locks(&mut observer).await, 0);
    }
}
