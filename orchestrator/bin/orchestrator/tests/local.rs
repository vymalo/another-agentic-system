//! Local agents (`transport: local`, ADR 0015) in the built binary, in a build with the Cargo
//! feature `agent-local`: an `echo` agent answers a run, the roles split the work (a control
//! plane starts a task and steps nothing, a worker steps it), and the journal is in the
//! orchestrator's own database under the prefix `orch_agent_`.
//!
//! Run them with `cargo test -p orchestrator --features agent-local`. They need
//! `ORCH_TEST_DATABASE_URL` (they skip without).
#![cfg(feature = "agent-local")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use orch_testsupport::{Chat, eventually};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../crates/store-postgres/tests/support/mod.rs"]
mod pgdb;

const BIN: &str = env!("CARGO_BIN_EXE_orchestrator");
const AGENTS: &str = "- id: helper\n  name: Helper\n  transport: local\n  agent: echo\n";

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("orch-local-{}", uuid::Uuid::now_v7().simple()));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A running binary, killed on drop so a failed assertion never leaves a process behind.
struct Running {
    child: Child,
    log: PathBuf,
}

impl Running {
    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().unwrap()
    }

    /// Sends SIGTERM and waits for the exit.
    fn terminate(&mut self, within: Duration) -> ExitStatus {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success(), "kill -TERM failed");
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.exited() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the process did not exit within {within:?}; log:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // A failed assertion shows what the binary said: the scratch directory goes with the test.
        if std::thread::panicking() {
            eprintln!("--- log of {} ---\n{}", self.log.display(), self.log());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One replica: the binary on a free port with the agents file of these tests.
struct Replica {
    run: RefCell<Running>,
    base: String,
    client: reqwest::Client,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn database_url_of(db: &pgdb::TestDb) -> String {
    let sep = if db.url.contains('?') { '&' } else { '?' };
    format!("{}{sep}options=-c%20search_path%3D{}", db.url, db.schema)
}

impl Replica {
    fn start(scratch: &Scratch, db: &pgdb::TestDb, name: &str, role: &str) -> Self {
        let agents = scratch.file("agents.yaml");
        fs::write(&agents, AGENTS).unwrap();
        let addr = format!("127.0.0.1:{}", free_port());
        let log = scratch.file(&format!("{name}.log"));
        let out = fs::File::create(&log).unwrap();
        let child = Command::new(BIN)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("DATABASE_URL", database_url_of(db))
            .env("AGENTS_FILE", &agents)
            .env("LISTEN_ADDR", &addr)
            .env("ORCH_ROLE", role)
            .env("ORCH_INSTANCE_ID", name)
            .env("OUTBOX_LEASE_SECS", "5")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap();
        Replica {
            run: RefCell::new(Running { child, log }),
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    async fn wait_ready(&self) {
        eventually("the binary answers /readyz", || async {
            assert!(
                self.run.borrow_mut().exited().is_none(),
                "the binary exited early; log:\n{}",
                self.run.borrow().log()
            );
            let status = self
                .client
                .get(format!("{}/readyz", self.base))
                .send()
                .await
                .ok()
                .map(|r| r.status().as_u16());
            (status == Some(200)).then_some(())
        })
        .await;
    }

    fn chat(&self) -> Chat {
        Chat::new(&self.base, "alice@example.com")
    }

    fn stop(&self, what: &str) {
        let status = self.run.borrow_mut().terminate(Duration::from_secs(20));
        let log = self.run.borrow().log();
        assert!(
            status.success(),
            "{what}: unclean exit {status:?}; log:\n{log}"
        );
        assert!(log.contains("shutting down"), "{what}: {log}");
    }
}

/// The number of runs in the local agents' journal.
async fn journal_runs(db: &pgdb::TestDb) -> i64 {
    let pool = db.pool("local-test", 2).await;
    sqlx::query_scalar("SELECT count(*) FROM orch_agent_runs")
        .fetch_one(&pool)
        .await
        .unwrap()
}

fn run_input(thread: &str, text: &str) -> serde_json::Value {
    Chat::agui_input(thread, "run-1", &[("msg-1", text)], serde_json::json!({}))
}

#[tokio::test]
async fn a_local_echo_agent_answers_through_the_binary() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let replica = Replica::start(&scratch, &db, "all-1", "all");
    replica.wait_ready().await;

    let chat = replica.chat();
    // The local agent is listed like any other, with its own card text.
    let (status, agents) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    assert_eq!(agents[0]["id"], "helper", "{agents}");

    let thread = uuid::Uuid::now_v7().to_string();
    let mut sse = chat
        .agui_run("helper", &run_input(&thread, "hello local binary"))
        .await;
    let frames = sse.collect_frames(Duration::from_secs(20)).await;
    let types: Vec<&str> = frames
        .iter()
        .map(|f| f.event["type"].as_str().unwrap())
        .collect();
    assert_eq!(types.first(), Some(&"RUN_STARTED"), "{types:?}");
    assert_eq!(types.last(), Some(&"RUN_FINISHED"), "{types:?}");
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        serde_json::json!({"type": "success"}),
        "{types:?}"
    );
    let text: String = frames.iter().map(|f| f.event.to_string()).collect();
    assert!(
        text.contains("hello local binary"),
        "the echo reaches the requester: {text}"
    );
    chat.wait_state(&thread, "done").await;

    // The task ran in the orchestrator's own database, under the prefix.
    assert_eq!(journal_runs(&db).await, 1);

    replica.stop("all");
    let log = replica.run.borrow().log();
    assert!(log.contains("local agents ready"), "{log}");
}

#[tokio::test]
async fn a_control_plane_with_a_local_agent_starts_without_stepping() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let control = Replica::start(&scratch, &db, "cp", "control-plane");
    control.wait_ready().await;
    let log = control.run.borrow().log();
    assert!(
        log.contains("local agents ready") && log.contains("\"steps\":false"),
        "the control plane is set up for local agents but does not step: {log}"
    );

    // A run is accepted and recorded, and nothing delivers it: the control plane has no
    // dispatcher and no local worker.
    let chat = control.chat();
    let thread = uuid::Uuid::now_v7().to_string();
    let mut sse = chat
        .agui_run("helper", &run_input(&thread, "hello split"))
        .await;
    let first = sse.next_frame(Duration::from_secs(10)).await.unwrap();
    assert_eq!(first.event["type"], "RUN_STARTED");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(chat.state(&thread).await, "queued");
    assert_eq!(journal_runs(&db).await, 0, "no task was started");

    // A worker joins on the same database, and the run completes there.
    let worker = Replica::start(&scratch, &db, "w1", "worker");
    worker.wait_ready().await;
    chat.wait_state(&thread, "done").await;
    assert_eq!(journal_runs(&db).await, 1);
    let log = worker.run.borrow().log();
    assert!(log.contains("\"steps\":true"), "{log}");

    worker.stop("worker");
    control.stop("control plane");
}
