//! `orchestrator usage backfill` as a process (ADR 0065): folds into the usage ledger of the people
//! the usage the log holds and the ledger has not been through, reads only the database of the
//! configuration file, and says how it ended in its exit code. The database tests need
//! `ORCH_TEST_DATABASE_URL` (they skip without); the configuration tests always run.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ModelTokens, ModelUsageTotalData, ThreadId,
    ThreadState, TokenCounts, UsageMonth, UserId, UserMessageData,
};
use orch_ports::{Commit, NewEvent, NewThreadRecord, ThreadStore as _, UsageLedger as _};
use orch_store_postgres::PgStore;

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../crates/store-postgres/tests/support/mod.rs"]
mod pgdb;

const BIN: &str = env!("CARGO_BIN_EXE_orchestrator");

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("orch-usage-{}", uuid::Uuid::now_v7().simple()));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the Job has: a configuration file with the database in a variable. It names an agents file, a
/// thread-tools key and a model key that are not there: the command must not read them.
fn config(scratch: &Scratch) -> PathBuf {
    scratch.write(
        "config.yaml",
        "version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: /nowhere/agents.yaml }
threadTools: { url: 'http://orchestrator:8080', secret: { file: /nowhere/thread-tools } }
models: { endpoints: { default: { baseUrl: 'https://m.example.com/v1', apiKey: { env: MODEL_KEY } } } }
log: { format: text }
",
    )
}

/// Runs the binary with exactly `env` (no inherited variables: no proxy, no stray configuration).
fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(BIN)
        .args(args)
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .unwrap()
}

fn path_str(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("the process exited by itself")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The database of a test schema, as a URL the binary connects to.
fn database_url_of(db: &pgdb::TestDb) -> String {
    let sep = if db.url.contains('?') { '&' } else { '?' };
    format!("{}{sep}options=-c%20search_path%3D{}", db.url, db.schema)
}

/// The one line of JSON the command prints on stdout.
fn report(output: &Output) -> serde_json::Value {
    let out = text(&output.stdout);
    assert_eq!(out.lines().count(), 1, "one line on stdout: {out:?}");
    serde_json::from_str(out.trim()).unwrap()
}

#[test]
fn without_a_configuration_file_it_exits_78_and_says_what_it_needs() {
    let out = run(&["usage", "backfill"], &[]);
    assert_eq!(code(&out), 78, "stderr: {}", text(&out.stderr));
    assert!(text(&out.stderr).contains("ORCH_CONFIG_FILE"));
    assert!(out.stdout.is_empty(), "no report without a run");
}

#[test]
fn a_database_that_does_not_resolve_is_named_by_its_key_and_exits_78() {
    let scratch = Scratch::new();
    let config = config(&scratch);
    // DATABASE_URL is unset; the thread-tools key and the model key are not there either, and are
    // not asked for
    let out = run(&["--config", path_str(&config), "usage", "backfill"], &[]);
    assert_eq!(code(&out), 78, "stderr: {}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(err.contains("database.url"), "{err}");
    assert!(
        !err.contains("threadTools") && !err.contains("MODEL_KEY"),
        "only the database is asked for:\n{err}"
    );
}

#[test]
fn the_flags_of_the_service_and_a_bad_page_are_usage_errors_with_exit_2() {
    let scratch = Scratch::new();
    let config = config(&scratch);
    let url = [("DATABASE_URL", "postgres://u:pw@127.0.0.1:1/db")];
    for args in [
        vec!["--role", "worker", "usage", "backfill"],
        vec!["usage", "backfill", "--page", "0"],
        vec!["usage", "backfill", "--all"],
        vec!["usage"],
    ] {
        let mut line = vec!["--config", path_str(&config)];
        line.extend(args.iter().copied());
        let out = run(&line, &url);
        assert_eq!(code(&out), 2, "{args:?}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn a_database_that_cannot_be_reached_exits_69_and_the_url_is_in_no_stream() {
    let scratch = Scratch::new();
    let config = config(&scratch);
    // nothing listens on port 1
    let out = run(
        &["--config", path_str(&config), "usage", "backfill"],
        &[("DATABASE_URL", "postgres://sekrit:hunter2@127.0.0.1:1/db")],
    );
    assert_eq!(code(&out), 69, "stderr: {}", text(&out.stderr));
    for stream in [text(&out.stdout), text(&out.stderr)] {
        assert!(
            !stream.contains("hunter2") && !stream.contains("sekrit"),
            "{stream}"
        );
    }
    assert!(out.stdout.is_empty(), "no report from a run that failed");
}

fn t0() -> Timestamp {
    "2026-01-15T10:00:00Z".parse().unwrap()
}

fn total(task: &str, input: u64) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            path: Vec::new(),
            totals: vec![ModelTokens {
                provider: Some("openai".into()),
                model: "glm".into(),
                tokens: TokenCounts {
                    input_tokens: input,
                    output_tokens: 1,
                    total_tokens: input + 1,
                    ..TokenCounts::default()
                },
            }],
            sandbox: None,
        }),
        idempotency_key: None,
    }
}

fn commit(events: Vec<NewEvent>) -> Commit {
    Commit {
        new_state: ThreadState::Working,
        job: None,
        events,
        outbox: Vec::new(),
        binding: None,
        now: t0(),
        lease: None,
        watches: Vec::new(),
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    }
}

#[tokio::test]
async fn it_folds_in_the_logs_the_ledger_has_not_been_through_and_a_second_run_finds_nothing() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let url = database_url_of(&db);
    let store = PgStore::connect_with(&url, 2).await.unwrap();
    store.migrate().await.unwrap();
    let alice = UserId::new("alice@example.com");
    for n in 0..3_u64 {
        store
            .create_thread(
                NewThreadRecord {
                    id: ThreadId(uuid::Uuid::now_v7()),
                    owner: alice.clone(),
                    title: "t".into(),
                    description: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("coder"),
                        release: None,
                    },
                    context_id: None,
                    rail_parent: None,
                    now: t0(),
                },
                commit(vec![
                    NewEvent {
                        at: t0(),
                        actor: Actor::user(&alice),
                        body: EventBody::UserMessage(UserMessageData::new("hi")),
                        idempotency_key: None,
                    },
                    total("t1", 100 * (n + 1)),
                ]),
            )
            .await
            .unwrap();
    }
    let month = UsageMonth::new(2026, 1).unwrap();
    let wanted = store.month(&alice, month).await.unwrap();
    assert_eq!(wanted.tasks, 3);
    // the log as a build without the ledger left it
    sqlx::query("DELETE FROM usage_tasks")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET usage_through = 0")
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.month(&alice, month).await.unwrap().tasks, 0);

    let config = config(&scratch);
    let env = [("DATABASE_URL", url.as_str())];
    let first = run(
        &[
            "--config",
            path_str(&config),
            "usage",
            "backfill",
            "--page",
            "2",
        ],
        &env,
    );
    let (out, err) = (text(&first.stdout), text(&first.stderr));
    assert_eq!(code(&first), 0, "stdout: {out}\nstderr: {err}");
    let json = report(&first);
    assert_eq!(json["complete"], true);
    assert_eq!(json["threads"], 3);
    assert_eq!(json["events"], 3);
    assert_eq!(json["pages"], 2, "three threads in pages of two");
    assert_eq!(store.month(&alice, month).await.unwrap(), wanted);

    // and a second run finds nothing to do
    let second = run(&["--config", path_str(&config), "usage", "backfill"], &env);
    assert_eq!(code(&second), 0, "stderr: {}", text(&second.stderr));
    let json = report(&second);
    assert_eq!(
        (json["threads"].as_u64(), json["events"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(store.month(&alice, month).await.unwrap(), wanted);
    // the database URL's password is in no stream
    for stream in [&out, &err] {
        assert!(!stream.contains("password"), "{stream}");
    }
}
