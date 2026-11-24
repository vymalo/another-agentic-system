//! The broker's run routes on the orchestrator's internal listener (ADR 0084 decisions 1, 2, 5
//! and 6): the binary binds a second port, serves `POST /internal/runs`,
//! `DELETE /internal/runs/{run}` and `GET /internal/runs/{run}/lease` there and **nothing else**,
//! and drains that port on SIGTERM. The database tests need `ORCH_TEST_DATABASE_URL` (they skip
//! without); the configuration tests always run.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::cell::RefCell;
use std::fs;
use std::io::Read as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../crates/store-postgres/tests/support/mod.rs"]
mod pgdb;

/// The addresses the binary binds, read from its log (see the module).
mod support;
use support::{bound, run_routes_bound};

const BIN: &str = env!("CARGO_BIN_EXE_orchestrator");
/// The orchestrator's own token: the bind and the unbind.
const ORCH_TOKEN: &str = "orch-run-token-0123456789abcdef0123456789abcdef";
/// The token of the agent the test binds its runs to.
const AGENT_TOKEN: &str = "agent-run-token-0123456789abcdef0123456789abcdef";
/// Another agent's token: the right run id is not enough.
const OTHER_TOKEN: &str = "other-run-token-0123456789abcdef0123456789abcdef";
/// The deployment's key for the endpoint `default`, and what a lease of it answers.
const MODEL_KEY: &str = "model-key-0123456789abcdef0123456789abcdef";
const AGENT: &str = "fake";
const PERSON: &str = "alice@example.com";

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("orch-runs-{}", uuid::Uuid::now_v7().simple()));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.file(name);
        fs::write(&path, text).unwrap();
        path
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

impl support::Process for Running {
    fn log(&self) -> String {
        Running::log(self)
    }

    fn exited(&mut self) -> Option<ExitStatus> {
        Running::exited(self)
    }
}

/// A configuration file with both listeners on port 0 (the operating system picks) and the run
/// routes' tokens in variables: nothing secret is in the file. The endpoint `default` has a key;
/// `no-key-model` is not an endpoint at all, so no credential exists for it.
fn config(scratch: &Scratch) -> PathBuf {
    let agents = scratch.write(
        "agents.yaml",
        &format!(
            "- id: {AGENT}\n  name: Fake agent\n  cardUrl: http://127.0.0.1:9/card\n  tokenEnv: SMOKE_AGENT_TOKEN\n"
        ),
    );
    scratch.write(
        "config.yaml",
        &format!(
            "\
version: 1
server:
  listen: 127.0.0.1:0
  internal:
    listen: 127.0.0.1:0
    token: {{ env: ORCH_RUN_TOKEN }}
    agents:
      {AGENT}: {{ env: AGENT_RUN_TOKEN }}
      other: {{ env: OTHER_RUN_TOKEN }}
database: {{ url: {{ env: DATABASE_URL }} }}
agents: {{ file: {} }}
models:
  endpoints:
    default: {{ baseUrl: 'https://model.example.com/v1', apiKey: {{ env: MODEL_KEY }} }}
",
            agents.display()
        ),
    )
}

/// Starts the binary with exactly `env` (no inherited variables: no proxy, no stray configuration).
fn spawn(scratch: &Scratch, config: &Path, env: &[(&str, &str)]) -> Running {
    let log = scratch.file("out.log");
    let out = fs::File::create(&log).unwrap();
    let child = Command::new(BIN)
        .args(["--config", config.to_str().unwrap()])
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

/// A started binary: its main address (where `/healthz` answers once the database is migrated),
/// the run routes' address, and the client. It holds the scratch directory, so the log outlives
/// the assertions that read it.
struct Started {
    run: RefCell<Running>,
    main: String,
    internal: String,
    client: reqwest::Client,
    _scratch: Scratch,
}

/// Starts the binary against `db`, and waits for both listeners and `/healthz`.
async fn start(db: &pgdb::TestDb, scratch: Scratch) -> Started {
    let config = config(&scratch);
    let database_url = db.pinned_url();
    let direct = pgdb::direct_counterpart(&database_url);
    let mut env: Vec<(&str, &str)> = vec![
        ("DATABASE_URL", database_url.as_str()),
        ("ORCH_RUN_TOKEN", ORCH_TOKEN),
        ("AGENT_RUN_TOKEN", AGENT_TOKEN),
        ("OTHER_RUN_TOKEN", OTHER_TOKEN),
        ("MODEL_KEY", MODEL_KEY),
        ("SMOKE_AGENT_TOKEN", OTHER_TOKEN),
        ("NO_PROXY", "127.0.0.1,localhost"),
    ];
    if let Some(direct) = &direct {
        env.push(("DATABASE_DIRECT_URL", direct.as_str()));
    }
    let run = RefCell::new(spawn(&scratch, &config, &env));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let addr = bound(&run).await;
    let main = format!("http://{addr}");
    let internal = run_routes_bound(&run).await;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(response) = client.get(format!("{main}/healthz")).send().await
            && response.status().as_u16() == 200
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the binary did not answer /healthz; log:\n{}",
            run.borrow().log()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Started {
        run,
        main,
        internal,
        client,
        _scratch: scratch,
    }
}

/// The deadline a binding may have: an hour from now, RFC 3339.
fn deadline() -> String {
    (jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1)).to_string()
}

fn bind_body(run: &str, model: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "run": run,
        "agent": AGENT,
        "person": PERSON,
        "expiresAt": deadline(),
    });
    if let Some(model) = model {
        body["model"] = serde_json::Value::String(model.to_owned());
    }
    body
}

/// A body of a refusal: the ruled slug and nothing else.
fn code(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// One call to a route of the internal listener; `token` is presented as a bearer.
async fn call(
    started: &Started,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, String) {
    let mut request = started
        .client
        .request(method, format!("http://{}{path}", started.internal));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap())
}

#[tokio::test]
async fn a_bind_then_a_lease_over_a_real_socket() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let started = start(&db, Scratch::new()).await;

    let (status, body) = call(
        &started,
        reqwest::Method::POST,
        "/internal/runs",
        Some(ORCH_TOKEN),
        Some(bind_body("run-a", Some("default"))),
    )
    .await;
    assert_eq!(status, 204, "{body}");

    // The same binding again is idempotent, and a different one for the same run is a conflict.
    let (status, _) = call(
        &started,
        reqwest::Method::POST,
        "/internal/runs",
        Some(ORCH_TOKEN),
        Some(bind_body("run-a", Some("default"))),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(
        &started,
        reqwest::Method::POST,
        "/internal/runs",
        Some(ORCH_TOKEN),
        Some(bind_body("run-a", Some("no-key-model"))),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "run_already_bound");

    // The bound agent leases the deployment's key, and the answer is `no-store`.
    let response = started
        .client
        .get(format!(
            "http://{}/internal/runs/run-a/lease",
            started.internal
        ))
        .bearer_auth(AGENT_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let lease: serde_json::Value = response.json().await.unwrap();
    assert_eq!(lease["token"], MODEL_KEY);
    assert!(lease["expiresAt"].as_str().is_some(), "{lease}");
    // The key is in the answer and nowhere else.
    assert!(!started.run.borrow().log().contains(MODEL_KEY));

    // Removed, the run is not bound any more.
    let (status, _) = call(
        &started,
        reqwest::Method::DELETE,
        "/internal/runs/run-a",
        Some(ORCH_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(
        &started,
        reqwest::Method::GET,
        "/internal/runs/run-a/lease",
        Some(AGENT_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(code(&body), "run_not_bound");
}

#[tokio::test]
async fn the_listener_serves_the_run_routes_and_nothing_else() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let started = start(&db, Scratch::new()).await;
    // No surface, no health, no edge route on the second port.
    for path in [
        "/healthz",
        "/readyz",
        "/api/agents",
        "/agui/agents/fake",
        "/mcp",
    ] {
        let (status, _) = call(&started, reqwest::Method::GET, path, None, None).await;
        assert_eq!(
            status, 404,
            "{path} is not a route of the internal listener"
        );
    }
    // The main listener still serves them: it is the same process, two ports.
    let health = started
        .client
        .get(format!("{}/healthz", started.main))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status().as_u16(), 200);
}

#[tokio::test]
async fn a_wrong_caller_is_refused() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let started = start(&db, Scratch::new()).await;
    let (status, _) = call(
        &started,
        reqwest::Method::POST,
        "/internal/runs",
        Some(ORCH_TOKEN),
        Some(bind_body("run-b", Some("default"))),
    )
    .await;
    assert_eq!(status, 204);

    // An agent may not bind or unbind.
    for (method, path) in [
        (reqwest::Method::POST, "/internal/runs"),
        (reqwest::Method::DELETE, "/internal/runs/run-b"),
    ] {
        let (status, body) = call(
            &started,
            method,
            path,
            Some(AGENT_TOKEN),
            Some(bind_body("run-c", Some("default"))),
        )
        .await;
        assert_eq!(status, 403, "{path}: {body}");
        assert_eq!(code(&body), "not_allowed");
    }

    // The right run id with another agent's token is `run_not_bound`: the answer never says
    // whether a run exists.
    let (status, body) = call(
        &started,
        reqwest::Method::GET,
        "/internal/runs/run-b/lease",
        Some(OTHER_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(code(&body), "run_not_bound");

    // A request with no token is refused at the door, and says how to present one.
    let response = started
        .client
        .get(format!(
            "http://{}/internal/runs/run-b/lease",
            started.internal
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer")
    );
}

#[tokio::test]
async fn a_model_with_no_credential_is_unavailable() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let started = start(&db, Scratch::new()).await;
    // A model the deployment has no key for: `unavailable` (503), never an empty header.
    for (run, model) in [("run-d", Some("no-key-model")), ("run-e", None)] {
        let (status, body) = call(
            &started,
            reqwest::Method::POST,
            "/internal/runs",
            Some(ORCH_TOKEN),
            Some(bind_body(run, model)),
        )
        .await;
        assert_eq!(status, 204, "{run}: {body}");
        let (status, body) = call(
            &started,
            reqwest::Method::GET,
            &format!("/internal/runs/{run}/lease"),
            Some(AGENT_TOKEN),
            None,
        )
        .await;
        assert_eq!(status, 503, "{run}: {body}");
        assert_eq!(code(&body), "unavailable");
        assert!(!body.contains("token"), "no lease in a refusal: {body}");
    }
}

#[tokio::test]
async fn a_graceful_shutdown_drains_the_listener() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let started = start(&db, scratch).await;
    let (status, _) = call(
        &started,
        reqwest::Method::POST,
        "/internal/runs",
        Some(ORCH_TOKEN),
        Some(bind_body("run-f", Some("default"))),
    )
    .await;
    assert_eq!(status, 204);

    // An idle connection to the internal port is open when the signal arrives.
    let addr = started
        .internal
        .parse::<std::net::SocketAddr>()
        .expect("the log names a socket address");
    let mut idle = TcpStream::connect(addr).expect("the internal listener accepts a connection");
    idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    let status = started.run.borrow_mut().terminate(Duration::from_secs(30));
    assert!(status.success(), "a drained process exits 0: {status:?}");
    // The listener is gone: the port refuses connections, and the connection it held is closed
    // rather than left dangling.
    let mut buf = [0u8; 1];
    match idle.read(&mut buf) {
        Ok(0) | Err(_) => {}
        Ok(read) => panic!("the draining listener sent {read} byte(s) to an idle connection"),
    }
    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err(),
        "the port is closed after the drain"
    );
    // Everything it served is still whole: the bind was committed, and the log holds no token.
    let log = started.run.borrow().log();
    assert!(log.contains("the run routes listen"), "{log}");
    for token in [ORCH_TOKEN, AGENT_TOKEN, OTHER_TOKEN, MODEL_KEY] {
        assert!(!log.contains(token), "a token is in the log:\n{log}");
    }
}

#[test]
fn each_listener_is_read_from_its_own_log_line() {
    // The line the binary writes for the run routes, in the JSON format the test runs it with
    // (the default: the text format is colorized unless `NO_COLOR` is set, so a text line of a
    // real process cannot be read by a `word.strip_prefix("addr=")` — this test asks for JSON and
    // reads `fields.addr`, as every other process test does).
    let run_routes = r#"{"role":"all","instance":"i-1","level":"INFO","fields":{"message":"the run routes listen","addr":"127.0.0.1:39437","agents":2},"target":"orchestrator::boot"}"#;
    let main = r#"{"role":"all","instance":"i-1","level":"INFO","fields":{"message":"orchestrator listening","addr":"127.0.0.1:38455","agents":1,"surfaces":"agui"},"target":"orchestrator::boot"}"#;
    assert_eq!(
        support::run_routes_listening_addr(run_routes).as_deref(),
        Some("127.0.0.1:39437")
    );
    // and neither helper answers for the other's line
    assert_eq!(support::run_routes_listening_addr(main), None);
    assert_eq!(support::listening_addr(run_routes), None);
    assert_eq!(
        support::listening_addr(main).as_deref(),
        Some("127.0.0.1:38455")
    );
}
