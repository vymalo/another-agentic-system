//! `database.directUrl` (issue 290): the connection string of the database itself, for what cannot go
//! through a transaction-mode pooler. Optional, a secret like `database.url`, and never the same text.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::path::Path;

use orch_config::{ConfigError, SecretRef};
use support::{Fake, lines, load};

const POOLED: &str = "postgres://u:pw@pooler:6432/orch";
const DIRECT: &str = "postgres://u:pw@db:5432/orch";

const HEAD: &str = "\
version: 1
agents:
  file: agents.yaml
";

fn file(database: &str) -> String {
    format!("{HEAD}database:\n{database}")
}

fn env() -> Fake {
    Fake::default()
        .env("DATABASE_URL", POOLED)
        .env("DATABASE_DIRECT_URL", DIRECT)
}

const BOTH: &str = "  url: { env: DATABASE_URL }\n  directUrl: { env: DATABASE_DIRECT_URL }\n";

#[test]
fn without_the_key_there_is_no_direct_url_and_nothing_is_printed_of_it() {
    let valid = load(&file("  url: { env: DATABASE_URL }\n"), &env()).unwrap();
    assert!(valid.config.database.direct_url.is_none());
    assert!(valid.secrets.database_direct_url.is_none());
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!shown.contains("directUrl"), "{shown}");
}

#[test]
fn the_key_is_a_secret_reference_resolved_like_the_url() {
    let valid = load(&file(BOTH), &env()).unwrap();
    assert_eq!(valid.secrets.database_url.expose(), POOLED);
    let direct = valid.secrets.database_direct_url.as_ref().unwrap();
    assert_eq!(direct.expose(), DIRECT);
    assert_eq!(
        direct.reference(),
        &SecretRef::Env("DATABASE_DIRECT_URL".to_owned())
    );
    // the effective document prints the reference, never the value
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(shown.contains("directUrl"), "{shown}");
    assert!(shown.contains("DATABASE_DIRECT_URL"), "{shown}");
    assert!(
        !shown.contains("db:5432") && !shown.contains("pw"),
        "{shown}"
    );
    let debug = format!("{:?}", valid.secrets);
    assert!(
        !debug.contains("db:5432") && !debug.contains("pw"),
        "{debug}"
    );
}

#[test]
fn a_file_reference_is_read_and_its_newline_cut() {
    let fake = Fake::default()
        .env("DATABASE_URL", POOLED)
        .file("/run/secrets/direct", &format!("{DIRECT}\n"));
    let valid = load(
        &file("  url: { env: DATABASE_URL }\n  directUrl: { file: /run/secrets/direct }\n"),
        &fake,
    )
    .unwrap();
    assert_eq!(valid.secrets.database_direct_url.unwrap().expose(), DIRECT);
}

#[test]
fn a_plain_string_is_refused_like_every_secret() {
    let errors = lines(load(
        &file("  url: { env: DATABASE_URL }\n  directUrl: postgres://u:hunter2@db/orch\n"),
        &env(),
    ));
    assert_eq!(
        errors,
        ["database.directUrl: a secret is a reference: `{ env: NAME }` or `{ file: PATH }`"]
    );
}

#[test]
fn a_reference_that_does_not_resolve_is_named_and_the_file_is_refused() {
    let fake = Fake::default().env("DATABASE_URL", POOLED);
    let errors = lines(load(&file(BOTH), &fake));
    assert_eq!(
        errors,
        ["database.directUrl: the environment variable DATABASE_DIRECT_URL is unset or empty"]
    );
}

#[test]
fn a_direct_url_equal_to_the_url_is_refused_naming_the_key_and_no_value() {
    // two names, one text: a mistake the process would otherwise hide (every migration would go
    // through the pooler it was meant to avoid)
    let fake = Fake::default()
        .env("DATABASE_URL", POOLED)
        .env("DATABASE_DIRECT_URL", &format!(" {POOLED} "));
    let errors = lines(load(&file(BOTH), &fake));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("database.directUrl: is the same as database.url"),
        "{errors:?}"
    );
    assert!(
        !errors[0].contains("pooler") || !errors[0].contains("pw"),
        "{errors:?}"
    );
    assert!(!errors[0].contains(POOLED), "{errors:?}");
}

#[test]
fn the_commands_that_open_the_database_get_the_key_and_the_same_rule() {
    let text = file(BOTH);
    let tree = orch_config::parse_yaml(&text).unwrap();
    let base = Path::new("/etc/orchestrator");

    let valid = orch_config::check(&tree)
        .unwrap()
        .validate_database(base, &env())
        .unwrap();
    assert_eq!(valid.database_direct_url.unwrap().expose(), DIRECT);

    let same = Fake::default()
        .env("DATABASE_URL", POOLED)
        .env("DATABASE_DIRECT_URL", POOLED);
    let errors: Vec<String> = orch_config::check(&tree)
        .unwrap()
        .validate_database(base, &same)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("database.directUrl: is the same as database.url"));

    let unset = Fake::default().env("DATABASE_URL", POOLED);
    let errors: Vec<ConfigError> = orch_config::check(&tree)
        .unwrap()
        .validate_database(base, &unset)
        .unwrap_err();
    assert_eq!(
        errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["database.directUrl: the environment variable DATABASE_DIRECT_URL is unset or empty"]
    );
}

#[test]
fn the_rewrap_command_gets_the_key_too() {
    const K: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let text = format!(
        "{}secrets:\n  store: pg\n  pg:\n    keys:\n      - {{ id: k1, key: {{ env: K1 }} }}\n    current: k1\n",
        file(BOTH)
    );
    let tree = orch_config::parse_yaml(&text).unwrap();
    let fake = env().env("K1", K);
    let valid = orch_config::check(&tree)
        .unwrap()
        .validate_user_secrets(Path::new("/etc/orchestrator"), &fake)
        .unwrap();
    assert_eq!(valid.database_direct_url.unwrap().expose(), DIRECT);

    let same = Fake::default()
        .env("DATABASE_URL", POOLED)
        .env("DATABASE_DIRECT_URL", POOLED)
        .env("K1", K);
    let errors: Vec<String> = orch_config::check(&tree)
        .unwrap()
        .validate_user_secrets(Path::new("/etc/orchestrator"), &same)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("database.directUrl: is the same as database.url"));
}
