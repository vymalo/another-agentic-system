//! `secrets` (ADR 0046, slice 3): where a person's own credentials are kept, sealed. A good file,
//! every kind of mistake listed at once and naming the key, the keys resolved from references and
//! never printed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{ConfigError, ErrorKind, SecretRef, SecretStoreKind, render};
use support::{Fake, lines, load};

const HEAD: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

/// Three keys, as `openssl rand -hex 32` writes them.
const K1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const K2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
const K3: &str = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";

const PG_PREFIX: &str = "  store: pg\n  pg:\n";

fn env() -> Fake {
    Fake::default()
        .env("DATABASE_URL", "postgres://u:pw@db/orch")
        .env("K1", K1)
        // an environment variable is trimmed
        .env("K2", &format!("  {K2} "))
        // a file loses one trailing newline
        .file("/run/secrets/k3", &format!("{K3}\n"))
}

fn file(section: &str) -> String {
    format!("{HEAD}secrets:\n{section}")
}

#[test]
fn a_file_without_the_section_keeps_no_secrets_and_prints_nothing_of_it() {
    let valid = load(HEAD, &env()).unwrap();
    assert!(valid.config.secrets.is_none());
    assert!(valid.secrets.user_secret_keys.is_empty());
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!shown.contains("secrets"), "{shown}");
}

#[test]
fn a_section_names_its_keys_and_each_one_is_resolved_and_trimmed() {
    let valid = load(
        &file(&format!(
            "{PG_PREFIX}    keys:
      - {{ id: k1, key: {{ env: K1 }} }}
      - {{ id: k2, key: {{ env: K2 }} }}
      - {{ id: \"2026-10\", key: {{ file: /run/secrets/k3 }} }}
    current: k2
"
        )),
        &env(),
    )
    .unwrap();
    let section = valid.config.secrets.as_ref().unwrap();
    assert_eq!(section.store, SecretStoreKind::Pg);
    let pg = section.pg.as_ref().unwrap();
    assert_eq!(pg.current, "k2");
    assert_eq!(
        pg.keys.iter().map(|k| k.id.as_str()).collect::<Vec<_>>(),
        ["k1", "k2", "2026-10"]
    );
    assert_eq!(pg.keys[0].key, SecretRef::Env("K1".to_owned()));

    let resolved = &valid.secrets.user_secret_keys;
    assert_eq!(
        resolved
            .iter()
            .map(|k| (k.id.as_str(), k.key.expose()))
            .collect::<Vec<_>>(),
        [("k1", K1), ("k2", K2), ("2026-10", K3)],
        "trimmed, and in the order of the file"
    );
    assert_eq!(SecretStoreKind::Pg.as_str(), "pg");
    assert_eq!(SecretStoreKind::Pg.feature(), "secrets-pg");
}

#[test]
fn the_store_is_a_closed_choice_and_its_section_is_required() {
    let errors = lines(load(&file("  store: openbao\n"), &env()));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("secrets.store: "), "{errors:?}");

    assert_eq!(
        lines(load(&file("  store: pg\n"), &env())),
        ["secrets.pg: required when secrets.store is pg"]
    );
    // and the store is required with the section
    let errors = lines(load(&file("  pg: { keys: [], current: k1 }\n"), &env()));
    assert!(
        errors.iter().any(|l| l.starts_with("secrets.store: ")),
        "{errors:?}"
    );
    // an unknown key of the section
    let errors = lines(load(
        &file(&format!(
            "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: K1 }} }}]\n    current: k1\n    rotate: true\n"
        )),
        &env(),
    ));
    assert_eq!(errors, ["secrets.pg.rotate: unknown key"]);
}

#[test]
fn current_names_a_listed_key() {
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: K1 }} }}]\n    current: k2\n"
    ));
    assert_eq!(
        lines(load(&text, &env())),
        ["secrets.pg.current: names no key of secrets.pg.keys"]
    );
    // and it is required
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: K1 }} }}]\n"
    ));
    let errors = lines(load(&text, &env()));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("secrets.pg.current: "), "{errors:?}");
}

#[test]
fn ids_are_unique_and_of_a_small_alphabet() {
    let text = file(&format!(
        "{PG_PREFIX}    keys:
      - {{ id: k1, key: {{ env: K1 }} }}
      - {{ id: k1, key: {{ env: K2 }} }}
      - {{ id: \"\", key: {{ env: K2 }} }}
      - {{ id: \"k 3\", key: {{ env: K2 }} }}
      - {{ id: clé, key: {{ env: K2 }} }}
      - {{ id: {long}, key: {{ env: K2 }} }}
    current: k1
",
        long = "x".repeat(65)
    ));
    let bad = "a key id is 1 to 64 characters of A-Z, a-z, 0-9, '.', '_' and '-'";
    assert_eq!(
        lines(load(&text, &env())),
        [
            "secrets.pg.keys[1].id: is the same as secrets.pg.keys[0].id".to_owned(),
            format!("secrets.pg.keys[2].id: {bad}"),
            format!("secrets.pg.keys[3].id: {bad}"),
            format!("secrets.pg.keys[4].id: {bad}"),
            format!("secrets.pg.keys[5].id: {bad}"),
        ]
    );
}

#[test]
fn a_key_is_64_hex_characters_and_no_message_quotes_one() {
    const SECRET: &str = "S3CR3T-KEY-THAT-MUST-NOT-LEAK-0123456789";
    let wrong = [
        K1[..63].to_owned(),
        format!("{K1}0"),
        format!("{}g", &K1[..63]),
        // 32 characters of text are 32 bytes, and not a key written as hex
        "0123456789abcdef0123456789abcdef".to_owned(),
        format!("0x{}", &K1[..62]),
        SECRET.to_owned(),
        // a base64 key
        "q83vEjRWeJCrze8SNFZ4kKvN7xI0VniQq83vEjRWeJA=".to_owned(),
    ];
    for value in wrong {
        let text = file(&format!(
            "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: BAD }} }}]\n    current: k1\n"
        ));
        let errors = load(&text, &env().env("BAD", &value)).unwrap_err();
        assert_eq!(
            errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "secrets.pg.keys[0].key: the secret is not 64 hexadecimal characters (32 bytes): \
              generate one with `openssl rand -hex 32`"
            ],
            "{value}"
        );
        let shown = format!("{} {errors:?}", render(&errors));
        assert!(
            !shown.contains(&value[..value.len().min(12)]),
            "a value reached an error: {shown}"
        );
    }
    // a key that does not resolve is named by its reference, never its value
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: UNSET }} }}, {{ id: k2, key: {{ file: /nope }} }}]\n    current: k1\n"
    ));
    assert_eq!(
        lines(load(&text, &env())),
        [
            "secrets.pg.keys[0].key: the environment variable UNSET is unset or empty",
            "secrets.pg.keys[1].key: the file /nope cannot be read",
        ]
    );
}

#[test]
fn a_key_of_all_zeros_is_refused_by_what_it_is_and_one_with_a_digit_is_not() {
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: BAD }} }}]\n    current: k1\n"
    ));
    for value in ["0".repeat(64), format!(" {}\n", "0".repeat(64))] {
        let errors = load(&text, &env().env("BAD", &value)).unwrap_err();
        assert_eq!(
            errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "secrets.pg.keys[0].key: the secret is all zero bytes, which is no key: \
                 generate one with `openssl rand -hex 32`"
            ],
            "{value:?}"
        );
        let shown = format!("{} {errors:?}", render(&errors));
        assert!(
            !shown.contains("00000000"),
            "the value is in an error: {shown}"
        );
    }
    // 63 zeros and a one is a key; 64 zeros of another length is still the shape error
    let one = format!("{}1", "0".repeat(63));
    assert!(load(&text, &env().env("BAD", &one)).is_ok());
    let errors = load(&text, &env().env("BAD", &"0".repeat(63))).unwrap_err();
    assert_eq!(
        errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
        [
            "secrets.pg.keys[0].key: the secret is not 64 hexadecimal characters (32 bytes): \
             generate one with `openssl rand -hex 32`"
        ]
    );
}

#[test]
fn a_plain_string_where_a_key_goes_is_refused_as_not_a_reference() {
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {K1} }}]\n    current: k1\n"
    ));
    let errors = lines(load(&text, &env()));
    assert_eq!(
        errors,
        ["secrets.pg.keys[0].key: a secret is a reference: `{ env: NAME }` or `{ file: PATH }`"]
    );
    assert!(!render(&load(&text, &env()).unwrap_err()).contains(K1));
}

#[test]
fn there_are_one_to_sixteen_keys() {
    let none = file(&format!("{PG_PREFIX}    keys: []\n    current: k1\n"));
    assert_eq!(
        lines(load(&none, &env())),
        ["secrets.pg.keys: needs at least 1 item(s)"]
    );
    let many: String = (0..17)
        .map(|n| format!("      - {{ id: k{n}, key: {{ env: K1 }} }}\n"))
        .collect();
    let text = file(&format!("{PG_PREFIX}    keys:\n{many}    current: k0\n"));
    assert_eq!(
        lines(load(&text, &env())),
        ["secrets.pg.keys: takes at most 16 item(s)"]
    );
}

#[test]
fn debug_and_the_printed_configuration_show_references_and_never_a_key() {
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: K1 }} }}, {{ id: k2, key: {{ file: /run/secrets/k3 }} }}]\n    current: k2\n"
    ));
    let valid = load(&text, &env()).unwrap();
    let shown = format!("{valid:?} {:?}", valid.secrets);
    assert!(!shown.contains(K1) && !shown.contains(K3), "{shown}");
    assert!(
        shown.contains("{ env: K1 }") && shown.contains("{ file: /run/secrets/k3 }"),
        "{shown}"
    );
    let printed = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!printed.contains(K1) && !printed.contains(K3), "{printed}");
    assert!(
        printed.contains("env: K1")
            && printed.contains("id: k2")
            && printed.contains("current: k2"),
        "{printed}"
    );
}

#[test]
fn every_error_of_the_section_is_listed_at_once_with_the_other_sections() {
    let text = format!(
        "{HEAD}secrets:
  store: pg
  pg:
    keys:
      - {{ id: k1, key: {{ env: UNSET }} }}
      - {{ id: k1, key: {{ env: K1 }} }}
    current: k9
threadTools: {{ url: 'http://o:8080' }}
"
    );
    let errors = lines(load(&text, &env()));
    assert_eq!(
        errors,
        [
            "secrets.pg.current: names no key of secrets.pg.keys",
            "secrets.pg.keys[0].key: the environment variable UNSET is unset or empty",
            "secrets.pg.keys[1].id: is the same as secrets.pg.keys[0].id",
            "threadTools.secret: required with threadTools.url: set both or neither",
        ]
    );
}

#[test]
fn the_error_kinds_are_the_ones_the_rest_of_the_file_uses() {
    let text = file(&format!(
        "{PG_PREFIX}    keys: [{{ id: k1, key: {{ env: UNSET }} }}]\n    current: k1\n"
    ));
    let errors = load(&text, &env()).unwrap_err();
    assert!(
        matches!(errors[0].kind, ErrorKind::Unresolved(_)),
        "{errors:?}"
    );
}

/// `orchestrator secrets rewrap` needs the database and the section, and nothing else: a Job
/// that re-seals rows is not given the thread-tools key, a model key or the agents file.
mod the_section_alone {
    use orch_config::ValidatedUserSecrets;

    use super::*;

    const WHOLE: &str = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: /nowhere/agents.yaml }
threadTools:
  url: http://orch:8080
  secret: { file: /run/secrets/orchestrator/thread-tools-secret }
models: { endpoints: { default: { baseUrl: https://m.example.com/v1, apiKey: { env: MODEL_KEY } } } }
log: { format: text }
secrets:
  store: pg
  pg:
    keys:
      - { id: k1, key: { env: K1 } }
      - { id: k2, key: { env: K2 } }
    current: k2
";

    fn only_the_section(text: &str, fake: &Fake) -> Result<ValidatedUserSecrets, Vec<ConfigError>> {
        let tree = orch_config::parse_yaml(text).unwrap();
        orch_config::check(&tree)
            .unwrap()
            .validate_user_secrets(std::path::Path::new("/etc/orchestrator"), fake)
    }

    #[test]
    fn the_thread_tools_key_and_the_models_are_not_asked_for() {
        // the whole file cannot be loaded here: the thread-tools key and the model key are unset
        let errors = lines(load(WHOLE, &env()));
        assert!(
            errors.iter().any(|l| l.starts_with("threadTools.secret: ")),
            "{errors:?}"
        );
        let valid = only_the_section(WHOLE, &env()).unwrap();
        assert_eq!(valid.database_url.expose(), "postgres://u:pw@db/orch");
        assert_eq!(valid.secrets.pg.as_ref().unwrap().current, "k2");
        assert_eq!(
            valid
                .keys
                .iter()
                .map(|k| (k.id.as_str(), k.key.expose()))
                .collect::<Vec<_>>(),
            [("k1", K1), ("k2", K2)]
        );
        assert_eq!(valid.log_format, orch_config::LogFormat::Text);
        let debug = format!("{valid:?}");
        assert!(
            !debug.contains(K1) && !debug.contains("u:pw@db") && debug.contains("{ env: K1 }"),
            "{debug}"
        );
    }

    #[test]
    fn the_rules_of_the_section_apply_and_those_of_the_rest_do_not() {
        let text = WHOLE.replace("current: k2", "current: k9");
        assert_eq!(
            only_the_section(&text, &env())
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["secrets.pg.current: names no key of secrets.pg.keys"]
        );
    }

    #[test]
    fn the_database_and_the_keys_are_named_when_they_do_not_resolve() {
        let errors: Vec<_> = only_the_section(WHOLE, &Fake::default())
            .unwrap_err()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            errors,
            [
                "database.url: the environment variable DATABASE_URL is unset or empty",
                "secrets.pg.keys[0].key: the environment variable K1 is unset or empty",
                "secrets.pg.keys[1].key: the environment variable K2 is unset or empty",
            ]
        );
    }

    #[test]
    fn a_file_without_the_section_is_refused_by_name() {
        let errors = lines(Err(only_the_section(HEAD, &env()).unwrap_err()));
        assert_eq!(
            errors,
            [
                "secrets: required: this command reads the secret store's keys from the configuration file"
            ]
        );
    }
}
