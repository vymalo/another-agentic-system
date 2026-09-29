//! The built binary as a process: configuration errors are fatal and readable; against a real
//! database it serves the chat API, runs a thread to completion through an A2A agent, and exits
//! cleanly on SIGTERM.
//!
//! The database tests need `ORCH_TEST_DATABASE_URL` (they skip without); the configuration
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

    /// SIGKILL: the process gets no chance to release anything (a crash, an OOM kill).
    fn kill_hard(&mut self) {
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        assert!(!status.success(), "a killed process cannot exit cleanly");
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
    spawn_logging_to(scratch, "out.log", env)
}

/// [`spawn`], with the output in `log_name` (one per process when a test starts several).
fn spawn_logging_to(scratch: &Scratch, log_name: &str, env: &[(&str, &str)]) -> Running {
    spawn_with_args(scratch, log_name, &[], env)
}

/// [`spawn_logging_to`] with command-line `args` as well.
fn spawn_with_args(
    scratch: &Scratch,
    log_name: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Running {
    let log = scratch.file(log_name);
    let out = fs::File::create(&log).unwrap();
    let child = Command::new(BIN)
        .args(args)
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
    assert_eq!(status.code(), Some(78), "EX_CONFIG");
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
    assert_eq!(status.code(), Some(78), "EX_CONFIG");
    let log = run.log();
    assert!(log.contains("SMOKE_AGENT_TOKEN"), "{log}");
    assert!(log.contains("unset or empty"), "{log}");
}

#[test]
fn help_lists_every_flag_and_variable_and_exits_zero() {
    let scratch = Scratch::new();
    let mut run = spawn_with_args(&scratch, "help.log", &["--help"], &[]);
    let status = run.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(0), "{}", run.log());
    let help = run.log();
    for (flag, var) in [
        ("--database-url", "DATABASE_URL"),
        ("--agents-file", "AGENTS_FILE"),
        ("--listen-addr", "LISTEN_ADDR"),
        ("--surfaces", "ORCH_SURFACES"),
        ("--auth-dev-user", "AUTH_DEV_USER"),
        ("--database-max-connections", "DATABASE_MAX_CONNECTIONS"),
        ("--dispatcher-concurrency", "DISPATCHER_CONCURRENCY"),
        ("--outbox-lease-secs", "OUTBOX_LEASE_SECS"),
        ("--shutdown-grace-secs", "SHUTDOWN_GRACE_SECS"),
        ("--instance-id", "ORCH_INSTANCE_ID"),
        ("--log-format", "LOG_FORMAT"),
    ] {
        assert!(help.contains(flag), "--help lacks {flag}:\n{help}");
        assert!(help.contains(var), "--help lacks {var}:\n{help}");
    }
}

#[test]
fn every_setting_is_read_from_its_variable() {
    // A bad value in the environment alone must be refused by name: the fallback is wired for
    // each variable, not only for the ones the other tests happen to set.
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    for (var, bad) in [
        ("LISTEN_ADDR", "nowhere"),
        ("ORCH_SURFACES", "agui"),
        ("ORCH_SURFACES", ","),
        ("AUTH_DEV_USER", "not-an-email"),
        ("DATABASE_MAX_CONNECTIONS", "1"),
        ("DISPATCHER_CONCURRENCY", "0"),
        ("OUTBOX_LEASE_SECS", "2"),
        ("SHUTDOWN_GRACE_SECS", "0"),
    ] {
        let mut run = spawn(
            &scratch,
            &[
                ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
                ("AGENTS_FILE", path_str(&agents)),
                ("SMOKE_AGENT_TOKEN", TOKEN),
                (var, bad),
            ],
        );
        let status = run.wait(Duration::from_secs(10));
        assert_eq!(
            status.code(),
            Some(78),
            "{var}={bad}: EX_CONFIG; {}",
            run.log()
        );
        let log = run.log();
        assert!(
            log.contains(&format!("{var} is invalid")),
            "{var}={bad}: {log}"
        );
    }
}

#[test]
fn a_flag_wins_over_its_variable() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    // The variable is valid; the flag is not, and the flag decides.
    let mut run = spawn_with_args(
        &scratch,
        "flag.log",
        &["--surfaces", "agui"],
        &[
            ("ORCH_SURFACES", "chat-api"),
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    let status = run.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(78), "{}", run.log());
    let log = run.log();
    assert!(
        log.contains("unknown surface") && log.contains("agui"),
        "{log}"
    );
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let scratch = Scratch::new();
    let mut run = spawn_with_args(&scratch, "usage.log", &["--no-such-flag"], &[]);
    let status = run.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(2), "clap's usage error: {}", run.log());
    assert!(run.log().contains("--no-such-flag"), "{}", run.log());
}

#[tokio::test]
async fn the_chat_api_surface_is_mounted_by_the_flag() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let addr = format!("127.0.0.1:{}", free_port());
    let database_url = database_url_of(&db);
    let run = std::cell::RefCell::new(spawn_with_args(
        &scratch,
        "surfaces.log",
        &["--surfaces", "chat-api", "--listen-addr", &addr],
        &[
            ("DATABASE_URL", &database_url),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
        ],
    ));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let base = format!("http://{addr}");
    eventually("the binary answers /healthz", || async {
        assert!(
            run.borrow_mut().exited().is_none(),
            "the binary exited early; log:\n{}",
            run.borrow().log()
        );
        (http_status(&client, &format!("{base}/healthz")).await == Some(200)).then_some(())
    })
    .await;
    // `createThread` is served (an empty body is a 400, not a 404/405), and so is the
    // resource API beside it.
    let post = client
        .post(format!("{base}/api/threads"))
        .header("X-Auth-Request-Email", "alice@example.com")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status().as_u16(), 400);
    let agents_resp = client
        .get(format!("{base}/api/agents"))
        .header("X-Auth-Request-Email", "alice@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(agents_resp.status().as_u16(), 200);
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("\"surfaces\":\"chat-api\""), "{log}");
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
    assert_eq!(status.code(), Some(69), "EX_UNAVAILABLE");
    let log = run.log();
    assert!(log.contains("cannot connect to Postgres"), "{log}");
    assert!(!log.contains("nobody:pw"), "the database URL leaked: {log}");
}

#[tokio::test]
async fn a_taken_listen_address_is_fatal_with_its_own_exit_code() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let database_url = database_url_of(&db);
    let mut run = spawn(
        &scratch,
        &[
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    let status = run.wait(Duration::from_secs(30));
    assert_eq!(status.code(), Some(71), "EX_OSERR: {}", run.log());
    let log = run.log();
    assert!(log.contains("cannot listen on"), "{log}");
    drop(taken);
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
    let database_url = database_url_of(&db);
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

/// The orchestrator database URL confined to the test's schema.
fn database_url_of(db: &pgdb::TestDb) -> String {
    let sep = if db.url.contains('?') { '&' } else { '?' };
    format!("{}{sep}options=-c%20search_path%3D{}", db.url, db.schema)
}

/// One orchestrator process of a multi-process test: its own address and log, the database and
/// the agent list shared with its peers.
struct Replica {
    run: std::cell::RefCell<Running>,
    base: String,
    client: reqwest::Client,
}

impl Replica {
    /// Starts `log_name`'s process; `extra` is added to the common environment.
    fn start(
        scratch: &Scratch,
        log_name: &str,
        database_url: &str,
        agents: &Path,
        extra: &[(&str, &str)],
    ) -> Self {
        let addr = format!("127.0.0.1:{}", free_port());
        let mut env = vec![
            ("DATABASE_URL", database_url),
            ("LISTEN_ADDR", addr.as_str()),
            ("AGENTS_FILE", path_str(agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
        ];
        env.extend_from_slice(extra);
        Replica {
            run: std::cell::RefCell::new(spawn_logging_to(scratch, log_name, &env)),
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    /// Waits until `/healthz` and `/readyz` both answer 200 (panics if the process exits first).
    async fn wait_ready(&self) {
        eventually("the binary answers /healthz", || async {
            assert!(
                self.run.borrow_mut().exited().is_none(),
                "the binary exited early; log:\n{}",
                self.run.borrow().log()
            );
            (http_status(&self.client, &format!("{}/healthz", self.base)).await == Some(200))
                .then_some(())
        })
        .await;
        assert_eq!(
            http_status(&self.client, &format!("{}/readyz", self.base)).await,
            Some(200),
            "log:\n{}",
            self.run.borrow().log()
        );
    }

    fn chat(&self) -> Chat {
        Chat::new(&self.base, "alice@example.com")
    }
}

/// An agent behind the bearer token the replicas are configured with, and its `agents.yaml`.
async fn agent_and_list(scratch: &Scratch) -> (FakeAgent, PathBuf) {
    let agent = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some(TOKEN.to_owned()),
        ..FakeAgentOptions::default()
    })
    .await;
    let agents = write_agents(scratch, &agents_yaml(&agent.card_url()));
    (agent, agents)
}

#[tokio::test]
async fn sigkill_mid_task_then_a_second_process_finishes_it() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);

    // A short lease: after a crash the other process may take the delegation over in 3 s (the smallest the binary accepts).
    let first = Replica::start(
        &scratch,
        "a.log",
        &url,
        &agents,
        &[("OUTBOX_LEASE_SECS", "3")],
    );
    first.wait_ready().await;
    let chat = first.chat();
    let id = chat.create_thread("fake", "gate sigkill", None).await;
    chat.wait_state(&id, "working").await;
    eventually("the agent executes the task", || async {
        (agent.executions().len() == 1).then_some(())
    })
    .await;

    // The process dies without a word: no shutdown, no lease release.
    first.run.borrow_mut().kill_hard();

    let second = Replica::start(
        &scratch,
        "b.log",
        &url,
        &agents,
        &[("OUTBOX_LEASE_SECS", "3")],
    );
    second.wait_ready().await;
    let chat = second.chat();
    // Once the lease has expired the second process re-attaches to the running task; only then
    // does the agent finish, so the rest of the run really is seen by the second process.
    eventually("the second process re-attaches to the task", || async {
        assert!(
            second.run.borrow_mut().exited().is_none(),
            "the second process died; log:\n{}",
            second.run.borrow().log()
        );
        (agent.rpc_count("subscribe_to_task") >= 1).then_some(())
    })
    .await;
    agent.release_gate();
    chat.wait_state(&id, "done").await;

    assert_eq!(
        agent.rpc_count("send_streaming_message"),
        1,
        "the message must reach the agent exactly once"
    );
    assert_eq!(agent.executions().len(), 1);
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ],
        "each update exactly once"
    );
    let seqs: Vec<i64> = events.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5], "seq stays contiguous");
    let status = second.run.borrow_mut().terminate(Duration::from_secs(20));
    assert!(status.success(), "log:\n{}", second.run.borrow().log());
}

#[tokio::test]
async fn two_processes_serve_each_others_threads() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);

    let a = Replica::start(&scratch, "a.log", &url, &agents, &[]);
    let b = Replica::start(&scratch, "b.log", &url, &agents, &[]);
    a.wait_ready().await;
    b.wait_ready().await;

    // Created through A ...
    let id = a
        .chat()
        .create_thread("fake", "gate two processes", None)
        .await;
    // ... read and streamed through B, which is a different process on the same database.
    let chat_b = b.chat();
    let thread = chat_b.thread(&id).await;
    assert_eq!(thread["id"], id.as_str());
    let (status, listed) = chat_b.get("/api/threads").await;
    assert_eq!(status, 200);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == id.as_str()),
        "B lists A's thread: {listed}"
    );
    let mut sse = chat_b.stream(&id, None).await;
    assert_eq!(sse.status, 200);
    chat_b.wait_state(&id, "working").await;
    agent.release_gate();
    let frames = sse
        .collect_until(Duration::from_secs(20), |kind, data| {
            kind == "thread_state" && data["data"]["state"] == "done"
        })
        .await;
    let seqs: Vec<i64> = frames.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    // Whichever process dispatched it, the agent saw the message once, and A reads the same log.
    assert_eq!(agent.executions().len(), 1);
    assert_eq!(a.chat().events(&id).await, chat_b.events(&id).await);

    for replica in [&a, &b] {
        assert_eq!(
            http_status(&replica.client, &format!("{}/readyz", replica.base)).await,
            Some(200)
        );
        let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
        assert!(status.success(), "log:\n{}", replica.run.borrow().log());
    }
}

#[tokio::test]
async fn sigterm_with_a_running_task_exits_within_the_grace() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);

    // The default lease (30 s) is left alone on purpose: only a graceful hand-over, not lease
    // expiry, can make the second process pick the task up within this test.
    let first = Replica::start(
        &scratch,
        "a.log",
        &url,
        &agents,
        &[("SHUTDOWN_GRACE_SECS", "3")],
    );
    first.wait_ready().await;
    let chat = first.chat();
    let id = chat.create_thread("fake", "slow sigterm", None).await;
    chat.wait_state(&id, "working").await;
    eventually("the agent executes the task", || async {
        (agent.executions().len() == 1).then_some(())
    })
    .await;

    let started = Instant::now();
    let status = first.run.borrow_mut().terminate(Duration::from_secs(10));
    let took = started.elapsed();
    let log = first.run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(
        took < Duration::from_secs(3) + Duration::from_secs(2),
        "SHUTDOWN_GRACE_SECS=3 must bound the shutdown, it took {took:?}; log:\n{log}"
    );

    // The task was neither failed nor lost: a second process takes it over at once and
    // re-attaches, without sending the message again.
    let second = Replica::start(&scratch, "b.log", &url, &agents, &[]);
    second.wait_ready().await;
    let chat = second.chat();
    assert_eq!(chat.state(&id).await, "working");
    eventually("the second process resubscribes to the task", || async {
        (agent.rpc_count("subscribe_to_task") >= 1).then_some(())
    })
    .await;
    assert_eq!(agent.rpc_count("send_streaming_message"), 1);
    // And it is in control of it: a cancel through the second process reaches the agent.
    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;
    assert_eq!(agent.cancels().len(), 1);
    let status = second.run.borrow_mut().terminate(Duration::from_secs(20));
    assert!(status.success(), "log:\n{}", second.run.borrow().log());
}
