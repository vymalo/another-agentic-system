//! The built binary as a process: configuration errors are fatal and readable; against a real
//! database it serves the resource API and the AG-UI surface (the default), runs a thread to
//! completion through an A2A agent, and exits cleanly on SIGTERM. The legacy chat API surface was
//! removed (ADR 0012): naming it in `ORCH_SURFACES` is a configuration error, and its routes are
//! not there.
//!
//! The database tests need `ORCH_TEST_DATABASE_URL` (they skip without); the configuration
//! tests always run.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use orch_testsupport::{Chat, FakeAgent, FakeAgentOptions, Frame, VerifierScript, eventually};

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

/// A build without the Cargo feature `agent-local` has no in-process agents: an AGENTS_FILE that
/// asks for one is a configuration error naming the feature, and it is found before anything
/// connects (ADR 0015). With the feature the same file is served; see `tests/local.rs`.
#[cfg(not(feature = "agent-local"))]
#[test]
fn transport_local_exits_78_naming_agent_local() {
    let scratch = Scratch::new();
    let agents = write_agents(
        &scratch,
        "- id: helper\n  name: Helper\n  transport: local\n  agent: echo\n",
    );
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
    assert!(
        log.contains("agent-local") && log.contains("--features agent-local"),
        "{log}"
    );
    assert!(log.contains("helper"), "{log}");
}

#[test]
fn help_lists_every_flag_and_variable_and_exits_zero() {
    let scratch = Scratch::new();
    let mut run = spawn_with_args(&scratch, "help.log", &["--help"], &[]);
    let status = run.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(0), "{}", run.log());
    let help = run.log();
    for (flag, var) in [
        ("--config", "ORCH_CONFIG_FILE"),
        ("--database-url", "DATABASE_URL"),
        ("--agents-file", "AGENTS_FILE"),
        ("--registry-url", "AGENT_REGISTRY_URL"),
        ("--registry-token", "AGENT_REGISTRY_TOKEN"),
        ("--registry-agent-token", "AGENT_REGISTRY_AGENT_TOKEN"),
        ("--registry-timeout-secs", "AGENT_REGISTRY_TIMEOUT_SECS"),
        ("--registry-max-age-secs", "AGENT_REGISTRY_MAX_AGE_SECS"),
        ("--listen-addr", "LISTEN_ADDR"),
        ("--role", "ORCH_ROLE"),
        ("--surfaces", "ORCH_SURFACES"),
        ("--mcp-tokens-file", "MCP_TOKENS_FILE"),
        ("--mcp-allowed-hosts", "MCP_ALLOWED_HOSTS"),
        ("--public-url", "ORCH_PUBLIC_URL"),
        ("--mcp-wait-max-secs", "MCP_WAIT_MAX_SECS"),
        ("--mcp-wait-max-concurrent", "MCP_WAIT_MAX_CONCURRENT"),
        ("--mcp-wait-max-per-user", "MCP_WAIT_MAX_PER_USER"),
        ("--mcp-allowed-origins", "MCP_ALLOWED_ORIGINS"),
        ("--thread-tools-secret", "THREAD_TOOLS_SECRET"),
        (
            "--thread-tools-secret-previous",
            "THREAD_TOOLS_SECRET_PREVIOUS",
        ),
        ("--thread-tools-url", "THREAD_TOOLS_URL"),
        (
            "--thread-tools-token-ttl-secs",
            "THREAD_TOOLS_TOKEN_TTL_SECS",
        ),
        ("--thread-tools-allowed-hosts", "THREAD_TOOLS_ALLOWED_HOSTS"),
        ("--gate", "ORCH_GATE"),
        ("--ci-timeout-secs", "ORCH_CI_TIMEOUT_SECS"),
        ("--webhook-generic-secrets", "WEBHOOK_GENERIC_SECRETS"),
        ("--webhook-github-secrets", "WEBHOOK_GITHUB_SECRETS"),
        (
            "--webhook-generic-max-skew-secs",
            "WEBHOOK_GENERIC_MAX_SKEW_SECS",
        ),
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
    assert!(
        help.contains("--print-config"),
        "--help lacks --print-config"
    );
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
        // The registry's settings are read whether or not the build can use them.
        ("AGENT_REGISTRY_TIMEOUT_SECS", "0"),
        ("AGENT_REGISTRY_MAX_AGE_SECS", "3601"),
        ("ORCH_ROLE", "controlplane"),
        ("ORCH_SURFACES", "a2a"),
        ("ORCH_SURFACES", ","),
        ("ORCH_CI_TIMEOUT_SECS", "0"),
        ("WEBHOOK_GENERIC_MAX_SKEW_SECS", "0"),
        ("WEBHOOK_GENERIC_SECRETS", "one,two,three"),
        ("WEBHOOK_GITHUB_SECRETS", "one,two,three"),
        ("AUTH_DEV_USER", "not-an-email"),
        ("DATABASE_MAX_CONNECTIONS", "1"),
        ("DISPATCHER_CONCURRENCY", "0"),
        ("OUTBOX_LEASE_SECS", "2"),
        ("SHUTDOWN_GRACE_SECS", "0"),
        // Read whatever the surfaces are: a typo is not quietly ignored until `mcp` is mounted.
        ("ORCH_PUBLIC_URL", "chat.example.com"),
        ("MCP_WAIT_MAX_SECS", "0"),
        ("MCP_WAIT_MAX_CONCURRENT", "0"),
        ("MCP_WAIT_MAX_PER_USER", "many"),
        ("MCP_ALLOWED_ORIGINS", "*"),
        // The thread-tools settings are read whatever the surfaces are, too.
        ("THREAD_TOOLS_TOKEN_TTL_SECS", "59"),
        ("THREAD_TOOLS_ALLOWED_HOSTS", "*"),
        ("THREAD_TOOLS_URL", "http://orchestrator:8080"),
        (
            "THREAD_TOOLS_SECRET",
            "not-paired-with-a-url-0123456789abcdef0123",
        ),
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

/// The surface `thread-tools` (the per-thread MCP endpoint) fails closed: named with no key, with a
/// key and no URL, with a key that is too short, or with a URL that is not one, the process exits 78
/// before it connects to anything, names the variable, and never prints the key. A worker is asked
/// the same: it is the one that mints.
#[test]
fn the_thread_tools_surface_fails_closed_and_never_prints_its_key() {
    const KEY: &str = "hunter2-0123456789abcdef0123456789abcdef0123456789abcdef";
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    for (n, (role, extra, var)) in [
        ("all", vec![], "THREAD_TOOLS_SECRET"),
        ("worker", vec![], "THREAD_TOOLS_SECRET"),
        (
            "all",
            vec![("THREAD_TOOLS_SECRET", KEY)],
            "THREAD_TOOLS_SECRET",
        ),
        (
            "all",
            vec![
                ("THREAD_TOOLS_SECRET", "hunter2-short"),
                ("THREAD_TOOLS_URL", "http://orchestrator:8080"),
            ],
            "THREAD_TOOLS_SECRET",
        ),
        (
            "all",
            vec![
                ("THREAD_TOOLS_SECRET", KEY),
                ("THREAD_TOOLS_URL", "orchestrator:8080"),
            ],
            "THREAD_TOOLS_URL",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut env = vec![
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("ORCH_SURFACES", "agui,thread-tools"),
            ("ORCH_ROLE", role),
        ];
        env.extend(extra);
        let mut run = spawn_logging_to(&scratch, &format!("thread-tools-{n}.log"), &env);
        let status = run.wait(Duration::from_secs(10));
        let log = run.log();
        assert_eq!(status.code(), Some(78), "case {n}: EX_CONFIG; {log}");
        assert!(log.contains(var), "case {n}: {log}");
        assert!(
            !log.contains("hunter2"),
            "case {n}: a key leaked into the log"
        );
        assert!(
            !log.contains("cannot connect to Postgres"),
            "nothing connects before the configuration is accepted: {log}"
        );
    }
}

/// A gate the build cannot run (the verifier with nobody to ask, or an agent that would verify its
/// own work) is refused before anything connects: exit 78, the message names the setting and says
/// why. It never falls back to no gate. (`ci` has been honoured since the CI webhook, slice 6, and
/// the verifier since slice 10: every source is, so what is refused is a gate that cannot be run.)
#[test]
fn a_gate_this_build_cannot_run_is_fatal_in_every_layer() {
    let scratch = Scratch::new();
    let plain = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let gated = scratch.file("gated.yaml");
    fs::write(
        &gated,
        format!(
            "{}  gate: {{require: [agent-checks, verifier], verifier: fake}}\n",
            agents_yaml("https://a.example.com/card")
        ),
    )
    .unwrap();
    // (the AGENTS_FILE, extra variables, what the message says, what it names)
    type Case<'a> = (&'a Path, &'a [(&'a str, &'a str)], &'a str, &'a str);
    let cases: [Case; 2] = [
        (
            &plain,
            &[("ORCH_GATE", "verifier")],
            "AGENTS_FILE: ",
            "no verifier agent is configured",
        ),
        (&gated, &[], "AGENTS_FILE: ", "would verify its own work"),
    ];
    for (n, (agents, extra, says, names)) in cases.into_iter().enumerate() {
        // The database is unreachable on purpose: configuration is validated first.
        let mut env = vec![
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ];
        env.extend_from_slice(extra);
        let mut run = spawn_logging_to(&scratch, &format!("gate-{n}.log"), &env);
        let status = run.wait(Duration::from_secs(10));
        assert_eq!(status.code(), Some(78), "EX_CONFIG: {}", run.log());
        let log = run.log();
        assert!(log.contains(says), "{says}: {log}");
        assert!(log.contains(names), "{names}: {log}");
        assert!(!log.contains(TOKEN), "a secret leaked into the log");
    }
}

/// A webhook surface without its secret must not start: a route anyone can call is worse than no
/// route. Exit 78 before anything connects, naming the variable and never a secret.
#[cfg(feature = "surface-webhook")]
#[test]
fn a_mounted_webhook_without_secrets_is_fatal_before_anything_connects() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    for (n, (surface, var, secrets)) in [
        ("webhook-generic", "WEBHOOK_GENERIC_SECRETS", None),
        ("webhook-generic", "WEBHOOK_GENERIC_SECRETS", Some("  ")),
        (
            "webhook-generic",
            "WEBHOOK_GENERIC_SECRETS",
            Some("a,b,hunter2-third"),
        ),
        ("webhook-github", "WEBHOOK_GITHUB_SECRETS", None),
        (
            "webhook-github",
            "WEBHOOK_GITHUB_SECRETS",
            Some("a,b,hunter2-third"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut env = vec![
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("ORCH_SURFACES", surface),
        ];
        if let Some(secrets) = secrets {
            env.push((var, secrets));
        }
        let mut run = spawn_logging_to(&scratch, &format!("webhook-{n}.log"), &env);
        let status = run.wait(Duration::from_secs(10));
        let log = run.log();
        assert_eq!(status.code(), Some(78), "{secrets:?}: EX_CONFIG; {log}");
        assert!(log.contains(var), "{surface} {secrets:?}: {log}");
        assert!(!log.contains("hunter2"), "a secret leaked into the log");
        assert!(
            !log.contains("cannot connect to Postgres"),
            "nothing connects before the configuration is accepted: {log}"
        );
    }
}

/// A gate that requires `ci` must be able to receive a report and to tell which reports count:
/// without a webhook surface, or without a named check, the binary exits 78 before anything
/// connects (the alternative was a job that waits for a report nobody can send, or that a
/// first `skipped` report of another check passes). A secret under 32 bytes is refused too.
#[cfg(feature = "surface-webhook")]
#[test]
fn a_ci_gate_that_could_never_be_decided_is_fatal_before_anything_connects() {
    const SECRET: &str = "smoke-webhook-secret-0123456789abcdef0123";
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let cases: [(&[(&str, &str)], &str); 5] = [
        (
            &[("ORCH_GATE", "ci"), ("ORCH_CI_REQUIRED", "build")],
            "no CI webhook surface is mounted (ORCH_SURFACES)",
        ),
        (
            &[
                ("ORCH_GATE", "ci"),
                ("ORCH_SURFACES", "agui,webhook-generic"),
                ("WEBHOOK_GENERIC_SECRETS", SECRET),
            ],
            "ci.required",
        ),
        (
            &[
                ("ORCH_GATE", "ci"),
                ("ORCH_CI_REQUIRED", " , "),
                ("ORCH_SURFACES", "agui,webhook-generic"),
                ("WEBHOOK_GENERIC_SECRETS", SECRET),
            ],
            "ORCH_CI_REQUIRED",
        ),
        (
            &[
                ("ORCH_SURFACES", "agui,webhook-generic"),
                ("WEBHOOK_GENERIC_SECRETS", "hunter2"),
            ],
            "the minimum is 32",
        ),
        (
            &[
                ("ORCH_SURFACES", "agui,webhook-github"),
                ("WEBHOOK_GITHUB_SECRETS", "hunter2"),
            ],
            "the minimum is 32",
        ),
    ];
    for (n, (extra, says)) in cases.into_iter().enumerate() {
        let mut env = vec![
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ];
        env.extend_from_slice(extra);
        let mut run = spawn_logging_to(&scratch, &format!("ci-gate-{n}.log"), &env);
        let status = run.wait(Duration::from_secs(10));
        let log = run.log();
        assert_eq!(status.code(), Some(78), "{extra:?}: EX_CONFIG; {log}");
        assert!(log.contains(says), "{extra:?}: {says}: {log}");
        assert!(!log.contains("hunter2"), "a secret leaked into the log");
        assert!(
            !log.contains("cannot connect to Postgres"),
            "nothing connects before the configuration is accepted: {log}"
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
            ("ORCH_SURFACES", "agui"),
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

// ---- the configuration file (ADR 0034) --------------------------------------------------------

/// What the process wrote to stdout and to stderr, apart, and how it ended.
struct Printed {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Runs the binary to its end with exactly `env`, for the commands that print and exit.
fn run_to_end(args: &[&str], env: &[(&str, &str)]) -> Printed {
    let out = Command::new(BIN)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Printed {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

const SECRET_URL: &str = "postgres://nobody:hunter2-s3cr3t@127.0.0.1:1/none";

/// Waits until the log of `run` holds `needle` (the process keeps running: it is killed when `run`
/// is dropped). The notes about where the settings came from are logged before anything connects.
fn wait_for_log(run: &Running, needle: &str, within: Duration) -> String {
    let deadline = Instant::now() + within;
    loop {
        let log = run.log();
        if log.contains(needle) {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "the log never held {needle:?}:\n{log}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn write_config(scratch: &Scratch, yaml: &str) -> PathBuf {
    let path = scratch.file("config.yaml");
    fs::write(&path, yaml).unwrap();
    path
}

const CONFIG: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

#[test]
fn a_configuration_file_with_many_mistakes_lists_every_one_and_exits_78_without_a_value() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        "version: 1\nnonsense: 1\nserver: { role: hunter2-s3cr3t, listen: 5 }\n\
         database: hunter2-s3cr3t\ntasks: { turnSummary: {} }\ndispatcher: { concurrency: 0 }\n",
    );
    let out = run_to_end(&[], &[("ORCH_CONFIG_FILE", path_str(&config))]);
    assert_eq!(out.status.code(), Some(78), "EX_CONFIG: {}", out.stderr);
    for line in [
        "nonsense: unknown key",
        "server.role: not an allowed value",
        "server.listen: expected string",
        "database: ",
        "tasks.turnSummary: reserved for a later task of ADR 0035 that has no PR yet",
        "dispatcher.concurrency: must be at least 1",
    ] {
        assert!(
            out.stderr.contains(line),
            "stderr lacks {line:?}:\n{}",
            out.stderr
        );
    }
    assert!(!out.stderr.contains("hunter2-s3cr3t") && !out.stdout.contains("hunter2-s3cr3t"));
}

/// `toolServers` (ADR 0024): every mistake is listed, the process exits 78 before anything
/// connects, and no line carries a value of the file or of a credential.
#[test]
fn a_tool_server_with_mistakes_lists_every_one_and_exits_78_without_a_value() {
    const SECRET: &str = "tool-s3cr3t-must-not-leak";
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!(
            "{CONFIG}toolServers:\n\
             \x20 - id: Web_Search\n    name: Web\n    url: 'https://u:{SECRET}@a.example.com/mcp'\n\
             \x20   icon: 'https://a.example.com/{SECRET}.png'\n    bearer: {{ env: TOOL_TOKEN_UNSET }}\n\
             \x20   headers: {{ Accept: {{ env: SMOKE_AGENT_TOKEN }} }}\n    agents: [nosuch]\n"
        ),
    );
    let out = run_to_end(
        &[],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            // unreachable on purpose: configuration is validated first
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    assert_eq!(out.status.code(), Some(78), "EX_CONFIG: {}", out.stderr);
    for line in [
        "toolServers[0].id: an id is",
        "toolServers[0].url: expected an absolute http:// or https:// URL",
        "toolServers[0].icon: an icon is a data: URI",
        "toolServers[0].bearer: the environment variable TOOL_TOKEN_UNSET is unset or empty",
        "toolServers[0].headers.Accept: this header is the orchestrator's own to set",
    ] {
        assert!(
            out.stderr.contains(line),
            "stderr lacks {line:?}:\n{}",
            out.stderr
        );
    }
    for value in [SECRET, TOKEN, "Web_Search"] {
        assert!(
            !out.stderr.contains(value) && !out.stdout.contains(value),
            "{value} is printed:\n{}\n{}",
            out.stdout,
            out.stderr
        );
    }
    // an agent that is not in the agents file is the next mistake, once the file's own are fixed
    let config = write_config(
        &scratch,
        &format!(
            "{CONFIG}toolServers:\n  - id: websearch\n    name: Web\n    url: https://a.example.com/mcp\n    agents: [fake, nosuch]\n"
        ),
    );
    let out = run_to_end(
        &[],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("toolServers[0].agents[1]: names no agent of the agents file"),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("toolServers[0].agents[0]"),
        "{}",
        out.stderr
    );
}

/// The servers of the file are in the printed configuration with their credentials as references
/// and the credential's value nowhere.
#[test]
fn print_config_shows_the_tool_servers_with_references_and_never_a_credential() {
    const BEARER: &str = "bearer-value-must-not-leak";
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!(
            "{CONFIG}toolServers:\n  - id: websearch\n    name: Web search\n    url: https://search.example.com/mcp\n\
             \x20   bearer: {{ env: SEARCH_TOKEN }}\n    headers: {{ X-Api-Key: {{ env: SEARCH_KEY }} }}\n    agents: [fake]\n"
        ),
    );
    let out = run_to_end(
        &["--print-config"],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", SECRET_URL),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("SEARCH_TOKEN", BEARER),
            ("SEARCH_KEY", "key-value-must-not-leak"),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("toolServers:"), "{}", out.stdout);
    assert!(out.stdout.contains("env: SEARCH_TOKEN"), "{}", out.stdout);
    assert!(out.stdout.contains("env: SEARCH_KEY"), "{}", out.stdout);
    for value in [BEARER, "key-value-must-not-leak", TOKEN] {
        assert!(
            !out.stdout.contains(value) && !out.stderr.contains(value),
            "{value} is printed:\n{}\n{}",
            out.stdout,
            out.stderr
        );
    }
}

#[test]
fn a_syntax_error_and_a_missing_file_are_exit_78() {
    let scratch = Scratch::new();
    let config = write_config(&scratch, "version: 1\ndatabase: [\n");
    let out = run_to_end(&[], &[("ORCH_CONFIG_FILE", path_str(&config))]);
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr.contains("the YAML cannot be read (line "),
        "{}",
        out.stderr
    );
    let out = run_to_end(&["--config", "/nonexistent/config.yaml"], &[]);
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr.contains("cannot read the configuration file"),
        "{}",
        out.stderr
    );
}

#[test]
fn print_config_prints_the_merged_configuration_with_references_and_never_a_value() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!("{CONFIG}server: {{ listen: 0.0.0.0:8080 }}\n"),
    );
    let out = run_to_end(
        &["--print-config"],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", SECRET_URL),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("LISTEN_ADDR", "0.0.0.0:9999"),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", out.stderr);
    // The file, the variable over it, the defaults filled in; the secret as its reference.
    assert!(out.stdout.starts_with("version: 1\n"), "{}", out.stdout);
    assert!(
        out.stdout.contains("listen: 0.0.0.0:9999"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("shutdownGraceSecs: 15"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("env: DATABASE_URL"), "{}", out.stdout);
    for value in ["hunter2-s3cr3t", TOKEN, "nobody"] {
        assert!(
            !out.stdout.contains(value) && !out.stderr.contains(value),
            "{value} is printed:\n{}\n{}",
            out.stdout,
            out.stderr
        );
    }
    // The variable that won is said, by name and key, on stderr.
    assert!(
        out.stderr.contains("LISTEN_ADDR overrides server.listen"),
        "{}",
        out.stderr
    );
    // It opened no connection: the database is unreachable and the exit is 0.
}

#[test]
fn print_config_with_errors_lists_them_and_exits_78() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(&scratch, CONFIG);
    // The reference does not resolve: the variable is not set.
    let out = run_to_end(
        &["--print-config"],
        &[("ORCH_CONFIG_FILE", path_str(&config))],
    );
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("database.url: the environment variable DATABASE_URL is unset or empty"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.is_empty());
}

/// The files `compose.yaml` mounts, as the orchestrator reads them: `dev/orchestrator.yaml` and
/// `dev/orchestrator.live.yaml`, each beside its agents file and the MCP tokens file, with the
/// dummies of the compose environment. CI runs `--print-config` on both (ADR 0034).
#[test]
fn the_dev_configuration_files_are_valid_for_every_role_they_are_used_with() {
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dev");
    let secrets = [
        (
            "DATABASE_URL",
            "postgres://postgres:postgres@postgres:5432/orch",
        ),
        ("AGENT_REGISTRY_AGENT_TOKEN", "dev-registry-agent-token"),
        ("MOCK_AGENT_TOKEN", "dev-mock-token"),
        ("CODER_A2A_TOKEN", "dev-coder-token"),
        ("CHAT_A2A_TOKEN", "dev-chat-token"),
        ("RESEARCHER_A2A_TOKEN", "dev-researcher-token"),
        (
            "THREAD_TOOLS_SECRET",
            "dev-thread-tools-secret-0123456789abcdef0123456789abcdef",
        ),
        // The `websearch` tool server's bearer and header (compose.yaml's dummy values).
        ("WEBSEARCH_TOKEN", "dev-search-token"),
        ("WEBSEARCH_TENANT", "dev-tenant-5c1f0a7e"),
    ];
    let routes = [
        (
            "WEBHOOK_GENERIC_SECRETS",
            "dev-webhook-secret-0123456789abcdef0123",
        ),
        (
            "WEBHOOK_GITHUB_SECRETS",
            "dev-webhook-secret-0123456789abcdef0123",
        ),
        ("MCP_TOKEN_DEV", "dev-mcp-token-0123456789abcdef0123456789"),
    ];
    for (config, agents) in [
        ("orchestrator.yaml", "agents.yaml"),
        ("orchestrator.live.yaml", "agents.live.yaml"),
    ] {
        // The three files side by side under the names compose mounts them with.
        let scratch = Scratch::new();
        fs::copy(dev.join(config), scratch.file("config.yaml")).unwrap();
        fs::copy(dev.join(agents), scratch.file("agents.yaml")).unwrap();
        fs::copy(dev.join("mcp-tokens.yaml"), scratch.file("mcp-tokens.yaml")).unwrap();
        let file = scratch.file("config.yaml");
        // The control plane (or both): it needs the secrets of the routes it serves.
        let mut env = secrets.to_vec();
        env.extend(routes);
        env.push(("ORCH_CONFIG_FILE", path_str(&file)));
        let out = run_to_end(&["--print-config"], &env);
        assert_eq!(out.status.code(), Some(0), "{config}: {}", out.stderr);
        assert!(
            out.stdout.contains("env: DATABASE_URL"),
            "{config}: {}",
            out.stdout
        );
        assert!(
            !out.stdout.contains("dev-mcp-token")
                && !out.stdout.contains("postgres:postgres")
                && !out.stdout.contains("dev-search-token")
                && !out.stdout.contains("dev-tenant-5c1f0a7e")
        );
        // A worker (the split profile) reads the same file and is not given the routes' secrets.
        let mut env = secrets.to_vec();
        env.extend([
            ("ORCH_CONFIG_FILE", path_str(&file)),
            ("ORCH_ROLE", "worker"),
            ("ORCH_INSTANCE_ID", "orchestrator-worker-1"),
            ("OUTBOX_LEASE_SECS", "5"),
        ]);
        let out = run_to_end(&["--print-config"], &env);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{config} as a worker: {}",
            out.stderr
        );
        assert!(out.stdout.contains("role: worker"), "{}", out.stdout);
        assert!(out.stdout.contains("outboxLeaseSecs: 5"), "{}", out.stdout);
        // Without the secrets of the routes, a control plane does not start.
        let mut env = secrets.to_vec();
        env.push(("ORCH_CONFIG_FILE", path_str(&file)));
        let out = run_to_end(&["--print-config"], &env);
        assert_eq!(out.status.code(), Some(78), "{config}: {}", out.stderr);
        assert!(
            out.stderr.contains("webhooks.generic.secrets[0]"),
            "{}",
            out.stderr
        );
    }
}

#[test]
fn a_variable_over_the_file_is_a_warning_and_a_process_override_is_info() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!("{CONFIG}server: {{ listen: 127.0.0.1:0 }}\n"),
    );
    // The database is unreachable on purpose: the notes are logged before anything connects, and
    // the test reads them and stops the process.
    let run = spawn(
        &scratch,
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", SECRET_URL),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("LISTEN_ADDR", "127.0.0.1:0"),
            ("DISPATCHER_CONCURRENCY", "4"),
            ("ORCH_ROLE", "all"),
            ("ORCH_INSTANCE_ID", "smoke-1"),
        ],
    );
    let log = wait_for_log(&run, "ORCH_INSTANCE_ID overrides", Duration::from_secs(20));
    let lines = json_lines(&log);
    let note = |var: &str| {
        lines
            .iter()
            .find(|l| l["fields"]["variable"] == var)
            .unwrap_or_else(|| panic!("no note for {var}:\n{log}"))
    };
    // LISTEN_ADDR says what the file says: not an override. DISPATCHER_CONCURRENCY is a variable the file leaves out.
    assert!(
        lines
            .iter()
            .all(|l| l["fields"]["variable"] != "LISTEN_ADDR"),
        "{log}"
    );
    let concurrency = note("DISPATCHER_CONCURRENCY");
    assert_eq!(concurrency["level"], "WARN");
    assert_eq!(concurrency["fields"]["key"], "dispatcher.concurrency");
    for var in ["ORCH_ROLE", "ORCH_INSTANCE_ID"] {
        assert_eq!(
            note(var)["level"],
            "INFO",
            "{var} is a process override: {log}"
        );
    }
    // The instance id of the override is on every line.
    assert_eq!(lines[0]["instance"], "smoke-1", "{log}");
    assert!(
        !log.contains("hunter2-s3cr3t"),
        "a secret reached the log:\n{log}"
    );
}

#[test]
fn without_a_file_the_environment_alone_starts_as_before_with_one_warning() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let run = spawn(
        &scratch,
        &[
            ("DATABASE_URL", SECRET_URL),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    let log = wait_for_log(&run, "no configuration file", Duration::from_secs(20));
    let warnings: Vec<_> = json_lines(&log)
        .into_iter()
        .filter(|l| {
            l["level"] == "WARN"
                && l["fields"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("no configuration file"))
        })
        .collect();
    assert_eq!(warnings.len(), 1, "{log}");
    assert!(!log.contains("hunter2-s3cr3t"), "{log}");
}

/// The legacy `chat-api` surface was removed on 2026-09-30. A deployment that still names it,
/// in the variable or on the flag, alone or beside `agui`, does not start quietly without the
/// routes it expects: the process exits 78 (EX_CONFIG) before it connects to anything, and the log
/// says what was removed and where to go.
#[test]
fn the_removed_chat_api_surface_is_a_config_error_pointing_to_agui() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    // The database is unreachable on purpose: configuration is validated first.
    let base_env = [
        ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
        ("AGENTS_FILE", path_str(&agents)),
        ("SMOKE_AGENT_TOKEN", TOKEN),
    ];
    // (how it is named, command-line arguments, extra environment)
    #[allow(clippy::type_complexity)]
    let cases: [(&str, &[&str], &[(&str, &str)]); 4] = [
        ("variable", &[], &[("ORCH_SURFACES", "chat-api")]),
        (
            "variable, beside agui",
            &[],
            &[("ORCH_SURFACES", "agui,chat-api")],
        ),
        ("flag", &["--surfaces", "agui,chat-api"], &[]),
        // The flag decides over a variable that would have been fine.
        (
            "flag over variable",
            &["--surfaces", "chat-api"],
            &[("ORCH_SURFACES", "agui")],
        ),
    ];
    for (how, args, extra) in cases {
        let mut env = base_env.to_vec();
        env.extend_from_slice(extra);
        let mut run = spawn_with_args(&scratch, "removed.log", args, &env);
        let status = run.wait(Duration::from_secs(10));
        let log = run.log();
        assert_eq!(status.code(), Some(78), "{how}: EX_CONFIG; {log}");
        for want in [
            "ORCH_SURFACES is invalid",
            // The log is JSON: the quotes around the name are escaped in it.
            "chat-api\\\" was removed on 2026-09-30",
            "Use AG-UI instead",
            "POST /agui/agents/{agentId}",
        ] {
            assert!(log.contains(want), "{how}: the log lacks {want:?}:\n{log}");
        }
        assert!(
            !log.contains("cannot connect to Postgres"),
            "{how}: nothing connects before the configuration is accepted: {log}"
        );
    }
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

/// The routes of the removed chat API interaction surface, as method and path.
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
    // The legacy interaction routes are gone. `POST /api/threads` shares its path with the
    // thread list, so it is a 405; the others match no route at all.
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

// ---- authentication (ADR 0033) ---------------------------------------------------------------

/// Starts the binary on a free port from a configuration file that is `CONFIG` and `extra`, and
/// waits for `/healthz` (the process is alive; it may not be ready).
async fn serve_configured(
    db: &pgdb::TestDb,
    scratch: &Scratch,
    log_name: &str,
    extra: &str,
) -> (std::cell::RefCell<Running>, String, reqwest::Client) {
    write_agents(scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(scratch, &format!("{CONFIG}{extra}"));
    let addr = format!("127.0.0.1:{}", free_port());
    let database_url = database_url_of(db);
    let run = std::cell::RefCell::new(spawn_with_args(
        scratch,
        log_name,
        &["--listen-addr", addr.as_str()],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", &database_url),
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

#[tokio::test]
async fn in_jwt_mode_only_a_valid_token_is_an_identity_and_readiness_follows_the_keys() {
    use orch_auth_jwt::testkit::TestIdp;
    use orch_ports::testkit::bearer::{Alg, Signing};

    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let idp = TestIdp::start().await;
    // The issuer cannot be reached when the process starts: nobody gets in, and it says so.
    idp.set_jwks_down(true);
    let (run, base, client) = serve_configured(
        &db,
        &scratch,
        "jwt.log",
        &format!(
            // Development: the local issuer is plain http, which production refuses.
            "auth:\n  mode: jwt\n  jwt:\n    issuer: {}\n    audiences: [orchestrator-web]\n",
            idp.issuer()
        ),
    )
    .await;
    let get = |path: &'static str, headers: Vec<(&'static str, String)>| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            let mut req = client.get(format!("{base}{path}"));
            for (name, value) in headers {
                req = req.header(name, value);
            }
            req.send().await.unwrap()
        }
    };
    let bearer = |token: &str| vec![("Authorization", format!("Bearer {token}"))];
    let good = idp.token("orchestrator-web", "alice@example.com");

    let ready = get("/readyz", vec![]).await;
    assert_eq!(ready.status(), 503, "never fetched: not ready");
    let down = get("/api/agents", bearer(&good)).await;
    assert_eq!(down.status(), 503, "a valid token cannot be checked yet");
    assert!(down.headers().contains_key("retry-after"));
    assert_eq!(
        get("/healthz", vec![]).await.status(),
        200,
        "alive all the while"
    );

    // The issuer comes back; readiness (what the probe asks) is what fetches the keys.
    idp.set_jwks_down(false);
    eventually("the keys are fetched and /readyz says ready", || async {
        (get("/readyz", vec![]).await.status() == 200).then_some(())
    })
    .await;

    let ok = get("/api/agents", bearer(&good)).await;
    assert_eq!(ok.status(), 200);
    // No token: 401 with the challenge. The identity header is not an identity in this mode.
    let none = get("/api/agents", vec![]).await;
    assert_eq!(none.status(), 401);
    assert_eq!(
        none.headers()["www-authenticate"],
        "Bearer realm=\"orchestrator\""
    );
    let header_only = get(
        "/api/agents",
        vec![("X-Auth-Request-Email", "alice@example.com".into())],
    )
    .await;
    assert_eq!(
        header_only.status(),
        401,
        "a client-supplied header is no identity"
    );
    // A bad token: 401 saying so, and the header beside it does not rescue it.
    let wrong_audience = idp.token("another-api", "alice@example.com");
    for (what, token) in [
        ("wrong audience", wrong_audience),
        ("garbage", "not.a.jwt".to_owned()),
        (
            "unpublished key",
            idp.mint(
                &idp.claims("orchestrator-web", "alice@example.com"),
                Signing::Unpublished(Alg::Rs256),
            ),
        ),
        (
            "alg none",
            idp.mint(
                &idp.claims("orchestrator-web", "alice@example.com"),
                Signing::NoneAlg,
            ),
        ),
    ] {
        let mut headers = bearer(&token);
        headers.push(("X-Auth-Request-Email", "alice@example.com".to_owned()));
        let r = get("/api/agents", headers).await;
        assert_eq!(r.status(), 401, "{what}");
        assert!(
            r.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("error=\"invalid_token\""),
            "{what}"
        );
    }
    // The token's user owns what it creates: alice's threads are not bob's.
    let bob = idp.token("orchestrator-web", "bob@example.com");
    assert_eq!(get("/api/threads", bearer(&good)).await.status(), 200);
    assert_eq!(get("/api/threads", bearer(&bob)).await.status(), 200);

    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("\"mode\":\"jwt\""), "{log}");
    assert!(
        !log.contains(&good) && !log.contains(&bob),
        "a token reached the log:\n{log}"
    );
}

/// ADR 0033: the roles of a token decide what it may do. The process is configured from a file
/// whose `auth.roles` define `admin` and `staff`, with no default role.
#[tokio::test]
async fn in_jwt_mode_the_roles_of_the_token_decide_what_it_may_do() {
    use orch_auth_jwt::testkit::TestIdp;
    use orch_ports::testkit::bearer::{Alg, Signing};

    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let idp = TestIdp::start().await;
    let (run, base, client) = serve_configured(
        &db,
        &scratch,
        "roles.log",
        &format!(
            "auth:
  mode: jwt
  jwt:
    issuer: {}
    audiences: [orchestrator-web]
    rolesClaim: realm_access.roles
  roles:
    staff:
      permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read]
    admin:
      permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read, admin]
      scope: {{ read: any, write: own }}
",
            idp.issuer()
        ),
    )
    .await;
    // Ready once the keys are fetched.
    eventually("the keys are fetched", || async {
        (http_status(&client, &format!("{base}/readyz")).await == Some(200)).then_some(())
    })
    .await;
    let token = |email: &str, roles: &[&str]| {
        let mut claims = idp.claims("orchestrator-web", email);
        claims.insert(
            "realm_access".to_owned(),
            serde_json::json!({ "roles": roles }),
        );
        idp.mint(&claims, Signing::Published(Alg::Rs256))
    };
    let get = |path: &str, token: &str| {
        let (client, url, token) = (client.clone(), format!("{base}{path}"), token.to_owned());
        async move {
            let resp = client.get(url).bearer_auth(token).send().await.unwrap();
            let status = resp.status().as_u16();
            (
                status,
                resp.json::<serde_json::Value>().await.unwrap_or_default(),
            )
        }
    };
    let staff = token("sam@example.com", &["staff", "offline_access"]);
    let admin = token("ada@example.com", &["admin"]);
    let nobody = token("nia@example.com", &["offline_access"]);

    // `/api/me` says who and what, from the token's roles claim and `auth.roles`.
    let (status, me) = get("/api/me", &staff).await;
    assert_eq!(status, 200);
    assert_eq!(me["user"], "sam@example.com");
    assert_eq!(
        me["roles"],
        serde_json::json!(["staff"]),
        "roles the file does not define do not count"
    );
    let (_, me) = get("/api/me", &admin).await;
    assert_eq!(me["roles"], serde_json::json!(["admin"]));
    let scope = |me: &serde_json::Value, permission: &str| {
        me["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["permission"] == permission)
            .map(|p| p["scope"].clone())
    };
    assert_eq!(scope(&me, "thread.read"), Some(serde_json::json!("any")));
    assert_eq!(scope(&me, "thread.write"), Some(serde_json::json!("own")));

    // The administrator lists everyone's threads; staff do not.
    assert_eq!(get("/api/threads?owner=*", &admin).await.0, 200);
    let (status, problem) = get("/api/threads?owner=*", &staff).await;
    assert_eq!((status, problem["code"].as_str()), (403, Some("forbidden")));
    // A person none of whose roles the file defines, with no default role, is refused everywhere
    // but `/api/me`, which says why.
    for path in ["/api/agents", "/api/threads", "/api/config"] {
        let (status, problem) = get(path, &nobody).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (403, Some("no_access")),
            "{path}"
        );
    }
    let (status, me) = get("/api/me", &nobody).await;
    assert_eq!(status, 200);
    assert_eq!(me["roles"], serde_json::json!([]));
    assert_eq!(me["permissions"], serde_json::json!([]));

    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(
        !log.contains(&staff) && !log.contains(&admin),
        "a token reached the log:\n{log}"
    );
}

#[test]
fn a_production_process_refuses_the_proxy_header_before_anything_connects() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!("{CONFIG}server: {{ environment: production }}\n"),
    );
    let out = run_to_end(
        &[],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", SECRET_URL),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("auth.mode: proxy_header is for a single user on a local machine"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("hunter2-s3cr3t"));
}

// ---- the MCP surface (ADR 0019) ---------------------------------------------------------------

const MCP_TOKENS: &str = "- user: mcp-user@example.com\n  tokenEnv: MCP_TOKEN_SMOKE\n";
const MCP_TOKEN: &str = "smoke-mcp-bearer-0123456789abcdef0123456789";

fn write_mcp_tokens(scratch: &Scratch) -> PathBuf {
    let path = scratch.file("mcp-tokens.yaml");
    fs::write(&path, MCP_TOKENS).unwrap();
    path
}

/// The MCP surface is fail closed: named in `ORCH_SURFACES` without its tokens, its token
/// variable or its hosts, the process stops with the configuration exit code before anything
/// connects, says which variable is missing and never prints a token.
#[cfg(feature = "surface-mcp")]
#[test]
fn mcp_without_its_tokens_or_hosts_is_a_config_error() {
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let tokens = write_mcp_tokens(&scratch);
    let unreachable = "postgres://nobody@127.0.0.1:1/none";
    let full = [
        ("DATABASE_URL", unreachable),
        ("AGENTS_FILE", path_str(&agents)),
        ("SMOKE_AGENT_TOKEN", TOKEN),
        ("ORCH_SURFACES", "agui,mcp"),
        ("MCP_TOKENS_FILE", path_str(&tokens)),
        ("MCP_ALLOWED_HOSTS", "127.0.0.1"),
        ("MCP_TOKEN_SMOKE", MCP_TOKEN),
    ];
    for (missing, want) in [
        ("MCP_TOKENS_FILE", "MCP_TOKENS_FILE is required"),
        ("MCP_ALLOWED_HOSTS", "MCP_ALLOWED_HOSTS is required"),
        ("MCP_TOKEN_SMOKE", "MCP_TOKEN_SMOKE"),
    ] {
        let env: Vec<(&str, &str)> = full
            .iter()
            .copied()
            .filter(|(k, _)| *k != missing)
            .collect();
        let mut run = spawn(&scratch, &env);
        let status = run.wait(Duration::from_secs(10));
        let log = run.log();
        assert_eq!(
            status.code(),
            Some(78),
            "without {missing}: EX_CONFIG; {log}"
        );
        assert!(log.contains(want), "without {missing}: {log}");
        assert!(
            !log.contains("cannot connect to Postgres"),
            "nothing connects before the configuration is accepted: {log}"
        );
        assert!(!log.contains(MCP_TOKEN), "a token leaked into the log");
    }
    // A token that is too short to be a secret, and hosts that are not `Host` values (a URL, a
    // wildcard, a port that is not a number), are refused at startup too, never at the first call.
    for (var, bad, want) in [
        ("MCP_TOKEN_SMOKE", "short", "shorter than 32 bytes"),
        (
            "MCP_ALLOWED_HOSTS",
            "https://orch.example.com",
            "MCP_ALLOWED_HOSTS",
        ),
        ("MCP_ALLOWED_HOSTS", "*", "MCP_ALLOWED_HOSTS"),
        (
            "MCP_ALLOWED_HOSTS",
            "orch.example.com:port",
            "MCP_ALLOWED_HOSTS",
        ),
        (
            "MCP_ALLOWED_ORIGINS",
            "orch.example.com",
            "MCP_ALLOWED_ORIGINS",
        ),
    ] {
        let mut env = full.to_vec();
        env.retain(|(k, _)| *k != var);
        env.push((var, bad));
        let mut run = spawn(&scratch, &env);
        let status = run.wait(Duration::from_secs(10));
        let log = run.log();
        assert_eq!(status.code(), Some(78), "{var}={bad}: EX_CONFIG; {log}");
        assert!(log.contains(want), "{var}={bad}: {log}");
        assert!(!log.contains(MCP_TOKEN), "a token leaked into the log");
        assert!(!log.contains("cannot connect to Postgres"), "{log}");
    }
    // A token file that is not there is the same kind of error.
    let mut env = full.to_vec();
    env.retain(|(k, _)| *k != "MCP_TOKENS_FILE");
    env.push(("MCP_TOKENS_FILE", "/nonexistent/mcp-tokens.yaml"));
    let mut run = spawn(&scratch, &env);
    assert_eq!(
        run.wait(Duration::from_secs(10)).code(),
        Some(78),
        "{}",
        run.log()
    );
    assert!(
        run.log().contains("cannot read MCP_TOKENS_FILE"),
        "{}",
        run.log()
    );
}

/// Parses the JSON-RPC messages of a response that is JSON or an SSE stream.
fn rpc_messages(body: &str) -> Vec<serde_json::Value> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        return vec![serde_json::from_str(trimmed).unwrap()];
    }
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str(d.trim()).ok())
        .collect()
}

async fn mcp_call(
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> (u16, Vec<serde_json::Value>) {
    let mut req = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .json(body);
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, rpc_messages(&resp.text().await.unwrap()))
}

/// The thread-tools endpoint in the real binary: it is there only when named, it is a machine route
/// that a token minted with the configured key opens (and nothing else does), and the thread the
/// token names has to exist and be the token's agent's.
#[cfg(feature = "surface-thread-tools")]
#[tokio::test]
async fn the_thread_tools_endpoint_is_mounted_by_its_name_and_a_minted_token_opens_it() {
    use orch_core::{AgentId, ThreadId, ToolsGrant};
    use orch_thread_token::{ThreadToolsIssuer, ThreadToolsKeys};
    use secrecy::{ExposeSecret, SecretString};

    const KEY: &str = "not-a-real-secret-smoke-thread-tools-0123456789abcdef";
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
    let addr = format!("127.0.0.1:{}", free_port());
    let base = format!("http://{addr}");
    let database_url = database_url_of(&db);
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
            ("ORCH_SURFACES", "agui,thread-tools"),
            ("THREAD_TOOLS_SECRET", KEY),
            ("THREAD_TOOLS_URL", &base),
        ],
    ));
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

    // A thread to open the endpoint of: one run through AG-UI, to the fake agent `fake`.
    let chat = Chat::new(&base, "alice@example.com");
    let id = uuid::Uuid::now_v7().to_string();
    let input = Chat::agui_input(
        &id,
        "run-1",
        &[("msg-1", "echo smoke")],
        serde_json::json!({}),
    );
    let frames = chat
        .agui_run("fake", &input)
        .await
        .collect_frames(Duration::from_secs(20))
        .await;
    assert_eq!(
        frames.last().map(|f| f.event["type"].clone()),
        Some("RUN_FINISHED".into())
    );
    let thread: ThreadId = id.parse().unwrap();

    // Tokens as the A2A adapter would mint them, with the key and the URL the binary was given.
    let issuer = ThreadToolsIssuer::new(
        ThreadToolsKeys::new(SecretString::from(KEY.to_owned()), None).unwrap(),
        &base,
        Duration::from_secs(7200),
    )
    .unwrap();
    let mint = |thread: ThreadId, agent: &str| {
        issuer
            .grant(
                &ToolsGrant::main(thread, 1, AgentId::new(agent)),
                "smoke-message",
                jiff::Timestamp::now(),
            )
            .unwrap()
    };
    let list = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let post = |url: String, token: Option<String>| {
        let client = client.clone();
        let list = list.clone();
        async move {
            let mut req = client
                .post(url)
                .header("Accept", "application/json, text/event-stream")
                // an edge identity changes nothing on a machine route
                .header("X-Auth-Request-Email", "mallory@example.com")
                .json(&list);
            if let Some(token) = token {
                req = req.bearer_auth(token);
            }
            let resp = req.send().await.unwrap();
            let status = resp.status().as_u16();
            let challenge = resp
                .headers()
                .get("www-authenticate")
                .map(|v| v.to_str().unwrap().to_owned());
            (
                status,
                challenge,
                rpc_messages_or_empty(&resp.text().await.unwrap()),
            )
        }
    };

    let url = issuer.url_for(thread);
    // no token; a token that is not one; a token for a thread that does not exist
    let (status, challenge, _) = post(url.clone(), None).await;
    assert_eq!((status, challenge.as_deref()), (401, Some("Bearer")));
    let invalid = Some(r#"Bearer error="invalid_token""#);
    let (status, challenge, _) = post(url.clone(), Some("not-a-token".to_owned())).await;
    assert_eq!((status, challenge.as_deref()), (401, invalid));
    let nobody = ThreadId(uuid::Uuid::now_v7());
    let (status, challenge, _) = post(
        issuer.url_for(nobody),
        Some(mint(nobody, "fake").token.expose_secret().to_owned()),
    )
    .await;
    assert_eq!((status, challenge.as_deref()), (401, invalid));
    // a token for another agent than the thread's
    let (status, challenge, _) = post(
        url.clone(),
        Some(mint(thread, "other").token.expose_secret().to_owned()),
    )
    .await;
    assert_eq!((status, challenge.as_deref()), (401, invalid));
    // the thread's agent: the one tool, over a route that is not under /mcp
    let (status, _, messages) = post(
        url,
        Some(mint(thread, "fake").token.expose_secret().to_owned()),
    )
    .await;
    assert_eq!(status, 200, "{messages:?}");
    let tools: Vec<&str> = messages
        .iter()
        .find_map(|m| m["result"]["tools"].as_array())
        .expect("a tools/list result")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["get_ui_catalog", "turn_output"]);
    // and the token never reached the log
    assert!(
        !run.borrow().log().contains(KEY),
        "the key leaked into the log"
    );
}

/// The whole loop through the real binary: its A2A adapter mints a grant for a message to an agent
/// whose card lists `thread-tools/v1`, the agent (the fake agent's `thread-tools` script) calls the
/// binary's own endpoint back with it, and gets the catalog the screen sent. The binary is given
/// the key and the URL it is reached at, as a deployment gives every role.
#[cfg(feature = "surface-thread-tools")]
#[tokio::test]
async fn an_agent_that_lists_the_extension_calls_the_binary_back_with_the_grant_it_was_given() {
    const KEY: &str = "not-a-real-secret-smoke-loop-0123456789abcdef0123456789";
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let agent = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some(TOKEN.to_owned()),
        extensions: vec![orch_core::THREAD_TOOLS_EXTENSION.to_owned()],
        ..FakeAgentOptions::default()
    })
    .await;
    let scratch = Scratch::new();
    let agents = write_agents(&scratch, &agents_yaml(&agent.card_url()));
    let addr = format!("127.0.0.1:{}", free_port());
    let base = format!("http://{addr}");
    let database_url = database_url_of(&db);
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
            ("ORCH_SURFACES", "agui,thread-tools"),
            ("THREAD_TOOLS_SECRET", KEY),
            ("THREAD_TOOLS_URL", &base),
        ],
    ));
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

    // a run that carries the screen's catalog, to an agent that calls the endpoint back
    let chat = Chat::new(&base, "alice@example.com");
    let id = uuid::Uuid::now_v7().to_string();
    let input = Chat::agui_input(
        &id,
        "run-1",
        &[("msg-1", "thread-tools please")],
        orch_testsupport::with_ui_catalog(1),
    );
    let frames = chat
        .agui_run("fake", &input)
        .await
        .collect_frames(Duration::from_secs(30))
        .await;
    assert_eq!(
        frames.last().map(|f| f.event["type"].clone()),
        Some("RUN_FINISHED".into()),
        "{frames:?}"
    );
    let (status, export) = chat.get(&format!("/api/threads/{id}/export")).await;
    assert_eq!(status, 200);
    let said: Vec<&str> = export["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "artifact" && e["data"]["name"] == "result")
        .map(|e| e["data"]["text"].as_str().unwrap())
        .collect();
    let catalog = orch_testsupport::ui_catalog(1);
    assert_eq!(
        said,
        [format!(
            "thread-tools: tools=get_ui_catalog,turn_output; catalog={} v1 {}; again unchanged=true",
            orch_testsupport::UI_CATALOG_ID,
            catalog["digest"].as_str().unwrap()
        )],
        "the agent called the endpoint back with its grant and got the catalog; log:\n{}",
        run.borrow().log()
    );

    // the grant the agent was given names the binary's endpoint; neither it nor the key is in the
    // thread the chat reads or in the binary's log
    let grants: Vec<serde_json::Value> = agent
        .executions()
        .into_iter()
        .filter_map(|call| call.thread_tools)
        .collect();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["url"], format!("{base}/thread-tools/{id}/mcp"));
    let token = grants[0]["token"].as_str().unwrap();
    assert!(!export.to_string().contains(token));
    let log = run.borrow().log();
    assert!(
        !log.contains(token) && !log.contains(KEY),
        "a secret leaked into the log"
    );
}

/// The relay is composed by the binary (feature `tool-relay`, ADR 0024): the real process, its
/// configuration file listing a web search with a bearer, a fake agent that lists `thread-tools/v1`
/// and calls the relayed tool with the grant it was given, and a real MCP server that wants the
/// bearer. The call is one step with the server's icon; neither the bearer nor the grant token is
/// in the thread or in the binary's log.
#[cfg(feature = "tool-relay")]
#[tokio::test]
async fn the_binary_relays_an_attached_servers_tool_with_the_configured_bearer() {
    const KEY: &str = "not-a-real-secret-smoke-relay-0123456789abcdef0123456789";
    const BEARER: &str = "smoke-relay-bearer-3b9d51e7a2c84f60-not-a-real-credential";
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let server = orch_testsupport::FakeToolServer::spawn(
        orch_testsupport::FakeToolServerOptions::default().bearer(BEARER),
    )
    .await;
    let agent = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some(TOKEN.to_owned()),
        extensions: vec![orch_core::THREAD_TOOLS_EXTENSION.to_owned()],
        ..FakeAgentOptions::default()
    })
    .await;
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml(&agent.card_url()));
    let config = write_config(
        &scratch,
        &format!(
            "{CONFIG}toolServers:\n  - id: websearch\n    name: Web search\n    url: {}\n\
             \x20   icon: \"data:image/svg+xml;base64,PHN2Zy8+\"\n    bearer: {{ env: SEARCH_TOKEN }}\n    agents: [fake]\n",
            server.url()
        ),
    );
    let addr = format!("127.0.0.1:{}", free_port());
    let base = format!("http://{addr}");
    let database_url = database_url_of(&db);
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("SEARCH_TOKEN", BEARER),
            ("NO_PROXY", "127.0.0.1,localhost"),
            ("ORCH_SURFACES", "agui,thread-tools"),
            ("THREAD_TOOLS_SECRET", KEY),
            ("THREAD_TOOLS_URL", &base),
        ],
    ));
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

    let chat = Chat::new(&base, "alice@example.com");
    let (status, servers) = chat.tool_servers().await;
    assert_eq!(status, 200, "{servers}");
    assert_eq!(servers[0]["id"], "websearch");
    let (status, created) = chat
        .try_create_thread_with_tools(
            "fake",
            r#"tool websearch__echo {"text":"relayed"}"#,
            &["websearch"],
        )
        .await;
    assert_eq!(status, 200, "{created}");
    let id = created["threadId"].as_str().unwrap().to_owned();
    chat.wait_state(&id, "done").await;
    let (status, export) = chat.get(&format!("/api/threads/{id}/export")).await;
    assert_eq!(status, 200);
    let events = export["events"].as_array().unwrap();
    let said: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "artifact" && e["data"]["name"] == "result")
        .map(|e| e["data"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        said,
        [r#"tool websearch__echo: {"text":"relayed"}"#],
        "log:\n{}",
        run.borrow().log()
    );
    let steps: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "agent_step")
        .map(|e| &e["data"])
        .collect();
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0]["icon"], "mcp-server:websearch");
    assert_eq!(steps[1]["state"], "completed");

    // the server saw the configured bearer; it is nowhere the binary says or keeps
    let seen = server.seen();
    let called = seen.iter().find(|r| r.method == "tools/call").unwrap();
    assert_eq!(called.bearer.as_deref(), Some(BEARER));
    let token = agent
        .executions()
        .into_iter()
        .find_map(|c| c.thread_tools)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let log = run.borrow().log();
    for secret in [BEARER, KEY, token.as_str()] {
        assert!(
            !export.to_string().contains(secret),
            "a secret is in the thread"
        );
        assert!(!log.contains(secret), "a secret leaked into the log");
    }
}

/// The JSON-RPC messages of a response, or none for an empty or non-JSON body (a refusal).
#[cfg(feature = "surface-thread-tools")]
fn rpc_messages_or_empty(body: &str) -> Vec<serde_json::Value> {
    if body.trim().is_empty() || !(body.trim_start().starts_with('{') || body.contains("data:")) {
        return Vec::new();
    }
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).into_iter().collect();
    }
    rpc_messages(body)
}

#[cfg(feature = "surface-mcp")]
#[tokio::test]
async fn mcp_is_mounted_by_its_name_and_a_token_lists_the_tools() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let tokens = write_mcp_tokens(&scratch);
    let agents = write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let addr = format!("127.0.0.1:{}", free_port());
    let database_url = database_url_of(&db);
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("AGENTS_FILE", path_str(&agents)),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
            ("ORCH_SURFACES", "agui,mcp"),
            ("MCP_TOKENS_FILE", path_str(&tokens)),
            ("MCP_ALLOWED_HOSTS", "127.0.0.1"),
            ("MCP_TOKEN_SMOKE", MCP_TOKEN),
            ("ORCH_PUBLIC_URL", "https://chat.example.com"),
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

    let list = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    // No token, or a wrong one: 401 with the challenge, whatever identity header comes along.
    for token in [None, Some("wrong")] {
        let resp = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .header("X-Auth-Request-Email", "mcp-user@example.com")
            .bearer_auth(token.unwrap_or(""))
            .json(&list)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 401);
        assert_eq!(resp.headers()["www-authenticate"], "Bearer");
    }
    // With the token: one tools/list gives the five tools.
    let (status, messages) = mcp_call(&client, &base, Some(MCP_TOKEN), &list).await;
    assert_eq!(status, 200, "{messages:?}");
    let tools: Vec<&str> = messages
        .iter()
        .find_map(|m| m["result"]["tools"].as_array())
        .expect("a tools/list result")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        [
            "list_agents",
            "start_job",
            "get_job",
            "wait_for_job",
            "answer",
            "cancel_job"
        ]
    );
    // A tool runs as the token's user: the agent list is the configured one.
    let (status, messages) = mcp_call(
        &client,
        &base,
        Some(MCP_TOKEN),
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "list_agents", "arguments": {}}}),
    )
    .await;
    assert_eq!(status, 200);
    let agents_result = messages
        .iter()
        .find_map(|m| m["result"]["structuredContent"]["agents"].as_array())
        .expect("a list_agents result");
    assert_eq!(agents_result[0]["id"], "fake");
    // The Host header is checked: a name that is not listed is refused, token or not.
    let resp = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("Host", "attacker.example.com")
        .bearer_auth(MCP_TOKEN)
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    // A browser's `Origin` is refused unless listed (none is), and two `Authorization` headers
    // are not a token, whichever they carry.
    let resp = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("Origin", "https://attacker.example.com")
        .bearer_auth(MCP_TOKEN)
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let resp = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {MCP_TOKEN}"))
        .header("Authorization", format!("Bearer {MCP_TOKEN}"))
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(log.contains("\"surfaces\":\"agui,mcp\""), "{log}");
    assert!(log.contains("the MCP server is mounted at /mcp"), "{log}");
    assert!(!log.contains(MCP_TOKEN), "a token leaked into the log");
}

/// Without `mcp` in `ORCH_SURFACES` there is no `/mcp`, even in a build that has it.
#[tokio::test]
async fn mcp_is_not_there_unless_it_is_named() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (run, base, client) = serve_with(&db, &scratch, "no-mcp.log", &[]).await;
    let resp = client
        .post(format!("{base}/mcp"))
        .header("X-Auth-Request-Email", "alice@example.com")
        .bearer_auth(MCP_TOKEN)
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    assert!(resp.headers().get("www-authenticate").is_none());
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    assert!(status.success(), "{}", run.borrow().log());
}

/// Without `thread-tools` in `ORCH_SURFACES` there is no endpoint, even when the key and the URL are
/// set (the adapter of this process mints; another replica serves).
#[tokio::test]
async fn thread_tools_is_not_there_unless_it_is_named() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (run, base, client) = serve_with(
        &db,
        &scratch,
        "no-thread-tools.log",
        &[
            "--thread-tools-secret",
            "not-a-real-secret-smoke-thread-tools-0123456789abcdef",
            "--thread-tools-url",
            "http://orchestrator:8080",
        ],
    )
    .await;
    let resp = client
        .post(format!("{base}/thread-tools/{}/mcp", uuid::Uuid::now_v7()))
        .bearer_auth("anything")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    // Not a machine route here: the identity layer answers every path it does not know.
    assert_eq!(resp.status().as_u16(), 401);
    assert!(resp.headers().get("www-authenticate").is_none());
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    assert!(status.success(), "{}", run.borrow().log());
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
    // Ready can trail live (readiness pings the store): a bounded wait, not one look.
    eventually("the binary answers /readyz", || async {
        (http_status(&client, &format!("{base}/readyz")).await == Some(200)).then_some(())
    })
    .await;

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
    // ... and the removed legacy interaction routes are not there to read its log.
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
            // The multi-process tests below drive threads over AG-UI, the default: say so, so
            // they do not depend on it.
            ("ORCH_SURFACES", "agui"),
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
        // Ready can trail live (readiness pings the store), most of all on a loaded machine, so
        // it gets the same bounded wait instead of one look.
        eventually("the binary answers /readyz", || async {
            assert!(
                self.run.borrow_mut().exited().is_none(),
                "the binary exited early; log:\n{}",
                self.run.borrow().log()
            );
            (http_status(&self.client, &format!("{}/readyz", self.base)).await == Some(200))
                .then_some(())
        })
        .await;
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

/// What a viewer reads of the thread `id` over the AG-UI connect stream, up to the end of its run
/// (`?mode=run`): the log, projected. There is no route that returns the log as the core wrote
/// it; this is what the processes under test answer.
async fn read_run(chat: &Chat, id: &str) -> Vec<Frame> {
    let mut sse = chat.agui_connect(id, None, true).await;
    sse.collect_frames(Duration::from_secs(20)).await
}

/// The resume points of the frames: the `seq` of each log event that ended a frame.
fn seqs(frames: &[Frame]) -> Vec<i64> {
    frames.iter().filter_map(|f| f.id).collect()
}

/// The five events of a plain successful run (a user message, working, one artifact, completed,
/// done) reached the viewer exactly once and in order: contiguous `seq` 1 to 5, one run, one
/// artifact, and the run finished with success.
fn assert_a_clean_run(frames: &[Frame]) {
    assert_eq!(seqs(frames), [1, 2, 3, 4, 5], "seq stays contiguous");
    let count = |pred: &dyn Fn(&Frame) -> bool| frames.iter().filter(|f| pred(f)).count();
    assert_eq!(count(&|f| f.event["type"] == "RUN_STARTED"), 1);
    assert_eq!(
        count(&|f| f.event["activityType"] == "vymalo.artifact"),
        1,
        "each update exactly once"
    );
    let last = &frames.last().unwrap().event;
    assert_eq!(last["type"], "RUN_FINISHED");
    assert_eq!(last["outcome"], serde_json::json!({"type": "success"}));
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
    assert_a_clean_run(&read_run(&chat, &id).await);
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
    let mut sse = chat_b.agui_connect(&id, None, false).await;
    assert_eq!(sse.status, 200);
    chat_b.wait_state(&id, "working").await;
    agent.release_gate();
    let frames = sse
        .frames_until(Duration::from_secs(20), |f| {
            f.event["type"] == "RUN_FINISHED"
        })
        .await;
    assert_a_clean_run(&frames);
    // Whichever process dispatched it, the agent saw the message once, and A reads the same log.
    assert_eq!(agent.executions().len(), 1);
    assert_eq!(read_run(&a.chat(), &id).await, read_run(&chat_b, &id).await);

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
    let mut sse = chat.agui_connect(&id, None, false).await;
    assert_eq!(sse.status, 200);
    eventually("the agent executes the task", || async {
        (agent.executions().len() == 1).then_some(())
    })
    .await;
    agent.release_gate();
    let frames = sse
        .frames_until(Duration::from_secs(20), |f| {
            f.event["type"] == "RUN_FINISHED"
        })
        .await;
    assert_a_clean_run(&frames);
    chat.wait_state(&id, "done").await;
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
    assert_a_clean_run(&read_run(&chat, &id).await);
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

/// The inbox worker is a worker component: a control plane alone leaves a due timer where it is,
/// and a worker process (which polls, since a timer coming due is not announced) applies it. The
/// timer is seeded with SQL because nothing schedules one from outside; it is for a thread the
/// gate is not watching, which the core takes as a stale deadline and changes nothing for.
#[tokio::test]
async fn a_due_timer_waits_for_a_worker_process_and_is_applied_by_it() {
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
    cp.wait_ready().await;
    let chat = cp.chat();
    let id = chat.create_thread("fake", "gate inbox split", None).await;

    let pool = db.pool("smoke-inbox", 2).await;
    let row = uuid::Uuid::now_v7();
    let payload = serde_json::json!({
        "kind": "timer",
        "thread": id,
        "timer": {"kind": "ci_deadline", "attempt": 1, "verification": 1},
    });
    sqlx::query(
        "INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, available_at, \
         created_at, updated_at) VALUES ($1, 'timer', 'smoke', 'timer', $2, 'pending', now(), \
         now(), now())",
    )
    .bind(row)
    .bind(&payload)
    .execute(&pool)
    .await
    .unwrap();
    let state = || async {
        sqlx::query_as::<_, (String, i32)>("SELECT status, attempts FROM inbox WHERE id = $1")
            .bind(row)
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    // A control plane runs no inbox worker: the row is due and stays so.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(state().await, ("pending".to_owned(), 0));

    let worker = Replica::start_with(
        &scratch,
        "worker.log",
        &url,
        &agents,
        &[],
        &[
            ("ORCH_ROLE", "worker"),
            ("ORCH_INSTANCE_ID", "w1"),
            ("INBOX_POLL_SECS", "1"),
        ],
    );
    worker.wait_ready().await;
    eventually("the worker applies the timer", || async {
        (state().await.0 == "applied").then_some(())
    })
    .await;
    assert_eq!(state().await.1, 1, "claimed once");
    agent.release_gate();

    for replica in [&worker, &cp] {
        let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
        assert!(status.success(), "log:\n{}", replica.run.borrow().log());
    }
}

/// The verifier through the real process: the deployment requires it (`ORCH_GATE`, `ORCH_VERIFIER`,
/// `ORCH_VERIFIER_TIMEOUT_SECS`), the worker pushes, the verifier agent finds something and is
/// satisfied by the rework, and the run ends once, in success. The verifier's own entry leaves the
/// source out for itself (an agent cannot verify its own work).
#[tokio::test]
async fn a_verifier_gate_runs_through_the_real_binary() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let agent = |verifier| async move {
        FakeAgent::spawn(FakeAgentOptions {
            bearer: Some(TOKEN.to_owned()),
            verifier,
            ..FakeAgentOptions::default()
        })
        .await
    };
    let worker = agent(None).await;
    let reviewer = agent(Some(VerifierScript::FindingsThenPass)).await;
    let scratch = Scratch::new();
    let agents = write_agents(
        &scratch,
        &format!(
            "- id: fake\n  name: Fake agent\n  cardUrl: {}\n  tokenEnv: SMOKE_AGENT_TOKEN\n\
             - id: reviewer\n  name: Reviewer\n  cardUrl: {}\n  tokenEnv: SMOKE_AGENT_TOKEN\n  \
             gate: {{require: []}}\n",
            worker.card_url(),
            reviewer.card_url()
        ),
    );
    let url = database_url_of(&db);
    let replica = Replica::start(
        &scratch,
        "verifier.log",
        &url,
        &agents,
        &[
            ("ORCH_GATE", "verifier"),
            ("ORCH_VERIFIER", "reviewer"),
            ("ORCH_VERIFIER_TIMEOUT_SECS", "60"),
            ("OUTBOX_LEASE_SECS", "5"),
        ],
    );
    replica.wait_ready().await;
    let chat = replica.chat();
    let id = uuid::Uuid::now_v7().to_string();
    let input = Chat::agui_input(
        &id,
        "run-1",
        &[("msg-1", "verify-reviewed fix the login")],
        serde_json::json!({}),
    );
    let mut sse = chat.agui_run("fake", &input).await;
    let frames = sse.collect_frames(Duration::from_secs(30)).await;
    let last = &frames.last().unwrap().event;
    assert_eq!(last["type"], "RUN_FINISHED", "{last}");
    assert_eq!(last["outcome"], serde_json::json!({"type": "success"}));
    let started: Vec<&str> = frames
        .iter()
        .filter(|f| f.event["type"] == "SUBAGENT_STARTED")
        .map(|f| f.event["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        started,
        ["fake", "reviewer", "fake", "reviewer"],
        "{started:?}"
    );
    let cards: Vec<(&str, &str)> = frames
        .iter()
        .filter(|f| f.event["activityType"] == "vymalo.check")
        .map(|f| {
            (
                f.event["content"]["source"].as_str().unwrap(),
                f.event["content"]["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        cards,
        [
            ("verifier", "pending"),
            ("verifier", "failed"),
            ("verifier", "pending"),
            ("verifier", "passed")
        ]
    );
    chat.wait_state(&id, "done").await;
    assert_eq!(
        chat.thread(&id).await["job"]["gate"],
        serde_json::json!(["verifier"])
    );
    assert_eq!(worker.executions().len(), 2, "the work, then the rework");
    let asked = reviewer.executions();
    assert_eq!(asked.len(), 2, "one verification per attempt");
    assert!(
        asked[0].context_id.ends_with("-verify-1-1")
            && asked[1].context_id.ends_with("-verify-2-2"),
        "{} {}",
        asked[0].context_id,
        asked[1].context_id
    );
    assert_eq!(
        asked[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );

    let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
    let log = replica.run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(!log.contains(TOKEN), "a secret leaked into the log");
}

/// The generic webhook through the real binary: `webhook-generic` mounted with a secret takes one
/// signed report with no identity (202, one inbox row, then the inbox worker parks it: no thread
/// watches that commit), refuses a bad signature (401, no second row), and leaves the rest of the
/// API behind the identity layer. The secret is nowhere in the log.
#[cfg(feature = "surface-webhook")]
#[tokio::test]
async fn the_generic_webhook_takes_one_signed_report_and_nothing_unsigned() {
    use orch_surface_webhook::signature::sign_generic;

    const SECRET: &str = "smoke-webhook-secret-0123456789abcdef0123";
    const DELIVERY: &str = "0195c1a2-7b3e-7c11-8f2a-5d6e7f809a1b";
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let (_agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);
    let replica = Replica::start(
        &scratch,
        "webhook.log",
        &url,
        &agents,
        &[
            ("ORCH_SURFACES", "agui,webhook-generic"),
            ("WEBHOOK_GENERIC_SECRETS", SECRET),
            ("INBOX_POLL_SECS", "1"),
        ],
    );
    replica.wait_ready().await;

    let sha = "0123456789abcdef0123456789abcdef01234567";
    let body = serde_json::json!({
        "version": 1,
        "repository": "https://github.com/acme/widgets",
        "sha": sha,
        "branch": "agent/fix-flaky-test",
        "name": "ci/build",
        "conclusion": "success",
        "url": "https://ci.example.com/runs/42",
        "summary": "212 tests passed",
    })
    .to_string();
    // One timestamp for every delivery of the test: the same timestamp and body are one delivery.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let key = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(format!("{ts}.").as_bytes());
        hasher.update(body.as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let post = |secret: &str, delivery: &str| {
        let ts = ts.clone();
        let signature = sign_generic(secret, &ts, body.as_bytes()).unwrap();
        replica
            .client
            .post(format!("{}/webhooks/ci", replica.base))
            .header("X-Vymalo-Delivery", delivery)
            .header("X-Vymalo-Timestamp", ts)
            .header("X-Vymalo-Signature-256", signature)
            .body(body.clone())
            .send()
    };
    let pool = db.pool("smoke-webhook", 2).await;
    let rows = || async {
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT source, idempotency_key, status FROM inbox ORDER BY created_at",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };

    // A bad signature: 401 and nothing stored.
    assert_eq!(
        post("not-the-secret", DELIVERY).await.unwrap().status(),
        401
    );
    assert!(rows().await.is_empty());

    // A good one: 202, one row; a redelivery, and a replay under another delivery id, is 202 and
    // still one row.
    assert_eq!(post(SECRET, DELIVERY).await.unwrap().status(), 202);
    assert_eq!(post(SECRET, DELIVERY).await.unwrap().status(), 202);
    assert_eq!(post(SECRET, "another-id").await.unwrap().status(), 202);
    eventually(
        "the inbox worker parks the report no thread waits for",
        || async {
            let rows = rows().await;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(
                (rows[0].0.as_str(), rows[0].1.as_str()),
                ("generic", key.as_str()),
                "keyed by the digest of the signed string, not by the delivery id"
            );
            (rows[0].2 == "parked").then_some(())
        },
    )
    .await;

    // The rest of the API is still behind the identity layer.
    let anonymous = replica
        .client
        .get(format!("{}/api/agents", replica.base))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
    let log = replica.run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(
        log.contains("webhook-generic"),
        "the surface is logged: {log}"
    );
    assert!(!log.contains(SECRET), "the secret leaked into the log");
}

/// The GitHub webhook through the real binary: `webhook-github` mounted with a secret answers a
/// `ping` with 204, stores a signed `check_run` (the synthetic fixture of the webhook crate, dated
/// now) and acknowledges a `push` and a `check_suite` without storing them, and refuses a bad
/// signature.
#[cfg(feature = "surface-webhook")]
#[tokio::test]
async fn the_github_webhook_takes_a_signed_check_run_and_acknowledges_the_rest() {
    use orch_surface_webhook::signature::sign_github;

    const SECRET: &str = "smoke-github-secret-0123456789abcdef012345";
    const DELIVERY: &str = "72d3162e-cc78-11e3-81ab-4c9367dc0958";
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let fixture = |name: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/surface-webhook/testdata/github")
            .join(format!("{name}.json"));
        fs::read(path).unwrap()
    };
    let scratch = Scratch::new();
    let (_agent, agents) = agent_and_list(&scratch).await;
    let url = database_url_of(&db);
    let replica = Replica::start(
        &scratch,
        "github.log",
        &url,
        &agents,
        &[
            ("ORCH_SURFACES", "agui,webhook-github"),
            ("WEBHOOK_GITHUB_SECRETS", SECRET),
            ("INBOX_POLL_SECS", "1"),
        ],
    );
    replica.wait_ready().await;
    let post = |secret: &str, event: &str, delivery: &str, body: &[u8]| {
        replica
            .client
            .post(format!("{}/webhooks/github", replica.base))
            .header("X-GitHub-Event", event)
            .header("X-GitHub-Delivery", delivery)
            .header("X-Hub-Signature-256", sign_github(secret, body).unwrap())
            .body(body.to_vec())
            .send()
    };
    let pool = db.pool("smoke-github", 2).await;
    let rows = || async {
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT source, idempotency_key, status FROM inbox ORDER BY created_at",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };

    // The fixture is dated on the day it was written; the route refuses an event older than a day,
    // so the test dates it now.
    let now = jiff::Timestamp::now().to_string();
    let run = {
        let mut v: serde_json::Value =
            serde_json::from_slice(&fixture("check_run.completed.failure")).unwrap();
        v["check_run"]["completed_at"] = serde_json::json!(now);
        v.to_string().into_bytes()
    };
    let suite = fixture("check_suite.completed.success");
    assert_eq!(
        post("guess", "check_run", DELIVERY, &run)
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        post(SECRET, "ping", DELIVERY, &fixture("ping"))
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        post(SECRET, "push", DELIVERY, &fixture("push"))
            .await
            .unwrap()
            .status(),
        202
    );
    assert_eq!(
        post(SECRET, "check_suite", DELIVERY, &suite)
            .await
            .unwrap()
            .status(),
        202,
        "a check_suite is named by an app, not a check: acknowledged, not stored"
    );
    assert!(
        rows().await.is_empty(),
        "refused, ping, push and check_suite wrote nothing"
    );

    assert_eq!(
        post(SECRET, "check_run", DELIVERY, &run)
            .await
            .unwrap()
            .status(),
        202
    );
    // A redelivery, and a replay under another delivery id: the key is the signed body's.
    assert_eq!(
        post(SECRET, "check_run", "another-id", &run)
            .await
            .unwrap()
            .status(),
        202
    );
    eventually(
        "the inbox worker parks the report no thread waits for",
        || async {
            let rows = rows().await;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(
                (rows[0].0.as_str(), rows[0].1.as_str()),
                ("github", format!("check_run:128620228:{now}").as_str())
            );
            (rows[0].2 == "parked").then_some(())
        },
    )
    .await;

    let status = replica.run.borrow_mut().terminate(Duration::from_secs(20));
    let log = replica.run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(
        log.contains("webhook-github"),
        "the surface is logged: {log}"
    );
    assert!(!log.contains(SECRET), "the secret leaked into the log");
}

/// The platform's registry as a server for the smoke test: the document of `agent-registry/v1`,
/// answered with `Content-Type: application/linkset+json`, and a 503 while `down` is set.
mod platform {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::extract::State;
    use axum::http::{StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    #[derive(Clone)]
    struct Served {
        down: Arc<AtomicBool>,
        items: serde_json::Value,
    }

    async fn linkset(State(served): State<Served>) -> Response {
        if served.down.load(Ordering::SeqCst) {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let body = serde_json::json!({"linkset": [{
            "profile": [{"href": "https://agents.vymalo.com/registry/v1"}],
            "item": served.items,
        }]});
        (
            [
                (header::CONTENT_TYPE, "application/linkset+json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body.to_string(),
        )
            .into_response()
    }

    pub struct Platform {
        pub url: String,
        pub down: Arc<AtomicBool>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Platform {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    pub async fn start(items: serde_json::Value) -> Platform {
        let down = Arc::new(AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/registry/v1/agents",
            listener.local_addr().unwrap()
        );
        let router = axum::Router::new()
            .route("/registry/v1/agents", get(linkset))
            .with_state(Served {
                down: Arc::clone(&down),
                items,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Platform { url, down, server }
    }
}

/// The binary reads the platform's agent registry (ADR 0022): with no `AGENTS_FILE` at all, the
/// agents come from the registry, live; a thread runs on one of them, with the deployment's
/// agent token; and when the registry goes down its agents leave the list, the UI's endpoint
/// says so, and a run on one of them is a 503 with `Retry-After`, never a 404.
#[cfg(feature = "registry-platform")]
#[tokio::test]
async fn the_platforms_agents_are_served_with_no_agent_file_and_leave_when_the_registry_goes_down()
{
    use std::sync::atomic::Ordering;

    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let agent = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some(TOKEN.to_owned()),
        ..FakeAgentOptions::default()
    })
    .await;
    let platform = platform::start(serde_json::json!([{
        "href": agent.card_url(), "type": "application/json",
        "title": "Platform fake", "service": ["platform-fake"], "tags": ["testing"]
    }]))
    .await;

    let scratch = Scratch::new();
    let addr = format!("127.0.0.1:{}", free_port());
    let database_url = database_url_of(&db);
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("DATABASE_URL", &database_url),
            ("LISTEN_ADDR", &addr),
            ("AGENT_REGISTRY_URL", &platform.url),
            ("AGENT_REGISTRY_AGENT_TOKEN", TOKEN),
            ("NO_PROXY", "127.0.0.1,localhost"),
        ],
    ));
    let base = format!("http://{addr}");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    eventually("the binary answers /readyz", || async {
        assert!(
            run.borrow_mut().exited().is_none(),
            "the binary exited early; log:\n{}",
            run.borrow().log()
        );
        (http_status(&client, &format!("{base}/readyz")).await == Some(200)).then_some(())
    })
    .await;

    let chat = Chat::new(&base, "alice@example.com");
    let (status, agents) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    assert_eq!(agents.as_array().unwrap().len(), 1, "{agents}");
    assert_eq!(agents[0]["id"], "platform-fake");
    assert_eq!(agents[0]["name"], "Platform fake");
    assert_eq!(agents[0]["source"], "registry");
    assert_eq!(agents[0]["tags"], serde_json::json!(["testing"]));
    let (_, sources) = chat.get("/api/registry").await;
    assert_eq!(
        sources,
        serde_json::json!({"sources": [
            {"name": "static", "status": "ok"},
            {"name": "platform", "status": "ok"}
        ]})
    );

    // A thread runs on the registry's agent, which gets the deployment's agent token.
    let thread = chat
        .create_thread("platform-fake", "echo smoke", None)
        .await;
    chat.wait_state(&thread, "done").await;
    let calls = agent.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str()),
        "AGENT_REGISTRY_AGENT_TOKEN reaches the registry's agent"
    );

    // The registry goes down: its agents leave the list, and the endpoint for the UI says so.
    platform.down.store(true, Ordering::SeqCst);
    let (status, agents) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    assert_eq!(agents, serde_json::json!([]));
    let (_, sources) = chat.get("/api/registry").await;
    assert_eq!(sources["sources"][1]["status"], "unavailable");
    assert_eq!(
        sources["sources"][1]["detail"],
        "the registry could not be reached"
    );
    let (status, problem) = chat
        .try_create_thread("platform-fake", "echo again", None)
        .await;
    assert_eq!(status, 503, "{problem}");
    // ... and back.
    platform.down.store(false, Ordering::SeqCst);
    assert_eq!(chat.get("/api/agents").await.1[0]["id"], "platform-fake");

    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    let log = run.borrow().log();
    assert!(status.success(), "unclean exit {status:?}; log:\n{log}");
    assert!(!log.contains(TOKEN), "a secret leaked into the log: {log}");
}

/// A registry URL in a build that cannot read one is refused at startup, never ignored.
#[cfg(not(feature = "registry-platform"))]
#[test]
fn a_registry_url_in_a_build_without_the_registry_exits_78_naming_the_feature() {
    let scratch = Scratch::new();
    let mut run = spawn(
        &scratch,
        &[
            ("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none"),
            (
                "AGENT_REGISTRY_URL",
                "https://platform.example.com/registry/v1/agents",
            ),
        ],
    );
    let status = run.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(78), "EX_CONFIG; {}", run.log());
    assert!(run.log().contains("registry-platform"), "{}", run.log());
}

// ---- the artifact store (ADR 0032) -----------------------------------------------------------

#[cfg(feature = "artifacts-s3")]
#[test]
fn print_config_shows_the_artifact_store_with_references_and_never_a_credential() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!(
            "{CONFIG}artifacts:\n  store: s3\n  maxFileBytes: 2048\n  s3:\n    bucket: orchestrator-files\n    \
             endpoint: https://minio.example.com:9000\n    accessKeyId: {{ env: S3_ACCESS_KEY_ID }}\n    \
             secretAccessKey: {{ file: s3-secret }}\n"
        ),
    );
    fs::write(scratch.file("s3-secret"), "hunter2-s3-secret\n").unwrap();
    let out = run_to_end(
        &["--print-config"],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", SECRET_URL),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("S3_ACCESS_KEY_ID", "AKIDHUNTER2EXAMPLE"),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", out.stderr);
    for shown in [
        "artifacts:",
        "store: s3",
        "maxFileBytes: 2048",
        "bucket: orchestrator-files",
        "region: us-east-1",
        "env: S3_ACCESS_KEY_ID",
        "file: s3-secret",
    ] {
        assert!(out.stdout.contains(shown), "{shown}:\n{}", out.stdout);
    }
    for value in ["AKIDHUNTER2EXAMPLE", "hunter2-s3-secret"] {
        assert!(
            !out.stdout.contains(value) && !out.stderr.contains(value),
            "{value} is printed:\n{}\n{}",
            out.stdout,
            out.stderr
        );
    }
}

/// A store this build lacks, or a section that is not complete, is exit 78 naming the key (and
/// the feature), with no database to be reached first.
#[test]
fn an_artifact_store_that_is_not_usable_is_78_before_anything_connects() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let run_with = |section: &str| {
        let config = write_config(&scratch, &format!("{CONFIG}{section}"));
        run_to_end(
            &[],
            &[
                ("ORCH_CONFIG_FILE", path_str(&config)),
                ("DATABASE_URL", SECRET_URL),
                ("SMOKE_AGENT_TOKEN", TOKEN),
            ],
        )
    };
    let out = run_with(
        "artifacts: { store: s3, s3: { accessKeyId: { env: X }, secretAccessKey: { env: X } } }\n",
    );
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("artifacts.s3.bucket: required key is missing"),
        "{}",
        out.stderr
    );
    let out = run_with("artifacts: { store: fs }\n");
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("artifacts.fs: required when artifacts.store is fs"),
        "{}",
        out.stderr
    );
    let out = run_with("artifacts: { store: fs, fs: { root: x }, maxFileBytes: 0 }\n");
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(
        out.stderr.contains("artifacts.maxFileBytes: "),
        "{}",
        out.stderr
    );
}

/// Without the Cargo feature of the store the file names, the process does not start.
#[cfg(not(feature = "artifacts-fs"))]
#[test]
fn a_directory_store_in_a_build_without_it_exits_78_naming_the_feature() {
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let config = write_config(
        &scratch,
        &format!("{CONFIG}artifacts: {{ store: fs, fs: {{ root: files }} }}\n"),
    );
    let out = run_to_end(
        &[],
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", SECRET_URL),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    assert_eq!(out.status.code(), Some(78), "{}", out.stderr);
    assert!(out.stderr.contains("artifacts-fs"), "{}", out.stderr);
}

#[cfg(feature = "artifacts-fs")]
#[tokio::test]
async fn a_directory_store_is_made_at_startup_and_a_root_that_cannot_be_used_stops_it_with_78() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    write_agents(&scratch, &agents_yaml("https://a.example.com/card"));
    let database_url = database_url_of(&db);

    // a root that is a file: the process says which key and exits 78
    let blocked = scratch.file("a-file");
    fs::write(&blocked, b"x").unwrap();
    let config = write_config(
        &scratch,
        &format!(
            "{CONFIG}artifacts:\n  store: fs\n  fs: {{ root: {} }}\n",
            blocked.display()
        ),
    );
    let mut run = spawn(
        &scratch,
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", &database_url),
            ("SMOKE_AGENT_TOKEN", TOKEN),
        ],
    );
    let status = run.wait(Duration::from_secs(60));
    assert_eq!(status.code(), Some(78), "EX_CONFIG; {}", run.log());
    assert!(
        run.log().contains("artifacts.fs.root: cannot use"),
        "{}",
        run.log()
    );
    drop(run);

    // a root under the config file's directory that is not there yet: made, and the process serves
    let config = write_config(
        &scratch,
        &format!("{CONFIG}artifacts:\n  store: fs\n  fs: {{ root: data/artifacts }}\n"),
    );
    let addr = format!("127.0.0.1:{}", free_port());
    let run = std::cell::RefCell::new(spawn(
        &scratch,
        &[
            ("ORCH_CONFIG_FILE", path_str(&config)),
            ("DATABASE_URL", &database_url),
            ("SMOKE_AGENT_TOKEN", TOKEN),
            ("LISTEN_ADDR", &addr),
            ("NO_PROXY", "127.0.0.1,localhost"),
        ],
    ));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    eventually("the binary answers /healthz", || async {
        assert!(
            run.borrow_mut().exited().is_none(),
            "the binary exited early; log:\n{}",
            run.borrow().log()
        );
        (http_status(&client, &format!("http://{addr}/healthz")).await == Some(200)).then_some(())
    })
    .await;
    assert!(scratch.file("data/artifacts").is_dir());
    let log = run.borrow().log();
    assert!(
        log.contains("files from agents are kept in a directory"),
        "{log}"
    );
    let status = run.borrow_mut().terminate(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "{}", run.borrow().log());
}
