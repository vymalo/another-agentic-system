//! The local agents behind a transaction-mode pooler (issue 290): the journal's queries use the pool,
//! and the tables' creation and the notifier's `LISTEN` take the direct connection. The routing is
//! proven from the database's side, with the two sets of connections told apart by `application_name`;
//! no pooler is needed, and with one (`ORCH_TEST_DIRECT_DATABASE_URL`) the same case runs through it.
//! Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt as _;
use orch_agent_adam::{LocalAgents, LocalKind, LocalOptions};
use orch_core::{AgentId, AgentTaskState};
use orch_pg_connect::PgOptions;
use orch_ports::{AgentClient as _, AgentEndpoint, SendContent, SendRequest};
use tokio_util::sync::CancellationToken;

const WITHIN: Duration = Duration::from_secs(15);

/// A database of its own on the server (swept by the next run that finds it old), as the pooled
/// suites of `orch-store-postgres` have it: the URL queries go through, and the database itself.
struct Database {
    url: String,
    direct_url: String,
}

fn with_database(url: &str, database: &str) -> String {
    let start = url.find("://").map_or(0, |i| i + 3);
    let path = url[start..].find('/').map_or(url.len(), |i| start + i);
    let query = url[path..].find('?').map_or(url.len(), |i| path + i);
    format!("{}/{}{}", &url[..path], database, &url[query..])
}

/// `url` whose sessions are named `app`, as a deployment would name them: in the URL, which both
/// readers of a connection string (the pools' and the listener's) take.
fn named(url: &str, app: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}application_name={app}")
}

async fn connect(url: &str) -> tokio_postgres::Client {
    url.parse::<PgOptions>()
        .unwrap()
        .connect()
        .await
        .expect("connect")
}

async fn database() -> Option<Database> {
    let url = std::env::var("ORCH_TEST_DATABASE_URL")
        .ok()
        .filter(|v| !v.is_empty())?;
    let direct = std::env::var("ORCH_TEST_DIRECT_DATABASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| url.clone());
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = format!("t_{secs}_{}", uuid::Uuid::now_v7().simple());
    connect(&direct)
        .await
        .batch_execute(&format!("CREATE DATABASE \"{name}\""))
        .await
        .expect("create the database");
    Some(Database {
        url: with_database(&url, &name),
        direct_url: with_database(&direct, &name),
    })
}

fn echo_request() -> SendRequest {
    SendRequest {
        endpoint: AgentEndpoint::local(AgentId::new("echo"), LocalKind::Echo.name()),
        message_id: "m-1".to_owned(),
        context_id: Some("ctx-1".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("hello through the pooler".to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
    }
}

#[tokio::test]
async fn the_tables_and_the_listener_are_the_direct_connection_and_a_run_goes_through() {
    let Some(db) = database().await else {
        eprintln!("skipped: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let observer = connect(&db.direct_url).await;
    // Every DDL statement of the database, with the name of the session that ran it.
    observer
        .batch_execute(
            "CREATE TABLE ddl_audit (app text NOT NULL); \
             CREATE FUNCTION audit_ddl() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN \
             INSERT INTO public.ddl_audit VALUES (current_setting('application_name')); END $$; \
             CREATE EVENT TRIGGER audit_ddl ON ddl_command_end EXECUTE FUNCTION audit_ddl(); \
             DELETE FROM ddl_audit",
        )
        .await
        .unwrap();

    // What the binary does: two connection strings, and `connect` makes the pools.
    let agents = std::sync::Arc::new(
        LocalAgents::connect(
            &named(&db.url, "orch-agents-pooled"),
            Some(&named(&db.direct_url, "orch-agents-direct")),
            LocalOptions::new("direct-test"),
            &[LocalKind::Echo],
        )
        .await
        .unwrap(),
    );
    agents.migrate().await.expect("migrate on the direct pool");
    let apps: Vec<String> = observer
        .query("SELECT DISTINCT app FROM ddl_audit", &[])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(apps, ["orch-agents-direct"], "the tables were made by it");

    let stop = CancellationToken::new();
    let worker = tokio::spawn({
        let (agents, stop) = (agents.clone(), stop.clone());
        async move { agents.run(stop).await }
    });
    // a run goes through: its journal on the pool, its wakeups by the notifier
    let client = agents.client();
    let mut stream = client.send_stream(echo_request()).await.unwrap();
    let finished = tokio::time::timeout(WITHIN, async {
        while let Some(envelope) = stream.next().await {
            if envelope.expect("the stream failed").task_state == Some(AgentTaskState::Completed) {
                return true;
            }
        }
        false
    })
    .await
    .expect("the run finished in time");
    assert!(finished, "the echo run completed");

    // the notifier listens on the direct connection and on no other
    let listening: Vec<String> = observer
        .query(
            "SELECT application_name::text FROM pg_stat_activity \
             WHERE datname = current_database() AND pid <> pg_backend_pid() \
             AND query ILIKE 'LISTEN%'",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(
        listening,
        ["orch-agents-direct"],
        "one listening session, and it is the direct one"
    );
    stop.cancel();
    worker.await.unwrap().unwrap();
}
