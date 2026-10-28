//! `orchestrator secrets rewrap` as a process (ADR 0046, slice 3): re-seals what people keep for
//! their own MCP servers under the current key, reads only the database and the `secrets` section,
//! and says how it ended in its exit code. The database tests need `ORCH_TEST_DATABASE_URL` (they
//! skip without); the configuration tests always run.
#![cfg(feature = "secrets-pg")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use orch_core::UserId;
use orch_ports::{SecretMaterial, SecretServer, SecretStore as _, UserToolServerId};
use orch_secrets_pg::{KeyBytes, KeyId, Keyring, PgSecretStore};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../crates/store-postgres/tests/support/mod.rs"]
mod pgdb;

const BIN: &str = env!("CARGO_BIN_EXE_orchestrator");

const K0: &str = "9999999999999999999999999999999999999999999999999999999999999999";
const K1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const K2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("orch-secrets-{}", uuid::Uuid::now_v7().simple()));
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

/// The database of a test schema, as a URL the binary connects to.
fn database_url_of(db: &pgdb::TestDb) -> String {
    db.pinned_url()
}

fn key(hex: &str) -> KeyBytes {
    KeyBytes::from_hex(hex).unwrap()
}

fn ring(current: &str, keys: &[(&str, &str)]) -> Keyring {
    Keyring::new(
        KeyId::parse(current).unwrap(),
        keys.iter()
            .map(|(id, hex)| (KeyId::parse(id).unwrap(), key(hex))),
    )
    .unwrap()
}

/// What the Job has: a configuration file, the key files it names, and the database in a variable.
/// It names an agents file and a thread-tools key that are not there: rewrap must not read them.
fn config(scratch: &Scratch, current: &str, with_k2: bool) -> PathBuf {
    scratch.write("k0", &format!("{K0}\n"));
    scratch.write("k1", &format!("{K1}\n"));
    if with_k2 {
        scratch.write("k2", &format!("{K2}\n"));
    }
    let k2 = if with_k2 {
        "      - { id: k2, key: { file: k2 } }\n"
    } else {
        ""
    };
    scratch.write(
        "config.yaml",
        &format!(
            "version: 1
database: {{ url: {{ env: DATABASE_URL }} }}
agents: {{ file: /nowhere/agents.yaml }}
threadTools: {{ url: 'http://orchestrator:8080', secret: {{ file: /nowhere/thread-tools }} }}
log: {{ format: text }}
secrets:
  store: pg
  pg:
    keys:
      - {{ id: k1, key: {{ file: k1 }} }}
{k2}    current: {current}
"
        ),
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

fn owner(n: u32) -> UserId {
    UserId::new(&format!("user{n}@example.test"))
}

fn server(n: u32) -> SecretServer {
    SecretServer::credential(&UserToolServerId::parse(&format!("u-{n:010}")).unwrap())
}

fn material(text: &str) -> SecretMaterial {
    SecretMaterial::new(text.as_bytes()).unwrap()
}

/// The one line of JSON the command prints on stdout.
fn report(output: &Output) -> serde_json::Value {
    let out = text(&output.stdout);
    assert_eq!(out.lines().count(), 1, "one line on stdout: {out:?}");
    serde_json::from_str(out.trim()).unwrap()
}

/// A store over the test schema with `keys`, migrated.
async fn store(db: &pgdb::TestDb, keys: Keyring) -> PgSecretStore {
    let store = PgSecretStore::connect_with(&database_url_of(db), keys, 2)
        .await
        .unwrap();
    PgSecretStore::migrate_direct(&db.pinned_direct_url())
        .await
        .unwrap();
    store
}

#[tokio::test]
async fn it_re_seals_every_secret_under_the_current_key_and_exits_zero() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let old = store(&db, ring("k1", &[("k1", K1)])).await;
    let mut kept = Vec::new();
    for n in 0..4 {
        let (owner, server) = (owner(n % 2), server(n));
        let version = old
            .put(&owner, &server, &material(&format!("secret {n}")))
            .await
            .unwrap();
        kept.push((owner, server, version, format!("secret {n}")));
    }

    let config = config(&scratch, "k2", true);
    let url = database_url_of(&db);
    let env = [("DATABASE_URL", url.as_str())];
    let first = run(&["--config", path_str(&config), "secrets", "rewrap"], &env);
    let (out, err) = (text(&first.stdout), text(&first.stderr));
    assert_eq!(code(&first), 0, "stdout: {out}\nstderr: {err}");
    let json = report(&first);
    assert_eq!(json["complete"], true);
    assert_eq!(json["currentKey"], "k2");
    assert_eq!(json["rewrapped"], 4);
    assert_eq!(json["alreadyCurrent"], 0);
    assert_eq!(json["failed"], 0);
    // no key and no secret in either stream
    for stream in [&out, &err] {
        for needle in [K1, K2, "secret 0"] {
            assert!(!stream.contains(needle), "{needle} is in:\n{stream}");
        }
    }

    // every secret reads under k2 alone, at the version it had
    let new = store(&db, ring("k2", &[("k2", K2)])).await;
    for (owner, server, version, text) in &kept {
        let (got_version, got) = new.get(owner, server).await.unwrap().unwrap();
        assert_eq!((got_version, got.expose()), (*version, text.as_bytes()));
    }

    // and a second run finds nothing to do
    let second = run(&["--config", path_str(&config), "secrets", "rewrap"], &env);
    assert_eq!(code(&second), 0);
    let json = report(&second);
    assert_eq!(
        (json["rewrapped"].as_u64(), json["alreadyCurrent"].as_u64()),
        (Some(0), Some(4))
    );
}

#[tokio::test]
async fn it_applies_its_migrations_to_a_database_that_has_none() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    let config = config(&scratch, "k1", false);
    let url = database_url_of(&db);
    let output = run(
        &["--config", path_str(&config), "secrets", "rewrap"],
        &[("DATABASE_URL", url.as_str())],
    );
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    assert_eq!(report(&output)["rewrapped"], 0);
    // the table is there for the service that starts next
    let store = store(&db, ring("k1", &[("k1", K1)])).await;
    assert!(store.get(&owner(0), &server(0)).await.unwrap().is_none());
}

#[tokio::test]
async fn a_secret_under_a_key_the_file_does_not_list_exits_65_and_is_left_as_it_was() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    // k0 was removed from the file before its secrets were re-sealed
    let lost = store(&db, ring("k0", &[("k0", K0)])).await;
    lost.put(&owner(1), &server(1), &material("under k0"))
        .await
        .unwrap();
    let held = store(&db, ring("k1", &[("k1", K1)])).await;
    held.put(&owner(2), &server(2), &material("under k1"))
        .await
        .unwrap();

    let config = config(&scratch, "k2", true);
    let url = database_url_of(&db);
    let output = run(
        &["--config", path_str(&config), "secrets", "rewrap"],
        &[("DATABASE_URL", url.as_str())],
    );
    assert_eq!(code(&output), 65, "{}", text(&output.stderr));
    let json = report(&output);
    assert_eq!(json["complete"], false);
    assert_eq!(json["rewrapped"], 1);
    assert_eq!(json["failed"], 1);
    assert_eq!(
        json["failures"],
        serde_json::json!([{"server": server(1).to_string(), "kid": "k0", "kind": "keyUnknown"}])
    );
    assert!(
        !text(&output.stdout).contains("example.test"),
        "no person in the report"
    );
    // the one it could open is under k2 now, the other is untouched and reads once k0 is back
    let all = store(&db, ring("k2", &[("k0", K0), ("k2", K2)])).await;
    assert_eq!(
        all.get(&owner(1), &server(1))
            .await
            .unwrap()
            .unwrap()
            .1
            .expose(),
        b"under k0"
    );
    assert_eq!(
        all.get(&owner(2), &server(2))
            .await
            .unwrap()
            .unwrap()
            .1
            .expose(),
        b"under k1"
    );
}

#[test]
fn a_file_without_the_section_exits_78_naming_it() {
    let scratch = Scratch::new();
    let config = scratch.write(
        "config.yaml",
        "version: 1\ndatabase: { url: { env: DATABASE_URL } }\nagents: { file: a.yaml }\n",
    );
    let output = run(
        &["--config", path_str(&config), "secrets", "rewrap"],
        &[("DATABASE_URL", "postgres://u:hunter2@127.0.0.1:1/x")],
    );
    assert_eq!(code(&output), 78);
    let err = text(&output.stderr);
    assert!(err.contains("secrets: required"), "{err}");
    assert!(!err.contains("hunter2"), "{err}");
    assert!(output.stdout.is_empty());
}

#[test]
fn no_configuration_file_exits_78_and_says_how_to_give_one() {
    let output = run(&["secrets", "rewrap"], &[]);
    assert_eq!(code(&output), 78);
    let err = text(&output.stderr);
    assert!(
        err.contains("ORCH_CONFIG_FILE") && err.contains("--config"),
        "{err}"
    );
}

#[test]
fn a_key_that_cannot_be_read_exits_78_naming_its_key_and_never_a_value() {
    let scratch = Scratch::new();
    let config = config(&scratch, "k2", false); // k2 is not written, and is not listed either
    let config_text = fs::read_to_string(&config).unwrap();
    fs::write(&config, config_text.replace("file: k1", "file: nowhere")).unwrap();
    let output = run(
        &["--config", path_str(&config), "secrets", "rewrap"],
        &[("DATABASE_URL", "postgres://u:hunter2@127.0.0.1:1/x")],
    );
    assert_eq!(code(&output), 78);
    let err = text(&output.stderr);
    assert!(err.contains("secrets.pg.keys[0].key"), "{err}");
    assert!(err.contains("secrets.pg.current"), "{err}");
    assert!(!err.contains("hunter2") && !err.contains(K1), "{err}");
}

#[test]
fn a_database_that_cannot_be_reached_exits_69() {
    let scratch = Scratch::new();
    let config = config(&scratch, "k2", true);
    // nothing listens on port 1
    let output = run(
        &["--config", path_str(&config), "secrets", "rewrap"],
        &[("DATABASE_URL", "postgres://u:hunter2@127.0.0.1:1/x")],
    );
    assert_eq!(code(&output), 69, "{}", text(&output.stderr));
    let (out, err) = (text(&output.stdout), text(&output.stderr));
    assert!(out.is_empty(), "no report without a run: {out}");
    assert!(
        !err.contains("hunter2") && !err.contains("127.0.0.1"),
        "{err}"
    );
}

#[test]
fn a_flag_of_the_service_is_a_usage_error() {
    let scratch = Scratch::new();
    let config = config(&scratch, "k2", true);
    let output = run(
        &[
            "--config",
            path_str(&config),
            "--role",
            "worker",
            "secrets",
            "rewrap",
        ],
        &[],
    );
    assert_eq!(code(&output), 2);
    let err = text(&output.stderr);
    assert!(
        err.contains("--role") && err.contains("secrets rewrap"),
        "{err}"
    );
    // and the service's --print-config takes no command
    let output = run(&["--print-config", "secrets", "rewrap"], &[]);
    assert_eq!(code(&output), 2);
}

#[test]
fn the_service_reads_the_section_and_prints_references_and_no_key() {
    let scratch = Scratch::new();
    let config = config(&scratch, "k2", true);
    scratch.write(
        "agents.yaml",
        "- { id: a, name: A, cardUrl: 'https://a.example.com/c', tokenEnv: A_TOKEN }\n",
    );
    // the whole file wants an agents file and a thread-tools key: give them
    let config_text = fs::read_to_string(&config).unwrap();
    let agents = scratch.0.join("agents.yaml");
    let tools = scratch.write("thread-tools", &format!("{K0}\n"));
    fs::write(
        &config,
        config_text
            .replace("/nowhere/agents.yaml", path_str(&agents))
            .replace("/nowhere/thread-tools", path_str(&tools)),
    )
    .unwrap();
    let output = run(
        &["--config", path_str(&config), "--print-config"],
        &[
            ("DATABASE_URL", "postgres://u:hunter2@127.0.0.1:1/x"),
            ("A_TOKEN", "t"),
        ],
    );
    assert_eq!(code(&output), 0, "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    assert!(
        printed.contains("secrets:") && printed.contains("current: k2"),
        "{printed}"
    );
    assert!(
        printed.contains("file: k1"),
        "references, as written: {printed}"
    );
    for needle in [K1, K2, "hunter2"] {
        assert!(!printed.contains(needle), "{needle} is in:\n{printed}");
    }
}

/// With `database.directUrl` the command opens the database itself and never the pooler: here the
/// pooled `database.url` is an address nobody listens on, and the command still re-seals. Without the
/// key the same file cannot connect (69, the database is down).
#[tokio::test]
async fn it_connects_to_the_direct_url_when_the_file_has_one() {
    let Some(db) = pgdb::TestDb::new().await else {
        eprintln!("skipping: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let scratch = Scratch::new();
    store(&db, ring("k1", &[("k1", K1)])).await;
    let config = config(&scratch, "k1", false);
    let with_key = scratch.write(
        "with-key.yaml",
        &std::fs::read_to_string(&config).unwrap().replace(
            "database: { url: { env: DATABASE_URL } }",
            "database: { url: { env: DATABASE_URL }, directUrl: { env: DATABASE_DIRECT_URL } }",
        ),
    );
    let nobody = "postgres://nobody:pw@127.0.0.1:1/none";
    let direct = db.pinned_direct_url();
    let out = run(
        &["--config", path_str(&with_key), "secrets", "rewrap"],
        &[("DATABASE_URL", nobody), ("DATABASE_DIRECT_URL", &direct)],
    );
    assert_eq!(code(&out), 0, "stderr: {}", text(&out.stderr));
    assert_eq!(report(&out)["complete"], true);
    for stream in [text(&out.stdout), text(&out.stderr)] {
        assert!(
            !stream.contains(&direct) && !stream.contains(nobody),
            "{stream}"
        );
    }

    let out = run(
        &["--config", path_str(&config), "secrets", "rewrap"],
        &[("DATABASE_URL", nobody)],
    );
    assert_eq!(code(&out), 69, "stderr: {}", text(&out.stderr));
}
