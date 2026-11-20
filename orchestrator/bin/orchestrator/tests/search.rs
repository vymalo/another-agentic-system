//! `orchestrator search backfill` as a process (ADR 0068): indexes the titles, descriptions and messages the
//! log holds and the search index has not been through, reads only the database of the configuration file,
//! and says how it ended in its exit code. The database test needs `ORCH_TEST_DATABASE_URL` (it skips
//! without); the configuration tests always run.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use jiff::Timestamp;
use orch_core::search::SearchTerms;
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
};
use orch_ports::{
    ArchivedFilter, Commit, NewEvent, NewThreadRecord, SearchRequest, ThreadSearch as _,
    ThreadStore as _,
};
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
            std::env::temp_dir().join(format!("orch-search-{}", uuid::Uuid::now_v7().simple()));
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

/// What the Job has: a configuration file with the database in a variable. It names an agents file that is
/// not there: the command must not read it.
fn config(scratch: &Scratch) -> PathBuf {
    scratch.write(
        "config.yaml",
        "version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: /nowhere/agents.yaml }
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

/// The one line of JSON the command prints on stdout.
fn report(output: &Output) -> serde_json::Value {
    let out = text(&output.stdout);
    assert_eq!(out.lines().count(), 1, "one line on stdout: {out:?}");
    serde_json::from_str(out.trim()).unwrap()
}

#[test]
fn without_a_configuration_file_it_exits_78_and_says_what_it_needs() {
    let out = run(&["search", "backfill"], &[]);
    assert_eq!(code(&out), 78, "stderr: {}", text(&out.stderr));
    assert!(text(&out.stderr).contains("ORCH_CONFIG_FILE"));
    assert!(out.stdout.is_empty(), "no report without a run");
}

#[test]
fn the_flags_of_the_service_and_a_bad_page_are_usage_errors_with_exit_2() {
    let scratch = Scratch::new();
    let config = config(&scratch);
    let url = [("DATABASE_URL", "postgres://u:pw@127.0.0.1:1/db")];
    for args in [
        vec!["--role", "worker", "search", "backfill"],
        vec!["search", "backfill", "--page", "0"],
        vec!["search", "backfill", "--all"],
        vec!["search"],
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
    let out = run(
        &["--config", path_str(&config), "search", "backfill"],
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
async fn it_indexes_the_logs_the_index_has_not_been_through_and_a_second_run_finds_nothing() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let url = db.pinned_url();
    let store = PgStore::connect_with(&url, 2).await.unwrap();
    PgStore::migrate_direct(&db.pinned_direct_url())
        .await
        .unwrap();
    let alice = UserId::new("alice@example.com");
    for n in 0..3 {
        store
            .create_thread(
                NewThreadRecord {
                    id: ThreadId(uuid::Uuid::now_v7()),
                    owner: alice.clone(),
                    title: format!("Walrus {n}"),
                    description: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("coder"),
                        release: None,
                    },
                    context_id: None,
                    rail_parent: None,
                    child_of: None,
                    now: t0(),
                },
                commit(vec![NewEvent {
                    at: t0(),
                    actor: Actor::user(&alice),
                    body: EventBody::UserMessage(UserMessageData::new("an otter message")),
                    idempotency_key: None,
                }]),
            )
            .await
            .unwrap();
    }
    let ask = |q: &str| SearchRequest {
        terms: SearchTerms::parse(q).unwrap(),
        archived: ArchivedFilter::Include,
        include_edits: false,
        after: None,
        limit: 50,
    };
    let wanted_title = store.search_threads(&alice, &ask("walrus")).await.unwrap();
    let wanted_message = store.search_threads(&alice, &ask("otter")).await.unwrap();
    assert_eq!(wanted_title.rows.len(), 3);
    assert_eq!(wanted_message.rows.len(), 3);
    // the log as a build without the index left it
    sqlx::query("DELETE FROM message_terms")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE threads SET search_through = 0, title_terms = NULL, description_terms = NULL",
    )
    .execute(store.pool())
    .await
    .unwrap();
    assert!(
        store
            .search_threads(&alice, &ask("walrus"))
            .await
            .unwrap()
            .rows
            .is_empty()
    );

    let config = config(&scratch);
    let env = [("DATABASE_URL", url.as_str())];
    let first = run(
        &[
            "--config",
            path_str(&config),
            "search",
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
    assert_eq!(json["messages"], 3);
    assert_eq!(json["pages"], 2, "three threads in pages of two");
    assert_eq!(json["skipped"], 0);
    assert_eq!(
        store.search_threads(&alice, &ask("walrus")).await.unwrap(),
        wanted_title
    );
    assert_eq!(
        store.search_threads(&alice, &ask("otter")).await.unwrap(),
        wanted_message
    );

    // and a second run finds nothing to do
    let second = run(&["--config", path_str(&config), "search", "backfill"], &env);
    assert_eq!(code(&second), 0, "stderr: {}", text(&second.stderr));
    let json = report(&second);
    assert_eq!(
        (json["threads"].as_u64(), json["messages"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(json["remaining"], 0);
    assert_eq!(report(&first)["remaining"], 0);

    // a thread indexed under other folding rules waits for the backfill, which says how many remain
    sqlx::query("UPDATE threads SET search_fold = 0 WHERE title = 'Walrus 1'")
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.search_backlog(100).await.unwrap(), 1);
    assert_eq!(
        store
            .search_threads(&alice, &ask("walrus"))
            .await
            .unwrap()
            .rows
            .len(),
        2,
        "the thread under other rules is not found"
    );
    let third = run(&["--config", path_str(&config), "search", "backfill"], &env);
    assert_eq!(code(&third), 0, "stderr: {}", text(&third.stderr));
    let json = report(&third);
    assert_eq!(
        (
            json["threads"].as_u64(),
            json["messages"].as_u64(),
            json["remaining"].as_u64()
        ),
        (Some(1), Some(1), Some(0))
    );
    assert_eq!(
        store.search_threads(&alice, &ask("walrus")).await.unwrap(),
        wanted_title
    );

    // --rebuild goes through every thread again and changes nothing
    let rebuilt = run(
        &[
            "--config",
            path_str(&config),
            "search",
            "backfill",
            "--rebuild",
        ],
        &env,
    );
    assert_eq!(code(&rebuilt), 0, "stderr: {}", text(&rebuilt.stderr));
    let json = report(&rebuilt);
    assert_eq!(
        (json["threads"].as_u64(), json["messages"].as_u64()),
        (Some(3), Some(3))
    );
    assert_eq!(
        store.search_threads(&alice, &ask("otter")).await.unwrap(),
        wanted_message
    );
    // no word a person wrote is in a stream, and neither is the database's password
    for stream in [&out, &err, &text(&second.stdout), &text(&second.stderr)] {
        assert!(
            !stream.contains("otter") && !stream.contains("Walrus"),
            "{stream}"
        );
        assert!(!stream.contains("password"), "{stream}");
    }
}
