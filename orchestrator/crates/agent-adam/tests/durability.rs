//! Durability: the journal is the orchestrator's Postgres, so a task outlives the worker that
//! held it, and the prefix keeps the journal apart from every other table and channel.
//!
//! The Postgres cases skip without `ORCH_TEST_DATABASE_URL`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt as _;
use orch_agent_adam::testkit::{LocalWorld, scripted_endpoint};
use orch_agent_adam::{LocalAgents, LocalKind, LocalOptions, TABLE_PREFIX};
use orch_core::AgentTaskState;
use orch_pg_connect::PgOptions;
use orch_ports::{
    AgentClient as _, AgentEnvelope, AgentStream, IdemKey, SendContent, SendRequest, TaskHandle,
};
use tokio::sync::mpsc;
use tokio_postgres::AsyncMessage;

/// Polls `f` until it returns `Some`, or panics after 15 s.
async fn eventually<T, Fut: std::future::Future<Output = Option<T>>>(
    what: &str,
    mut f: impl FnMut() -> Fut,
) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn request(text: &str, message_id: &str, context_id: &str) -> SendRequest {
    SendRequest {
        endpoint: scripted_endpoint("scripted"),
        message_id: message_id.to_owned(),
        context_id: Some(context_id.to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
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

fn state_of(env: &AgentEnvelope) -> Option<AgentTaskState> {
    env.task_state
}

async fn until_working(stream: &mut AgentStream) -> (String, Vec<AgentEnvelope>) {
    let mut seen = Vec::new();
    while let Some(env) = stream.next().await {
        let env = env.expect("the stream failed");
        let working = state_of(&env) == Some(AgentTaskState::Working);
        let task = env.task_id.clone();
        seen.push(env);
        if working {
            return (task, seen);
        }
    }
    panic!("the stream ended before the task was working: {seen:?}");
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut all = Vec::new();
    while let Some(env) = stream.next().await {
        all.push(env.expect("the stream failed"));
    }
    all
}

fn keys(envs: &[AgentEnvelope]) -> Vec<IdemKey> {
    envs.iter()
        .filter(|e| e.update.is_some())
        .map(|e| e.key.clone())
        .collect()
}

async fn worlds() -> Vec<(&'static str, LocalWorld)> {
    let mut all = vec![("memory", LocalWorld::memory())];
    match LocalWorld::postgres().await {
        Some(world) => all.push(("postgres", world)),
        None => eprintln!("skipping the Postgres variant: ORCH_TEST_DATABASE_URL is not set"),
    }
    all
}

async fn postgres() -> Option<LocalWorld> {
    let world = LocalWorld::postgres().await;
    if world.is_none() {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
    }
    world
}

/// The first column of every row `sql` returns, on a connection of `pool`.
async fn column(pool: &deadpool_postgres::Pool, sql: &str) -> Vec<String> {
    pool.get()
        .await
        .unwrap()
        .query(sql, &[])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect()
}

/// A session of its own to `url` that listens on `channels`: the channel of every notification
/// it receives, in order. The session lives as long as the returned client.
async fn listen(
    url: &str,
    channels: &[&str],
) -> (tokio_postgres::Client, mpsc::UnboundedReceiver<String>) {
    let options: PgOptions = url.parse().unwrap();
    let (client, mut connection) = options
        .wire_config()
        .connect(options.tls_connector().unwrap())
        .await
        .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    // A pooled connection's task drops notifications: this one reads them itself.
    tokio::spawn(async move {
        while let Some(Ok(message)) =
            futures::future::poll_fn(|cx| connection.poll_message(cx)).await
        {
            if let AsyncMessage::Notification(note) = message {
                let _ = tx.send(note.channel().to_owned());
            }
        }
    });
    let statements: String = channels.iter().map(|c| format!("LISTEN {c}; ")).collect();
    client.batch_execute(&statements).await.unwrap();
    (client, rx)
}

#[tokio::test]
async fn a_task_survives_its_worker_dying_mid_step() {
    for (backend, world) in worlds().await {
        let mut first = world.instance(LocalWorld::options("first")).await;
        let mut original = first
            .client
            .send_stream(request("gate durable", "m-durable", "ctx-durable"))
            .await
            .unwrap();
        let (task, mut seen) = until_working(&mut original).await;
        // The step is inside the gate: the first worker holds the run's lease.
        eventually("the first worker at the gate", || async {
            (world.gate_waiters() == 1).then_some(())
        })
        .await;

        // The process dies: nothing is released, the lease has to expire (1 s).
        first.kill();
        eventually("the dead worker's step to stop counting", || async {
            (world.gate_waiters() == 0).then_some(())
        })
        .await;

        // A second process on the same journal picks the task up: it re-attaches to it...
        let second = world.instance(LocalWorld::options("second")).await;
        let handle = TaskHandle {
            endpoint: scripted_endpoint("scripted"),
            task_id: task.clone(),
        };
        let resubscribed = second
            .client
            .resubscribe(&handle)
            .await
            .unwrap_or_else(|e| panic!("{backend}: resubscribe failed: {e}"));
        let snapshot = second.client.get_task(&handle).await.unwrap();
        assert_eq!(snapshot.state, AgentTaskState::Working, "{backend}");

        // ...and, once the lease expired, steps it: it waits at the gate again.
        eventually("the second worker at the gate", || async {
            (world.gate_waiters() == 1).then_some(())
        })
        .await;
        world.release_gate();
        let rest = drain(resubscribed).await;
        seen.extend(drain(original).await);

        assert_eq!(world.finished_by(), ["second"], "{backend}");
        let last = rest.iter().rev().find_map(state_of);
        assert_eq!(last, Some(AgentTaskState::Completed), "{backend}: {rest:?}");
        // What the second process reported carries the keys of the original stream, so a
        // dispatcher that fell over from one to the other records each event once.
        let original_keys = keys(&seen);
        assert!(!original_keys.is_empty());
        for key in keys(&rest) {
            assert!(
                original_keys.contains(&key),
                "{backend}: {key:?} is new: {original_keys:?}"
            );
        }
        assert_eq!(
            second.client.get_task(&handle).await.unwrap().state,
            AgentTaskState::Completed,
            "{backend}"
        );
        second.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn tables_and_channels_carry_the_prefix() {
    let Some(world) = postgres().await else {
        return;
    };
    let pool = world.pool("prefix-test", 4).unwrap();
    let instance = world.instance(LocalWorld::options("prefix")).await;

    let tables = column(
        &pool,
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = current_schema() ORDER BY 1",
    )
    .await;
    for table in ["runs", "journal", "meta"] {
        let name = format!("{TABLE_PREFIX}{table}");
        assert!(tables.contains(&name), "{name} is missing from {tables:?}");
    }
    assert!(
        tables.iter().all(|t| !t.starts_with("adam_")),
        "no table has adam's default prefix: {tables:?}"
    );
    assert!(
        tables.iter().all(|t| t.starts_with(TABLE_PREFIX)),
        "nothing but the journal is here: {tables:?}"
    );

    // The channels: a started task is announced on `orch_agent_signals`, and nothing is sent on
    // the default `adam_` channels.
    let (_session, mut notes) = listen(
        world.url().unwrap(),
        &[
            "orch_agent_events",
            "orch_agent_signals",
            "adam_events",
            "adam_signals",
        ],
    )
    .await;
    let stream = instance
        .client
        .send_stream(request("echo channels", "m-channels", "ctx-channels"))
        .await
        .unwrap();
    drain(stream).await;
    let mut channels = Vec::new();
    while let Ok(Some(channel)) =
        tokio::time::timeout(Duration::from_millis(300), notes.recv()).await
    {
        channels.push(channel);
    }
    assert!(
        channels.iter().any(|c| c == "orch_agent_signals"),
        "the start was announced: {channels:?}"
    );
    assert!(
        channels.iter().all(|c| c.starts_with("orch_agent_")),
        "only prefixed channels carry anything: {channels:?}"
    );
    instance.shutdown().await.unwrap();
}

#[tokio::test]
async fn two_prefixes_do_not_collide() {
    use adam_core::{NewRun, Store as _};
    use adam_store_postgres::PgStore;

    let Some(world) = postgres().await else {
        return;
    };
    let pool = world.pool("collide", 8).unwrap();
    let url = world.url().unwrap();
    // The orchestrator's own tables, adam's default prefix and ours, in one schema: the thread
    // store's migrations on a pool of its own over the same URL, which names the schema.
    let options: PgOptions = url.parse().unwrap();
    orch_store_postgres::PgStore::from_pool(orch_store_postgres::PgPool::new(options, 2).unwrap())
        .migrate()
        .await
        .unwrap();
    let ours = LocalAgents::postgres(
        pool.clone(),
        url,
        LocalWorld::options("ours"),
        &[LocalKind::Echo],
    )
    .unwrap();
    ours.migrate().await.unwrap();
    let theirs = PgStore::from_pool(pool.clone());
    theirs.migrate().await.unwrap();

    let tables = column(
        &pool,
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = current_schema() ORDER BY 1",
    )
    .await;
    for table in [
        "threads",
        "events",
        "a2a_bindings",
        "outbox",
        "adam_runs",
        "adam_journal",
        "orch_agent_runs",
        "orch_agent_journal",
    ] {
        assert!(
            tables.iter().any(|t| t == table),
            "{table} is missing from {tables:?}"
        );
    }

    // A run of the same agent name in each: neither sees the other's.
    let mine = theirs
        .create_run(NewRun::new("echo", serde_json::json!({})))
        .await
        .unwrap();
    let ours_store = PgStore::from_pool(pool.clone())
        .with_table_prefix(TABLE_PREFIX)
        .unwrap();
    assert!(ours_store.load_run(mine.id).await.unwrap().is_none());
    let other = ours_store
        .create_run(NewRun::new("echo", serde_json::json!({})))
        .await
        .unwrap();
    assert!(theirs.load_run(other.id).await.unwrap().is_none());
    assert!(theirs.load_run(mine.id).await.unwrap().is_some());
}

#[tokio::test]
async fn migrate_twice_and_concurrently_is_idempotent() {
    let Some(world) = postgres().await else {
        return;
    };
    let pool = world.pool("migrate", 12).unwrap();
    let url = world.url().unwrap();
    // Replicas booting together: each holds its own LocalAgents over the same database.
    let replicas: Vec<LocalAgents> = (0..6)
        .map(|i| {
            LocalAgents::postgres(
                pool.clone(),
                url,
                LocalOptions::new(format!("replica-{i}")),
                &[LocalKind::Echo],
            )
            .unwrap()
        })
        .collect();
    let first = futures::future::join_all(replicas.iter().map(LocalAgents::migrate)).await;
    for outcome in first {
        outcome.unwrap();
    }
    // Again, alone and together.
    replicas[0].migrate().await.unwrap();
    let again = futures::future::join_all(replicas.iter().map(LocalAgents::migrate)).await;
    for outcome in again {
        outcome.unwrap();
    }
    let runs: i64 = pool
        .get()
        .await
        .unwrap()
        .query_one("SELECT count(*) FROM orch_agent_runs", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(runs, 0);
}
