//! `Config::validate_database` (ADR 0065): `orchestrator usage backfill` needs the database and
//! nothing else, so the Job that runs it is not given the agents file, the thread-tools key or a
//! model key.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{ConfigError, LogFormat, ValidatedDatabase};
use support::Fake;

const WHOLE: &str = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: /nowhere/agents.yaml }
threadTools:
  url: http://orch:8080
  secret: { file: /run/secrets/orchestrator/thread-tools-secret }
models: { endpoints: { default: { baseUrl: https://m.example.com/v1, apiKey: { env: MODEL_KEY } } } }
log: { format: text }
";

fn only_the_database(text: &str, fake: &Fake) -> Result<ValidatedDatabase, Vec<ConfigError>> {
    let tree = orch_config::parse_yaml(text).unwrap();
    orch_config::check(&tree)
        .unwrap()
        .validate_database(std::path::Path::new("/etc/orchestrator"), fake)
}

#[test]
fn the_thread_tools_key_the_models_and_the_agents_are_not_asked_for() {
    let fake = Fake::default().env("DATABASE_URL", "postgres://u:pw@db/orch");
    let valid = only_the_database(WHOLE, &fake).unwrap();
    assert_eq!(valid.database_url.expose(), "postgres://u:pw@db/orch");
    assert_eq!(valid.log_format, LogFormat::Text);
    let debug = format!("{valid:?}");
    assert!(
        !debug.contains("u:pw@db") && debug.contains("{ env: DATABASE_URL }"),
        "{debug}"
    );
}

#[test]
fn a_database_that_does_not_resolve_is_named() {
    let errors: Vec<String> = only_the_database(WHOLE, &Fake::default())
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        errors,
        ["database.url: the environment variable DATABASE_URL is unset or empty"]
    );
}
