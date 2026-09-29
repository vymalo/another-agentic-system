//! The built binary as a process: configuration errors are fatal and readable; against a real
//! database it serves the resource API and the AG-UI surface (the default), runs a thread to
//! completion through an A2A agent, and exits cleanly on SIGTERM. The legacy chat API is off by
//! default: the tests that drive it say `ORCH_SURFACES=agui,chat-api` themselves.
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
        ("--role", "ORCH_ROLE"),
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
    for role in ["all", "control-plane", "worker"] {
        assert!(help.contains(role), "--help does not name the role {role}");
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
        ("ORCH_ROLE", "controlplane"),
        ("ORCH_SURFACES", "a2a"),
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
        &["--surfaces", "a2a"],
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
        log.contains("unknown surface") && log.contains("a2a"),
        "{log}"
    );
}

#[test]
fn a_role_flag_wins_over_its_variable_and_a_bad_role_is_a_config_error() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    // The variable is valid; the flag is not, and the flag decides.
    let mut run = spawn_with_args(
        &scratch,
        "role.log",
        &["--role", "nowhere"],
        &[
            ("ORCH_ROLE", "worker"),
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    let status = run.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(78), "EX_CONFIG: {}", run.log());
    let log = run.log();
    assert!(
        log.contains("ORCH_ROLE is invalid")
            && log.contains("nowhere")
            && log.contains("control-plane"),
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
    // A surface that is not listed is not there: the AG-UI route is a 404, not a 400.
    let agui = client
        .post(format!("{base}/agui/agents/plain"))
        .header("X-Auth-Request-Email", "alice@example.com")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(agui.status().as_u16(), 404);
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("\"surfaces\":\"chat-api\""), "{log}");
}

/// Starts the binary on a free port with `args` and the agents file of these tests, and waits for
/// `/healthz`. Returns the process, its base URL and a client.
async fn serve_with(
    db: &pgdb::TestDb,
    scratch: &Scratch,
    log_name: &str,
    args: &[&str],
) -> (std::cell::RefCell<Running>, String, reqwest::Client) {
    let agents = write_agents(scratch, &agents_yaml("https://a.example.com/card"));
    let addr = format!("127.0.0.1:{}", free_port());
    let database_url = database_url_of(db);
    let mut all_args = vec!["--listen-addr", addr.as_str()];
    all_args.extend_from_slice(args);
    let run = std::cell::RefCell::new(spawn_with_args(
        scratch,
        log_name,
        &all_args,
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
    (run, base, client)
}

/// The routes of the deprecated chat API interaction surface, as method and path.
const LEGACY_ROUTES: [(&str, &str); 4] = [
    ("POST", "/api/threads"),
    (
        "GET",
        "/api/threads/00000000-0000-7000-8000-000000000001/events",
    ),
    (
        "POST",
        "/api/threads/00000000-0000-7000-8000-000000000001/messages",
    ),
    (
        "GET",
        "/api/threads/00000000-0000-7000-8000-000000000001/stream",
    ),
];

async fn status_of(
    client: &reqwest::Client,
    base: &str,
    method: &str,
    path: &str,
    user: Option<&str>,
) -> u16 {
    let mut req = client.request(method.parse().unwrap(), format!("{base}{path}"));
    if let Some(user) = user {
        req = req.header("X-Auth-Request-Email", user);
    }
    if method == "POST" {
        req = req.json(&serde_json::json!({}));
    }
    req.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn by_default_only_the_agui_surface_and_the_resource_api_are_mounted() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    // No `--surfaces`, no `ORCH_SURFACES`: what an operator gets who sets nothing.
    let (run, base, client) = serve_with(&db, &scratch, "default-surfaces.log", &[]).await;
    let alice = Some("alice@example.com");
    // The AG-UI route answers (an empty body is a 400, not a 404/405), behind the identity layer.
    assert_eq!(
        status_of(&client, &base, "POST", "/agui/agents/plain", alice).await,
        400
    );
    assert_eq!(
        status_of(&client, &base, "POST", "/agui/agents/plain", None).await,
        401
    );
    // The resource API is always there: the agent list, the thread list, a thread and cancel.
    for path in ["/api/agents", "/api/threads"] {
        assert_eq!(
            status_of(&client, &base, "GET", path, alice).await,
            200,
            "{path}"
        );
    }
    let missing = "/api/threads/00000000-0000-7000-8000-000000000001";
    assert_eq!(
        status_of(&client, &base, "GET", missing, alice).await,
        404,
        "a thread that does not exist: the route is there, the thread is not"
    );
    assert_eq!(
        status_of(&client, &base, "POST", &format!("{missing}/cancel"), alice).await,
        404,
        "cancel is routed too: it answers about the thread, not the route"
    );
    // The legacy interaction routes are not mounted. `POST /api/threads` shares its path with
    // the thread list, so it is a 405; the others match no route at all.
    for (method, path) in LEGACY_ROUTES {
        let want = if (method, path) == ("POST", "/api/threads") {
            405
        } else {
            404
        };
        assert_eq!(
            status_of(&client, &base, method, path, alice).await,
            want,
            "{method} {path}"
        );
    }
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("\"surfaces\":\"agui\""), "{log}");
}

#[tokio::test]
async fn the_legacy_routes_come_back_with_agui_and_chat_api_listed() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (run, base, client) = serve_with(
        &db,
        &scratch,
        "both-surfaces.log",
        &["--surfaces", "agui,chat-api"],
    )
    .await;
    let alice = Some("alice@example.com");
    // Both interaction routes answer (an empty body is a 400, not a 404/405) ...
    for path in ["/api/threads", "/agui/agents/plain"] {
        assert_eq!(
            status_of(&client, &base, "POST", path, alice).await,
            400,
            "{path}"
        );
    }
    // ... and every legacy route answers as itself: with `Deprecation`, which an unknown route
    // (the 404 of the default) never carries.
    for (method, path) in LEGACY_ROUTES {
        let mut req = client
            .request(method.parse().unwrap(), format!("{base}{path}"))
            .header("X-Auth-Request-Email", "alice@example.com");
        if method == "POST" {
            req = req.json(&serde_json::json!({}));
        }
        let resp = req.send().await.unwrap();
        assert!(
            resp.headers().contains_key("deprecation"),
            "{method} {path} ({}) is served by the chat API surface",
            resp.status()
        );
    }
    assert_eq!(
        status_of(&client, &base, "GET", "/api/agents", alice).await,
        200
    );
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("\"surfaces\":\"agui,chat-api\""), "{log}");
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
    // The default surface is AG-UI: the run is one POST whose response ends with the run.
    let id = uuid::Uuid::now_v7().to_string();
    let input = Chat::agui_input(
        &id,
        "run-1",
        &[("msg-1", "echo smoke")],
        serde_json::json!({}),
    );
    let mut sse = chat.agui_run("fake", &input).await;
    let frames = sse.collect_frames(Duration::from_secs(20)).await;
    let types: Vec<&str> = frames
        .iter()
        .map(|f| f.event["type"].as_str().unwrap())
        .collect();
    assert_eq!(types.first(), Some(&"RUN_STARTED"), "{types:?}");
    assert_eq!(types.last(), Some(&"RUN_FINISHED"), "{types:?}");
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        serde_json::json!({"type": "success"})
    );
    assert!(
        frames
            .iter()
            .any(|f| f.event["activityType"] == "vymalo.artifact"),
        "the agent's artifact reaches the requester: {types:?}"
    );
    // The thread the consumer named is an ordinary thread of the resource API.
    chat.wait_state(&id, "done").await;
    // ... and the legacy interaction routes are not there to read its log.
    let (legacy, _) = chat.get(&format!("/api/threads/{id}/events")).await;
    assert_eq!(legacy, 404);
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
        Self::start_with(scratch, log_name, database_url, agents, &[], extra)
    }

    /// [`Replica::start`] with command-line `args` as well (`--role worker`).
    fn start_with(
        scratch: &Scratch,
        log_name: &str,
        database_url: &str,
        agents: &Path,
        args: &[&str],
        extra: &[(&str, &str)],
    ) -> Self {
        let addr = format!("127.0.0.1:{}", free_port());
        let mut env = vec![
            ("DATABASE_URL", database_url),
            ("LISTEN_ADDR", addr.as_str()),
            ("AGENTS_FILE", path_str(agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
            // The multi-process tests below drive threads through the legacy client of
            // `orch-testsupport` (create, events, stream), and the chat API is off by default:
            // they ask for it, so they do not depend on the default.
            ("ORCH_SURFACES", "agui,chat-api"),
        ];
        env.extend_from_slice(extra);
        Replica {
            run: std::cell::RefCell::new(spawn_with_args(scratch, log_name, args, &env)),
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

/// `GET url` with the identity header, as a status code.
async fn status_as_alice(client: &reqwest::Client, url: &str) -> u16 {
    client
        .get(url)
        .header("X-Auth-Request-Email", "alice@example.com")
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// `GET /metrics` (no identity) as `(status, content type, body)`.
async fn metrics(client: &reqwest::Client, base: &str) -> (u16, String, String) {
    let response = client.get(format!("{base}/metrics")).send().await.unwrap();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (status, content_type, response.text().await.unwrap())
}

/// Every JSON line of `log` (the lines that start with `{`), parsed; a line that starts with
/// `{` and is not valid JSON fails the test.
fn json_lines(log: &str) -> Vec<serde_json::Value> {
    log.lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}")))
        .collect()
}

#[tokio::test]
async fn a_worker_serves_probes_and_no_api() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (_agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);

    // The flag says worker, the variable says nonsense: the flag wins, so this starts at all.
    let worker = Replica::start_with(
        &scratch,
        "worker.log",
        &url,
        &agents,
        &["--role", "worker"],
        &[("ORCH_ROLE", "nonsense"), ("ORCH_INSTANCE_ID", "w1")],
    );
    worker.wait_ready().await;
    let base = &worker.base;
    // The queue metrics are served by every role, without an identity.
    let (status, content_type, body) = metrics(&worker.client, base).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type, "text/plain; version=0.0.4; charset=utf-8");
    assert!(body.contains("# TYPE orch_outbox_rows gauge"), "{body}");
    assert!(body.contains("orch_outbox_rows{state=\"due\"} 0"), "{body}");
    for path in ["/healthz", "/readyz"] {
        assert_eq!(
            http_status(&worker.client, &format!("{base}{path}")).await,
            Some(200),
            "{path}"
        );
    }
    // No resource API and no surface: not even a 401, because no route matches (fail closed
    // twice: nothing is served, and what is not served needs no identity to say so).
    for path in ["/api/agents", "/api/threads", "/api/threads/x/events"] {
        assert_eq!(
            http_status(&worker.client, &format!("{base}{path}")).await,
            Some(404),
            "{path} without identity"
        );
        assert_eq!(
            status_as_alice(&worker.client, &format!("{base}{path}")).await,
            404,
            "{path} with identity"
        );
    }
    let post = worker
        .client
        .post(format!("{base}/api/threads"))
        .header("X-Auth-Request-Email", "alice@example.com")
        .json(&serde_json::json!({ "agentId": "fake", "text": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status().as_u16(), 404, "a worker creates no thread");

    let log = worker.run.borrow().log();
    assert!(
        log.contains("\"surfaces\":\"\""),
        "no surface is mounted: {log}"
    );
    let status = worker.run.borrow_mut().terminate(Duration::from_secs(20));
    let log = worker.run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("shutting down"), "{log}");
    // Every line, not only the first, says which process wrote it.
    let lines = json_lines(&log);
    assert!(lines.len() >= 3, "expected several log lines: {log}");
    for line in &lines {
        assert_eq!(line["role"], "worker", "{line}");
        assert_eq!(line["instance"], "w1", "{line}");
    }
    assert!(
        lines
            .iter()
            .any(|l| l["fields"]["message"] == "shutting down"),
        "the shutdown line has them too: {log}"
    );
}

#[tokio::test]
async fn a_control_plane_alone_does_not_dispatch_until_a_worker_starts() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);

    let cp = Replica::start_with(
        &scratch,
        "cp.log",
        &url,
        &agents,
        &["--role", "control-plane"],
        &[("ORCH_INSTANCE_ID", "cp")],
    );
    cp.wait_ready().await;
    let chat = cp.chat();
    // The control plane serves the whole API ...
    let (status, agents_body) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    assert_eq!(agents_body[0]["id"], "fake");
    let id = chat.create_thread("fake", "gate role split", None).await;

    // ... but nothing delivers the delegation: with a dispatcher in the process this thread
    // is `working` within milliseconds (the other smoke tests), so a few seconds of `queued`
    // and an agent that never heard of it is the absence of one.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(chat.state(&id).await, "queued");
    assert!(
        agent.calls().is_empty(),
        "no worker, so no call to the agent"
    );
    assert_eq!(agent.rpc_count("send_streaming_message"), 0);
    // The backlog is what an autoscaler reads from the control plane: one row is waiting for
    // a worker that does not exist yet.
    let (status, _, body) = metrics(&cp.client, &cp.base).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("orch_outbox_rows{state=\"due\"} 1"), "{body}");
    assert!(
        body.contains("orch_outbox_rows{state=\"leased\"} 0"),
        "{body}"
    );

    // A worker process appears; the control plane and the worker share only the database.
    let worker = Replica::start_with(
        &scratch,
        "worker.log",
        &url,
        &agents,
        &[],
        &[("ORCH_ROLE", "worker"), ("ORCH_INSTANCE_ID", "w1")],
    );
    worker.wait_ready().await;
    chat.wait_state(&id, "working").await;
    // The stream a user holds open on the control plane carries what the worker committed.
    let mut sse = chat.stream(&id, None).await;
    assert_eq!(sse.status, 200);
    eventually("the agent executes the task", || async {
        (agent.executions().len() == 1).then_some(())
    })
    .await;
    agent.release_gate();
    let frames = sse
        .collect_until(Duration::from_secs(20), |kind, data| {
            kind == "thread_state" && data["data"]["state"] == "done"
        })
        .await;
    let seqs: Vec<i64> = frames.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
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
    assert_eq!(agent.rpc_count("send_streaming_message"), 1);
    // The row is closed with the thread: nothing is due or leased any more.
    eventually("the backlog is empty", || async {
        let (_, _, body) = metrics(&cp.client, &cp.base).await;
        (body.contains("orch_outbox_rows{state=\"due\"} 0")
            && body.contains("orch_outbox_rows{state=\"leased\"} 0"))
        .then_some(())
    })
    .await;

    // Each role exits cleanly on SIGTERM. The worker drains its dispatcher; the control plane
    // has none.
    for (name, replica) in [("worker", &worker), ("control plane", &cp)] {
        let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
        let log = replica.run.borrow().log();
        assert!(
            status.success(),
            "{name}: unclean exit {status:?}; log:\n{log}"
        );
        assert!(log.contains("shutting down"), "{name}: {log}");
    }
}

#[tokio::test]
async fn one_control_plane_and_two_workers_survive_a_killed_worker() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);
    let lease = ("OUTBOX_LEASE_SECS", "3");

    let cp = Replica::start_with(
        &scratch,
        "cp.log",
        &url,
        &agents,
        &["--role", "control-plane"],
        &[("ORCH_INSTANCE_ID", "cp"), lease],
    );
    let w1 = Replica::start_with(
        &scratch,
        "w1.log",
        &url,
        &agents,
        &["--role", "worker"],
        &[("ORCH_INSTANCE_ID", "w1"), lease],
    );
    let w2 = Replica::start_with(
        &scratch,
        "w2.log",
        &url,
        &agents,
        &[],
        &[("ORCH_ROLE", "worker"), ("ORCH_INSTANCE_ID", "w2"), lease],
    );
    for replica in [&cp, &w1, &w2] {
        replica.wait_ready().await;
    }
    let chat = cp.chat();
    let id = chat.create_thread("fake", "gate two workers", None).await;
    chat.wait_state(&id, "working").await;
    eventually("the agent executes the task", || async {
        (agent.executions().len() == 1).then_some(())
    })
    .await;

    // Which worker holds the delegation? Its lease names it.
    let pool = db.pool("smoke-lease", 1).await;
    let owner: String = eventually("a worker leases the delegation", || async {
        sqlx::query_scalar(
            "SELECT lease_owner FROM outbox WHERE kind = 'delegate' AND status = 'inflight'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .flatten()
    })
    .await;
    let (victim, survivor) = match owner.as_str() {
        "w1" => (&w1, &w2),
        "w2" => (&w2, &w1),
        other => panic!("the delegation is leased by {other:?}, not a worker"),
    };

    // The worker dies without a word: no shutdown, no lease release. The control plane and the
    // other worker are untouched.
    victim.run.borrow_mut().kill_hard();
    assert!(cp.run.borrow_mut().exited().is_none());

    // Once the lease has expired the surviving worker re-attaches to the running task; only
    // then does the agent finish, so the rest of the run is really the survivor's.
    eventually("the surviving worker re-attaches to the task", || async {
        assert!(
            survivor.run.borrow_mut().exited().is_none(),
            "the surviving worker died; log:\n{}",
            survivor.run.borrow().log()
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
    for replica in [survivor, &cp] {
        let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
        assert!(status.success(), "log:\n{}", replica.run.borrow().log());
    }
}

#[tokio::test]
async fn a_worker_stopped_with_a_running_task_hands_it_over_at_once() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);

    let cp = Replica::start_with(
        &scratch,
        "cp.log",
        &url,
        &agents,
        &["--role", "control-plane"],
        &[],
    );
    // The default lease (30 s) is left alone on purpose: only a graceful hand-over, not lease
    // expiry, can make the second worker pick the task up within this test.
    let first = Replica::start_with(
        &scratch,
        "w1.log",
        &url,
        &agents,
        &["--role", "worker"],
        &[("SHUTDOWN_GRACE_SECS", "3")],
    );
    cp.wait_ready().await;
    first.wait_ready().await;
    let chat = cp.chat();
    let id = chat
        .create_thread("fake", "slow worker sigterm", None)
        .await;
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

    // The task was neither failed nor lost: a second worker takes it over at once and
    // re-attaches, without sending the message again; a cancel through the control plane
    // reaches the agent through it.
    let second = Replica::start_with(
        &scratch,
        "w2.log",
        &url,
        &agents,
        &["--role", "worker"],
        &[],
    );
    second.wait_ready().await;
    assert_eq!(chat.state(&id).await, "working");
    eventually("the second worker resubscribes to the task", || async {
        (agent.rpc_count("subscribe_to_task") >= 1).then_some(())
    })
    .await;
    assert_eq!(agent.rpc_count("send_streaming_message"), 1);
    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;
    assert_eq!(agent.cancels().len(), 1);
    for replica in [&second, &cp] {
        let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
        assert!(status.success(), "log:\n{}", replica.run.borrow().log());
    }
}
