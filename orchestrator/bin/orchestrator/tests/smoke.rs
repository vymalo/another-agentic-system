//! The built binary as a process: configuration errors are fatal and readable; against a real
//! database it serves the chat API, runs a thread to completion through an A2A agent, and exits
//! cleanly on SIGTERM.
//!
//! The database test needs `ORCH_TEST_DATABASE_URL` (it skips without); the configuration
//! tests always run.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use orch_testsupport::{Chat, FakeAgent, FakeAgentOptions, eventually, shape};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../crates/store-postgres/tests/support/mod.rs"]
mod pgdb;

const BIN: &str = env!("CARGO_BIN_EXE_orchestrator");
const TOKEN: &str = "smoke-token";

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("orch-smoke-{}", uuid::Uuid::now_v7().simple()));
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
        self.wait(within)
    }

    fn wait(&mut self, within: Duration) -> ExitStatus {
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the binary with exactly `env` (no inherited variables: no proxy, no stray config).
fn spawn(scratch: &Scratch, env: &[(&str, &str)]) -> Running {
    let log = scratch.file("out.log");
    let out = fs::File::create(&log).unwrap();
    let child = Command::new(BIN)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    Running { child, log }
}

fn write_agents(scratch: &Scratch, yaml: &str) -> PathBuf {
    let path = scratch.file("agents.yaml");
    fs::write(&path, yaml).unwrap();
    path
}

fn agents_yaml(card_url: &str) -> String {
    format!(
        "- id: fake\n  name: Fake agent\n  cardUrl: {card_url}\n  tokenEnv: SMOKE_AGENT_TOKEN\n"
    )
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn path_str(p: &Path) -> &str {
    p.to_str().unwrap()
}

async fn http_status(client: &reqwest::Client, url: &str) -> Option<u16> {
    client
        .get(url)
        .send()
        .await
        .ok()
        .map(|r| r.status().as_u16())
}

#[test]
fn a_missing_database_url_is_fatal_and_named() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let mut run = spawn(
        &scratch,
        &[
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    let status = run.wait(Duration::from_secs(10));
    assert!(!status.success());
    let log = run.log();
    assert!(log.contains("DATABASE_URL is required"), "{log}");
    assert!(!log.contains(TOKEN), "a secret leaked into the log");
}

#[test]
fn a_missing_agent_token_is_fatal_before_anything_connects() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    // The database is unreachable on purpose: configuration is validated first.
    let mut run = spawn(
        &scratch,
        &[
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
        ],
    );
    let status = run.wait(Duration::from_secs(10));
    assert!(!status.success());
    let log = run.log();
    assert!(log.contains("SMOKE_AGENT_TOKEN"), "{log}");
    assert!(log.contains("unset or empty"), "{log}");
}

#[test]
fn an_unreachable_database_is_fatal() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let mut run = spawn(
        &scratch,
        &[
            ("DATABASE_URL", "postgres://nobody:pw@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    // sqlx gives up on an unreachable database after its 30 s acquire timeout.
    let status = run.wait(Duration::from_secs(90));
    assert!(!status.success());
    let log = run.log();
    assert!(log.contains("cannot connect to Postgres"), "{log}");
    assert!(!log.contains("nobody:pw"), "the database URL leaked: {log}");
}

#[tokio::test]
async fn serves_a_thread_to_completion_and_exits_cleanly_on_sigterm() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let agent = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some(TOKEN.to_owned()),
        ..FakeAgentOptions::default()
    })
    .await;

    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml(&agent.card_url()));
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let sep = if db.url.contains('?') { '&' } else { '?' };
    let database_url = format!("{}{sep}options=-c%20search_path%3D{}", db.url, db.schema);
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
            ("OUTBOX_LEASE_SECS", "5"),
        ],
    ));

    let base = format!("http://{addr}");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    eventually("the binary answers /healthz", || async {
        assert!(
            run.borrow_mut().exited().is_none(),
            "the binary exited early; log:\n{}",
            run.borrow().log()
        );
        (http_status(&client, &format!("{base}/healthz")).await == Some(200)).then_some(())
    })
    .await;
    assert_eq!(
        http_status(&client, &format!("{base}/readyz")).await,
        Some(200)
    );

    // Fail closed: no identity, no service (the health probes above needed none).
    assert_eq!(
        http_status(&client, &format!("{base}/api/agents")).await,
        Some(401)
    );

    let chat = Chat::new(&base, "alice@example.com");
    let (status, agents_body) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    assert_eq!(agents_body[0]["id"], "fake");
    let id = chat.create_thread("fake", "echo smoke", None).await;
    chat.wait_state(&id, "done").await;
    assert_eq!(
        shape(&chat.events(&id).await),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    let calls = agent.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str()),
        "the token named by tokenEnv reaches the agent"
    );

    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    for line in log.lines() {
        let json: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("log line is not JSON ({e}): {line}"));
        assert!(json["level"].is_string(), "{line}");
    }
    assert!(log.contains("orchestrator listening"), "{log}");
    assert!(log.contains("shutting down"), "{log}");
    assert!(!log.contains(TOKEN), "a secret leaked into the log");
}
