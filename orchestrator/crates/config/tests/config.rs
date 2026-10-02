//! Good files, and every kind of error: listed at once, naming the key and never a value.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{ErrorKind, Role, SecretRef, Surface, render};
use support::{Fake, LONG, lines, load};

/// A file with only what is required.
const MINIMAL: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

fn minimal_env() -> Fake {
    Fake::default().env("DATABASE_URL", "postgres://u:pw@db/orch")
}

/// The example of `docs/api/config.md`, so the document cannot drift from the types.
fn documented_example() -> String {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/config.md"),
    )
    .unwrap();
    let after = doc.split("## An example").nth(1).unwrap();
    let block = after.split("```yaml\n").nth(1).unwrap();
    block.split("```").next().unwrap().to_owned()
}

#[test]
fn a_minimal_file_is_valid_and_every_default_is_filled_in() {
    let valid = load(MINIMAL, &minimal_env()).unwrap();
    let c = &valid.config;
    assert_eq!(c.server.listen, "0.0.0.0:8080");
    assert_eq!(c.server.role, Role::All);
    assert_eq!(c.server.shutdown_grace_secs, 15);
    assert_eq!(c.database.max_connections, 10);
    assert_eq!(c.dispatcher.concurrency, 32);
    assert_eq!(c.dispatcher.outbox_lease_secs, 30);
    assert_eq!(c.inbox.lease_secs, 30);
    assert_eq!(c.inbox.poll_secs, 2);
    assert_eq!(c.inbox.parked_ttl_secs, 86_400);
    assert_eq!(c.inbox.max_attempts, 10);
    assert_eq!(c.gate.max_attempts_cap, 10);
    assert_eq!(c.gate.verifier_timeout_secs, 1800);
    assert_eq!(c.gate.verifier_watch_secs, 5);
    assert_eq!(c.gate.ci.timeout_secs, 3600);
    assert!(c.steps.record_tool_io);
    assert_eq!(c.thread_tools.token_ttl_secs, 7200);
    assert_eq!(c.mcp.wait_max_secs, 3600);
    assert_eq!(c.mcp.wait_max_concurrent, 256);
    assert_eq!(c.mcp.wait_max_per_user, 16);
    assert!(c.tasks.title.is_none(), "no task, titles are off");
    assert!(c.tasks.description.is_none(), "no task, no descriptions");
    assert!(
        c.ui.show_descriptions,
        "the web shows descriptions by default"
    );
    assert_eq!(valid.prompts, orch_config::Prompts::default());
    assert_eq!(
        valid.secrets.database_url.expose(),
        "postgres://u:pw@db/orch"
    );
    assert_eq!(
        valid.secrets.database_url.reference(),
        &SecretRef::Env("DATABASE_URL".to_owned())
    );
    assert_eq!(
        valid.agents_file().unwrap(),
        std::path::Path::new("/etc/orchestrator/agents.yaml"),
        "a relative path is relative to the directory of the file"
    );
    // The surfaces and the attempts that depend on other keys are shown filled in.
    let shown = c.effective();
    assert_eq!(shown.server.surfaces, Some(vec![Surface::Agui]));
    assert_eq!(shown.gate.max_attempts, Some(3));
}

#[test]
fn a_cap_below_the_default_lowers_the_default_attempts() {
    let text = format!("{MINIMAL}gate:\n  maxAttemptsCap: 2\n");
    let valid = load(&text, &minimal_env()).unwrap();
    assert_eq!(valid.config.effective().gate.max_attempts, Some(2));
}

#[test]
fn the_example_of_the_contract_is_valid() {
    let fake = Fake::default()
        .env("DATABASE_URL", "postgres://u:pw@db/orch")
        .env("ORCH_MODEL_API_KEY", "model-key")
        .env("WEBHOOK_GENERIC_SECRET", LONG)
        .env("WEBHOOK_GITHUB_SECRET", LONG)
        .file("/run/secrets/registry-agent-token", "agent-token\n")
        .file("/run/secrets/thread-tools", &format!("{LONG}\n"));
    let valid = load(&documented_example(), &fake).unwrap_or_else(|e| panic!("{}", render(&e)));
    let c = &valid.config;
    assert_eq!(
        c.server.surfaces.as_ref().unwrap().len(),
        5,
        "all five surfaces"
    );
    assert_eq!(
        c.server.public_url.as_deref(),
        Some("https://chat.example.com")
    );
    assert_eq!(c.gate.ci.required, ["build"]);
    let title = c.tasks.title.as_ref().unwrap();
    assert_eq!(
        (title.endpoint.as_str(), title.model.as_str()),
        ("small", "small-model")
    );
    let description = c.tasks.description.as_ref().unwrap();
    assert_eq!(description.endpoint, "default");
    assert_eq!(description.recompute.min_new_messages, 4);
    assert_eq!(
        valid.prompts.description.as_deref(),
        Some("Say in one or two sentences what the person wants and where it stands.")
    );
    assert!(c.ui.show_descriptions);
    assert_eq!(
        c.models.endpoints.keys().collect::<Vec<_>>(),
        ["default", "small"]
    );
    assert_eq!(
        valid.secrets.model_api_keys["default"].expose(),
        "model-key"
    );
    // A `{ file }` loses one trailing newline.
    assert_eq!(
        valid
            .secrets
            .registry_agent_token
            .as_ref()
            .unwrap()
            .expose(),
        "agent-token"
    );
    assert_eq!(valid.secrets.webhook_generic.len(), 1);
    assert_eq!(valid.secrets.webhook_github.len(), 1);
}

#[test]
fn anchors_and_merge_keys_are_resolved_before_validation() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
inbox: &timing
  leaseSecs: 9
  pollSecs: 4
dispatcher:
  <<: { concurrency: 5 }
  outboxLeaseSecs: 7
";
    let valid = load(text, &minimal_env()).unwrap();
    assert_eq!(valid.config.inbox.lease_secs, 9);
    assert_eq!(valid.config.dispatcher.concurrency, 5);
    assert_eq!(valid.config.dispatcher.outbox_lease_secs, 7);
}

#[test]
fn a_syntax_error_is_reported_alone_with_its_place() {
    let errors = lines(load("version: 1\ndatabase: [\n", &minimal_env()));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("the YAML cannot be read (line "),
        "{errors:?}"
    );
}

#[test]
fn a_repeated_key_is_an_error_never_the_last_one_winning() {
    let text = format!("{MINIMAL}server:\n  listen: 0.0.0.0:1\n  listen: 0.0.0.0:2\n");
    let errors = load(&text, &minimal_env()).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(errors[0].kind, ErrorKind::DuplicateKey { .. }));
    assert_eq!(errors[0].path, "server");
    assert!(errors[0].to_string().contains("`listen`"), "{}", errors[0]);
    assert!(
        !errors[0].to_string().contains("0.0.0.0:"),
        "a value is not repeated back"
    );
}

#[test]
fn a_tag_and_a_key_that_is_not_a_string_are_errors_at_their_path() {
    let text = "\
version: 1
database: { url: !secret DATABASE_URL }
agents: { file: agents.yaml, 5: x }
";
    let errors = load(text, &minimal_env()).unwrap_err();
    let kinds: Vec<_> = errors.iter().map(|e| (e.path.as_str(), &e.kind)).collect();
    assert!(
        kinds.contains(&("database.url", &ErrorKind::Tag)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&("agents", &ErrorKind::NonStringKey)),
        "{kinds:?}"
    );
}

#[test]
fn the_document_must_be_a_mapping_with_a_version() {
    assert_eq!(
        lines(load("- a\n- b\n", &minimal_env())),
        ["the configuration must be a mapping, starting with `version: 1`"]
    );
    for text in [
        "database: { url: { env: DATABASE_URL } }\n",
        "version: 2\ndatabase: { url: { env: DATABASE_URL } }\n",
        "version: one\ndatabase: { url: { env: DATABASE_URL } }\n",
    ] {
        let errors = lines(load(text, &minimal_env()));
        assert_eq!(
            errors,
            ["version: this build reads version 1 (write `version: 1`)"],
            "{text}"
        );
    }
}

/// Every shape error of a file is listed, each with its key path, in one run.
#[test]
fn every_shape_error_is_listed_at_once() {
    let text = "\
version: 1
server:
  listen: 0.0.0.0:8080
  role: boss
  surfaces: [agui, chat-api]
  nonsense: true
log: { format: yaml }
database:
  maxConnections: 1
dispatcher: { concurrency: many, outboxLeaseSecs: 2 }
agents:
  registry: { timeoutSecs: 61 }
gate: { require: [ci, magic], maxAttemptsCap: 101 }
webhooks:
  generic: { secrets: [] }
  github: { secrets: [{ env: A }, { env: B }, { env: C }] }
threadTools: { secret: plain-text }
auth: { devUser: 5 }
";
    let errors = lines(load(text, &minimal_env()));
    let want = [
        "agents.registry.timeoutSecs: must be at most 60",
        "agents.registry.url: required key is missing",
        "auth.devUser: expected string",
        "database.maxConnections: must be at least 2",
        "database.url: required key is missing",
        "dispatcher.concurrency: expected integer",
        "dispatcher.outboxLeaseSecs: must be at least 3",
        "gate.maxAttemptsCap: must be at most 100",
        "gate.require[1]: not an allowed value (allowed: ci, agent-checks, verifier)",
        "log.format: not an allowed value (allowed: json, text)",
        "server.nonsense: unknown key",
        "server.role: not an allowed value (allowed: all, control-plane, worker)",
        "server.surfaces[1]: not an allowed value (allowed: agui, mcp, thread-tools, webhook-generic, webhook-github)",
        "threadTools.secret: a secret is a reference: `{ env: NAME }` or `{ file: PATH }`",
        "webhooks.generic.secrets: needs at least 1 item(s)",
        "webhooks.github.secrets: takes at most 2 item(s)",
    ];
    assert_eq!(errors, want);
}

#[test]
fn a_plain_string_is_refused_wherever_a_secret_goes() {
    // One file with a string at each of the ten secrets.
    let text = "\
version: 1
database: { url: postgres://u:hunter2@db/orch }
agents:
  file: agents.yaml
  registry: { url: https://r.example.com/agents, token: plain, agentToken: plain }
models:
  endpoints: { default: { baseUrl: https://m.example.com/v1, apiKey: plain } }
threadTools: { url: http://orch:8080, secret: plain, previousSecret: plain }
webhooks:
  generic: { secrets: [plain] }
  github: { secrets: [plain] }
artifacts:
  store: s3
  s3: { bucket: files, accessKeyId: plain, secretAccessKey: plain }
";
    let errors = lines(load(text, &minimal_env()));
    let reference = "a secret is a reference: `{ env: NAME }` or `{ file: PATH }`";
    let want = [
        "agents.registry.agentToken",
        "agents.registry.token",
        "artifacts.s3.accessKeyId",
        "artifacts.s3.secretAccessKey",
        "database.url",
        "models.endpoints.default.apiKey",
        "threadTools.previousSecret",
        "threadTools.secret",
        "webhooks.generic.secrets[0]",
        "webhooks.github.secrets[0]",
    ]
    .map(|key| format!("{key}: {reference}"));
    assert_eq!(errors, want);
    assert!(
        !render(&load(text, &minimal_env()).unwrap_err()).contains("hunter2"),
        "the value of a secret is never in the message"
    );
}

#[test]
fn a_reference_with_the_wrong_members_is_refused_as_not_a_reference() {
    for secret in [
        "{ vault: x }",
        "{ env: x, file: y }",
        "{ env: 5 }",
        "[ x ]",
        "5",
    ] {
        let text = format!("version: 1\ndatabase: {{ url: {secret} }}\nagents: {{ file: a }}\n");
        assert_eq!(
            lines(load(&text, &minimal_env())),
            ["database.url: a secret is a reference: `{ env: NAME }` or `{ file: PATH }`"],
            "{secret}"
        );
    }
}

#[test]
fn unknown_keys_are_errors_and_reserved_keys_name_what_brings_them() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml, nonsense: 1 }
tasks:
  turnSummary: {}
  stepLabel: {}
auth: { defaultRole: user, roles: {} }
artifacts: { store: fs, fs: { root: files }, maxPerJobBytes: 1, fetchHosts: [] }
";
    let errors = lines(load(text, &minimal_env()));
    let find = |key: &str| {
        errors
            .iter()
            .find(|l| l.starts_with(&format!("{key}: ")))
            .unwrap()
    };
    assert_eq!(find("agents.nonsense"), "agents.nonsense: unknown key");
    for (key, by) in [
        ("tasks.turnSummary", "no PR yet"),
        ("tasks.stepLabel", "no PR yet"),
        ("auth.defaultRole", "PR S15 (ADR 0033"),
        ("auth.roles", "PR S15 (ADR 0033"),
        ("artifacts.maxPerJobBytes", "PR S11 (ADR 0032"),
        ("artifacts.fetchHosts", "PR S11 (ADR 0032"),
    ] {
        let line = find(key);
        assert!(
            line.contains("reserved for") && line.contains(by) && line.contains("not built yet"),
            "{line}"
        );
    }
    // `artifacts` itself is built (ADR 0032): only the two keys of the ingest are reserved
    assert!(
        !errors.iter().any(|l| l.starts_with("artifacts: ")
            || l.starts_with("artifacts.store")
            || l.starts_with("artifacts.fs")),
        "{errors:?}"
    );
    // S18 built these: they are keys now, not reservations
    let built = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
models: { endpoints: { default: { baseUrl: 'https://m.example.com/v1' } } }
tasks:
  title: { endpoint: default, model: m, system: { inline: x }, maxTokens: 9, language: conversation }
  description: { endpoint: default, model: m }
ui: { showDescriptions: false }
";
    load(built, &minimal_env()).unwrap();
}

const S3_ENV: [(&str, &str); 2] = [
    ("S3_ACCESS_KEY_ID", "AKIDEXAMPLE"),
    ("S3_SECRET_ACCESS_KEY", "s3cr3t-access-key"),
];

fn s3_env() -> Fake {
    S3_ENV
        .iter()
        .fold(minimal_env(), |fake, (name, value)| fake.env(name, value))
}

/// The artifact store (ADR 0032).
mod artifacts {
    use orch_config::{ArtifactStoreKind, DEFAULT_MAX_FILE_BYTES, DEFAULT_S3_REGION};

    use super::*;

    const S3: &str = "\
artifacts:
  store: s3
  s3:
    bucket: orchestrator-files
    endpoint: https://minio.example.com:9000
    prefix: prod/files
    accessKeyId: { env: S3_ACCESS_KEY_ID }
    secretAccessKey: { env: S3_SECRET_ACCESS_KEY }
";

    #[test]
    fn no_section_is_no_store() {
        let valid = load(MINIMAL, &minimal_env()).unwrap();
        assert!(valid.config.artifacts.is_none());
        assert!(valid.artifacts_fs_root().is_none());
        assert!(valid.secrets.s3_access_key_id.is_none());
        let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
        assert!(!shown.contains("artifacts"), "{shown}");
    }

    #[test]
    fn a_directory_store_is_valid_with_its_defaults() {
        let text = format!("{MINIMAL}artifacts:\n  store: fs\n  fs: {{ root: files }}\n");
        let valid = load(&text, &minimal_env()).unwrap();
        let artifacts = valid.config.artifacts.as_ref().unwrap();
        assert_eq!(artifacts.store, ArtifactStoreKind::Fs);
        assert_eq!(artifacts.max_file_bytes, DEFAULT_MAX_FILE_BYTES);
        assert_eq!(DEFAULT_MAX_FILE_BYTES, 10 * 1024 * 1024);
        assert_eq!(
            valid.artifacts_fs_root().unwrap(),
            std::path::Path::new("/etc/orchestrator/files"),
            "relative to the directory of the file"
        );
        let absolute = format!(
            "{MINIMAL}artifacts: {{ store: fs, fs: {{ root: /var/lib/orchestrator/artifacts }}, maxFileBytes: 1 }}\n"
        );
        let valid = load(&absolute, &minimal_env()).unwrap();
        assert_eq!(
            valid.artifacts_fs_root().unwrap(),
            std::path::Path::new("/var/lib/orchestrator/artifacts")
        );
        assert_eq!(valid.config.artifacts.unwrap().max_file_bytes, 1);
        assert!(valid.secrets.s3_access_key_id.is_none());
    }

    #[test]
    fn an_s3_store_is_valid_and_its_credentials_are_read_by_reference() {
        let valid = load(&format!("{MINIMAL}{S3}"), &s3_env()).unwrap();
        let artifacts = valid.config.artifacts.as_ref().unwrap();
        assert_eq!(artifacts.store, ArtifactStoreKind::S3);
        let s3 = artifacts.s3.as_ref().unwrap();
        assert_eq!(s3.bucket, "orchestrator-files");
        assert_eq!(s3.region, DEFAULT_S3_REGION);
        assert_eq!(s3.timeout_secs, 60);
        assert_eq!(s3.prefix.as_deref(), Some("prod/files"));
        assert_eq!(
            valid.secrets.s3_access_key_id.as_ref().unwrap().expose(),
            "AKIDEXAMPLE"
        );
        assert_eq!(
            valid
                .secrets
                .s3_secret_access_key
                .as_ref()
                .unwrap()
                .reference(),
            &SecretRef::Env("S3_SECRET_ACCESS_KEY".to_owned())
        );
        assert!(valid.artifacts_fs_root().is_none());
        // `--print-config` shows references and never a value
        let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
        assert!(shown.contains("S3_SECRET_ACCESS_KEY"), "{shown}");
        assert!(
            !shown.contains("AKIDEXAMPLE") && !shown.contains("s3cr3t"),
            "{shown}"
        );
        let debug = format!("{valid:?}");
        assert!(
            !debug.contains("AKIDEXAMPLE") && !debug.contains("s3cr3t"),
            "{debug}"
        );
    }

    #[test]
    fn credentials_can_be_read_from_files_and_cannot_be_unset() {
        let text = format!("{MINIMAL}{S3}")
            .replace("{ env: S3_SECRET_ACCESS_KEY }", "{ file: /run/secrets/s3 }");
        let fake = Fake::default()
            .env("DATABASE_URL", "postgres://u:pw@db/orch")
            .env("S3_ACCESS_KEY_ID", "AKIDEXAMPLE")
            .file("/run/secrets/s3", "from-a-file\n");
        let valid = load(&text, &fake).unwrap();
        assert_eq!(
            valid.secrets.s3_secret_access_key.unwrap().expose(),
            "from-a-file"
        );
        // a variable that is not set is exit 78, naming the key and the variable
        let errors = lines(load(&format!("{MINIMAL}{S3}"), &minimal_env()));
        assert_eq!(
            errors,
            [
                "artifacts.s3.accessKeyId: the environment variable S3_ACCESS_KEY_ID is unset or empty",
                "artifacts.s3.secretAccessKey: the environment variable S3_SECRET_ACCESS_KEY is unset or empty",
            ]
        );
    }

    #[test]
    fn s3_without_a_bucket_or_a_section_is_refused() {
        let no_bucket = "\
artifacts:
  store: s3
  s3: { accessKeyId: { env: S3_ACCESS_KEY_ID }, secretAccessKey: { env: S3_SECRET_ACCESS_KEY } }
";
        assert_eq!(
            lines(load(&format!("{MINIMAL}{no_bucket}"), &s3_env())),
            ["artifacts.s3.bucket: required key is missing"]
        );
        let no_section = "artifacts: { store: s3 }\n";
        assert_eq!(
            lines(load(&format!("{MINIMAL}{no_section}"), &s3_env())),
            ["artifacts.s3: required when artifacts.store is s3"]
        );
        let no_credentials = "artifacts: { store: s3, s3: { bucket: files } }\n";
        assert_eq!(
            lines(load(&format!("{MINIMAL}{no_credentials}"), &s3_env())),
            [
                "artifacts.s3.accessKeyId: required key is missing",
                "artifacts.s3.secretAccessKey: required key is missing",
            ]
        );
    }

    #[test]
    fn a_store_needs_its_section_and_refuses_the_other_ones() {
        let cases = [
            (
                "artifacts: { store: fs }",
                vec!["artifacts.fs: required when artifacts.store is fs"],
            ),
            (
                "artifacts: { store: fs, fs: { root: x }, s3: { bucket: files, accessKeyId: { env: S3_ACCESS_KEY_ID }, secretAccessKey: { env: S3_SECRET_ACCESS_KEY } } }",
                vec!["artifacts.s3: only with artifacts.store: s3; this file's store is fs"],
            ),
            (
                "artifacts: { store: s3, fs: { root: x }, s3: { bucket: files, accessKeyId: { env: S3_ACCESS_KEY_ID }, secretAccessKey: { env: S3_SECRET_ACCESS_KEY } } }",
                vec!["artifacts.fs: only with artifacts.store: fs; this file's store is s3"],
            ),
            (
                "artifacts: { fs: { root: x } }",
                vec!["artifacts.store: required key is missing"],
            ),
            (
                "artifacts: { store: gcs }",
                vec!["artifacts.store: not an allowed value (allowed: fs, s3)"],
            ),
            (
                "artifacts: { store: fs, fs: {} }",
                vec!["artifacts.fs.root: required key is missing"],
            ),
            (
                "artifacts: { store: fs, fs: { root: '  ' } }",
                vec!["artifacts.fs.root: must name a directory"],
            ),
        ];
        for (section, want) in cases {
            assert_eq!(
                lines(load(&format!("{MINIMAL}{section}\n"), &s3_env())),
                want,
                "{section}"
            );
        }
    }

    #[test]
    fn the_values_of_the_section_are_checked_and_every_error_is_listed() {
        let text = "\
artifacts:
  store: s3
  maxFileBytes: 0
  s3:
    bucket: Not_A_Bucket
    region: US East
    endpoint: ftp://files.example.com
    prefix: ../escape
    accessKeyId: { env: S3_ACCESS_KEY_ID }
    secretAccessKey: { env: S3_SECRET_ACCESS_KEY }
    timeoutSecs: 0
";
        let errors = lines(load(&format!("{MINIMAL}{text}"), &s3_env()));
        assert_eq!(
            errors.len(),
            2,
            "the shape errors come first and alone: {errors:?}"
        );
        assert!(
            errors[0].starts_with("artifacts.maxFileBytes: "),
            "{errors:?}"
        );
        assert!(
            errors[1].starts_with("artifacts.s3.timeoutSecs: "),
            "{errors:?}"
        );
        let text = text
            .replace("maxFileBytes: 0", "maxFileBytes: 268435457")
            .replace("timeoutSecs: 0", "timeoutSecs: 5");
        let errors = lines(load(&format!("{MINIMAL}{text}"), &s3_env()));
        assert!(
            errors[0].starts_with("artifacts.maxFileBytes: "),
            "{errors:?}"
        );
        let text = text.replace("maxFileBytes: 268435457", "maxFileBytes: 268435456");
        let errors = lines(load(&format!("{MINIMAL}{text}"), &s3_env()));
        let keys: Vec<_> = errors
            .iter()
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "artifacts.s3.bucket",
                "artifacts.s3.endpoint",
                "artifacts.s3.prefix",
                "artifacts.s3.region",
            ],
            "{errors:?}"
        );
        for bucket in [
            "ab",
            "-bucket",
            "bucket-",
            "Bucket",
            "a_b_c",
            &"a".repeat(64),
            "a/b/c",
        ] {
            let text = format!("{MINIMAL}{S3}").replace("orchestrator-files", bucket);
            assert!(load(&text, &s3_env()).is_err(), "{bucket}");
        }
        for bucket in ["abc", "my.bucket-1", &"a".repeat(63)] {
            let text = format!("{MINIMAL}{S3}").replace("orchestrator-files", bucket);
            assert!(load(&text, &s3_env()).is_ok(), "{bucket}");
        }
        for prefix in ["a/../b", "..", "/", "has space", "tr\u{e9}s", "a?b"] {
            let text = format!("{MINIMAL}{S3}").replace("prod/files", &format!("'{prefix}'"));
            assert!(load(&text, &s3_env()).is_err(), "{prefix}");
        }
        for prefix in [
            "files",
            "prod/files",
            "/leading/and/trailing/",
            "a.b_c-d/E9",
        ] {
            let text = format!("{MINIMAL}{S3}").replace("prod/files", &format!("'{prefix}'"));
            assert!(load(&text, &s3_env()).is_ok(), "{prefix}");
        }
        for endpoint in [
            "https://user:pw@h.example.com",
            "https://h.example.com?x=1",
            "minio:9000",
            "https://",
        ] {
            let text = format!("{MINIMAL}{S3}").replace("https://minio.example.com:9000", endpoint);
            assert!(load(&text, &s3_env()).is_err(), "{endpoint}");
        }
        for endpoint in [
            "http://minio:9000",
            "https://s3.eu-central-1.amazonaws.com",
            "http://127.0.0.1:9000/s3",
        ] {
            let text = format!("{MINIMAL}{S3}").replace("https://minio.example.com:9000", endpoint);
            assert!(load(&text, &s3_env()).is_ok(), "{endpoint}");
        }
    }

    #[test]
    fn a_secret_that_is_a_plain_string_never_shows_its_value() {
        let text =
            format!("{MINIMAL}{S3}").replace("{ env: S3_SECRET_ACCESS_KEY }", "hunter2-s3cr3t");
        let errors = render(&load(&text, &s3_env()).unwrap_err());
        assert!(
            errors.contains("artifacts.s3.secretAccessKey: a secret is a reference"),
            "{errors}"
        );
        assert!(!errors.contains("hunter2"), "{errors}");
    }
}

/// Every rule between keys, in one run.
#[test]
fn every_rule_error_is_listed_at_once() {
    let text = "\
version: 1
server:
  listen: nowhere
  surfaces: [agui, mcp, thread-tools, webhook-generic, webhook-github, agui]
  publicUrl: https://chat.example.com/some/path
database: { url: { env: DATABASE_URL } }
agents:
  registry: { url: 'https://user:pw@r.example.com/agents' }
gate: { maxAttempts: 4, maxAttemptsCap: 3 }
models:
  endpoints:
    default: { baseUrl: ftp://m.example.com }
    Second_One: { baseUrl: https://m.example.com/v1 }
tasks: { title: { endpoint: nowhere, model: m } }
threadTools: { allowedHosts: ['https://x'] }
mcp: { allowedHosts: ['*'], allowedOrigins: ['https://o.example.com/'] }
auth: { devUser: nobody }
";
    let errors = lines(load(text, &minimal_env()));
    let want = [
        "agents.registry.url: expected an absolute http:// or https:// URL with a host and no user name or password, like https://platform.example.com/registry/v1/agents",
        "auth.devUser: expected an e-mail address",
        "gate.maxAttempts: is above gate.maxAttemptsCap",
        "mcp.allowedHosts[0]: not a host name or address, with or without a port (no scheme, path, wildcard or credentials): write orch.example.com or orch.example.com:8443",
        "mcp.allowedOrigins[0]: not an origin: write https://host or https://host:port, with no path",
        "mcp.tokensFile: required when server.surfaces mounts mcp",
        "models.endpoints.Second_One: an endpoint name is a slug: a to z, 0 to 9 and -, 1 to 32 characters",
        "models.endpoints.default.baseUrl: expected an http:// or https:// URL, like https://api.example.com/v1",
        "server.listen: not a socket address like 0.0.0.0:8080",
        "server.publicUrl: must be an origin such as https://chat.example.com: http or https, a host, no credentials, path, query or fragment",
        "server.surfaces[5]: a surface is listed more than once",
        "tasks.title.endpoint: names no endpoint of models.endpoints",
        "threadTools.allowedHosts[0]: not a host name or address, with or without a port (no scheme, path, wildcard or credentials): write orch.example.com or orch.example.com:8443",
        "threadTools.secret: required when server.surfaces mounts thread-tools (in every role)",
        "threadTools.url: required when server.surfaces mounts thread-tools (in every role)",
        "webhooks.generic: required when server.surfaces mounts webhook-generic",
        "webhooks.github: required when server.surfaces mounts webhook-github",
    ];
    assert_eq!(errors, want);
}

#[test]
fn the_cross_key_rules_of_the_thread_tools_and_the_agents() {
    let base = "version: 1\ndatabase: { url: { env: DATABASE_URL } }\n";
    let fake = minimal_env().env("S", LONG);
    for (extra, want) in [
        (
            "agents: { file: a }\nthreadTools: { url: 'http://o:8080' }\n",
            "threadTools.secret: required with threadTools.url: set both or neither",
        ),
        (
            "agents: { file: a }\nthreadTools: { secret: { env: S } }\n",
            "threadTools.url: required with threadTools.secret: set both or neither",
        ),
        (
            "agents: { file: a }\nthreadTools: { previousSecret: { env: S } }\n",
            "threadTools.previousSecret: needs threadTools.secret: the previous key only verifies",
        ),
        (
            "agents: { file: a }\nthreadTools: { url: 'http://u:p@o:8080', secret: { env: S } }\n",
            "threadTools.url: expected an http:// or https:// URL with a host, without credentials, query or fragment",
        ),
        (
            "agents: {}\n",
            "agents.file: required unless agents.registry.url is set (the agents have to come from somewhere)",
        ),
        (
            "agents: { file: a, registry: { url: 'https://r.example.com/agents' } }\nthreadTools: { url: 'http://o:8080', secret: { env: S }, previousSecret: { env: S } }\n",
            "threadTools.previousSecret: is the same as threadTools.secret",
        ),
    ] {
        let text = format!("{base}{extra}");
        let errors = lines(load(&text, &fake));
        assert_eq!(errors, [want], "{extra}");
    }
    // A registry is enough to start without an agents file.
    let ok = format!("{base}agents:\n  registry: {{ url: 'https://r.example.com/agents' }}\n");
    assert!(load(&ok, &fake).is_ok());
}

#[test]
fn a_worker_is_not_asked_for_what_it_would_never_use() {
    // The control plane's webhook secrets are not in a worker's environment: not an error.
    let text = "\
version: 1
server: { role: worker, surfaces: [agui, webhook-generic, webhook-github, mcp] }
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
webhooks:
  generic: { secrets: [{ env: WEBHOOK_GENERIC_SECRET }] }
  github: { secrets: [{ env: WEBHOOK_GITHUB_SECRET }] }
";
    let valid = load(text, &minimal_env()).unwrap();
    assert!(valid.secrets.webhook_generic.is_empty());
    // The same file for a control plane needs them, and says which variable.
    let control = text.replace("role: worker", "role: control-plane");
    let errors = lines(load(&control, &minimal_env()));
    assert!(
        errors.contains(
            &"webhooks.generic.secrets[0]: the environment variable WEBHOOK_GENERIC_SECRET is unset or empty"
                .to_owned()
        ),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|l| l.starts_with("mcp.tokensFile: required")),
        "{errors:?}"
    );
}

#[test]
fn an_unmounted_webhook_whose_secret_is_unset_is_not_an_error_but_a_short_one_is() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
webhooks: { github: { secrets: [{ env: GH }] } }
";
    assert!(load(text, &minimal_env()).is_ok(), "not mounted, unset");
    let errors = lines(load(text, &minimal_env().env("GH", "short")));
    assert_eq!(
        errors,
        [
            "webhooks.github.secrets[0]: the secret is shorter than 32 bytes (generate one with `openssl rand -hex 32`)"
        ]
    );
}

#[test]
fn references_that_do_not_resolve_name_the_key_and_the_variable_or_path() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents:
  file: agents.yaml
  registry: { url: 'https://r.example.com/a', token: { file: /run/secrets/none }, agentToken: { file: empty } }
models: { endpoints: { default: { baseUrl: 'https://m.example.com', apiKey: { env: BLANK } } } }
threadTools: { url: 'http://o:8080', secret: { file: big } }
";
    let fake = Fake::default()
        .env("BLANK", "   ")
        .file("/etc/orchestrator/empty", "\n")
        .file("/etc/orchestrator/big", &"x".repeat(64 * 1024 + 1));
    let errors = lines(load(text, &fake));
    assert_eq!(
        errors,
        [
            "agents.registry.agentToken: the file empty is empty",
            "agents.registry.token: the file /run/secrets/none cannot be read",
            "database.url: the environment variable DATABASE_URL is unset or empty",
            "models.endpoints.default.apiKey: the environment variable BLANK is unset or empty",
            "threadTools.secret: the file big is larger than 64 KiB",
        ]
    );
}

#[test]
fn a_file_secret_loses_one_trailing_newline_and_an_env_secret_is_trimmed() {
    let text = "\
version: 1
database: { url: { file: db } }
agents: { file: agents.yaml }
threadTools: { url: 'http://o:8080', secret: { env: KEY } }
";
    let fake = Fake::default()
        .file("/etc/orchestrator/db", "postgres://x\r\n\n")
        .env("KEY", &format!("  {LONG}\n"));
    let valid = load(text, &fake).unwrap();
    assert_eq!(valid.secrets.database_url.expose(), "postgres://x\r\n");
    assert_eq!(valid.secrets.thread_tools_secret.unwrap().expose(), LONG);
}

#[test]
fn a_thread_tools_key_must_be_long_enough() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
threadTools: { url: 'http://o:8080', secret: { env: KEY } }
";
    let errors = lines(load(text, &minimal_env().env("KEY", "short")));
    assert_eq!(
        errors,
        [
            "threadTools.secret: the secret is shorter than 32 bytes (generate one with `openssl rand -hex 32`)"
        ]
    );
}

/// ADR 0034: "No error carries a value." Every message is built for a file whose values are
/// recognisable, and none of them may appear in what is printed, in any error kind.
#[test]
fn no_error_carries_a_value() {
    const SECRET: &str = "S3CR3T-VALUE-THAT-MUST-NOT-LEAK";
    let files = [
        // Shape errors: a secret pasted where a reference goes, as a number, as an enum value, as
        // the value of an unknown key, in a list.
        format!(
            "version: 1\ndatabase: {{ url: {SECRET}, maxConnections: {SECRET} }}\nagents: {{ file: a }}\n\
             server: {{ role: {SECRET}, listen: 1, surfaces: [{SECRET}] }}\n\
             log: {{ format: {SECRET} }}\nunknown: {SECRET}\nthreadTools: {{ secret: {SECRET} }}\n\
             auth: {{ devUser: [{SECRET}] }}\n"
        ),
        // Rule errors: the same text where a URL, a host, an origin, an e-mail or an address goes.
        format!(
            "version: 1\ndatabase: {{ url: {{ env: DATABASE_URL }} }}\nagents: {{ file: a, registry: {{ url: '{SECRET}' }} }}\n\
             server: {{ listen: {SECRET}, publicUrl: 'https://u:{SECRET}@chat.example.com/{SECRET}' }}\n\
             threadTools: {{ url: 'http://u:{SECRET}@o:8080', allowedHosts: [{SECRET}://x] }}\n\
             mcp: {{ allowedHosts: ['{SECRET}/x'], allowedOrigins: ['https://o.example.com/{SECRET}'] }}\n\
             auth: {{ devUser: {SECRET} }}\n\
             models: {{ endpoints: {{ main: {{ baseUrl: '{SECRET}' }} }} }}\ntasks: {{ title: {{ endpoint: {SECRET}, model: m }} }}\n"
        ),
        // The secret as a key that is repeated, a tag, a syntax error.
        format!("version: 1\nversion: {SECRET}\n"),
        format!("version: 1\ndatabase: !{SECRET} x\n"),
        format!("version: 1\ndatabase: [{SECRET}\n"),
        format!("version: {SECRET}\n"),
        format!("- {SECRET}\n"),
        // Rules that name a secret: references to unset variables and files, a short key.
        "version: 1\ndatabase: { url: { env: DATABASE_URL } }\nagents: { file: a }\n\
         threadTools: { url: 'http://o:8080', secret: { env: SHORT }, previousSecret: { env: SHORT } }\n"
            .to_owned(),
    ];
    let fake = minimal_env().env("SHORT", SECRET);
    for text in files {
        let errors = match load(&text, &fake) {
            Ok(_) => panic!("expected errors for {text}"),
            Err(errors) => errors,
        };
        assert!(!errors.is_empty());
        let shown = format!("{} {errors:?}", render(&errors));
        assert!(
            !shown.contains(SECRET),
            "a value reached an error:\n{shown}\nfor:\n{text}"
        );
    }
}

#[test]
fn debug_prints_references_and_never_a_value() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
threadTools: { url: 'http://o:8080', secret: { env: KEY } }
";
    let valid = load(text, &minimal_env().env("KEY", LONG)).unwrap();
    let shown = format!("{valid:?} {:?}", valid.secrets);
    assert!(
        !shown.contains(LONG) && !shown.contains("postgres://u:pw@db/orch"),
        "{shown}"
    );
    assert!(
        shown.contains("{ env: DATABASE_URL }") && shown.contains("{ env: KEY }"),
        "{shown}"
    );
}

#[test]
fn the_validated_configuration_prints_as_yaml_with_references_only() {
    let valid = load(MINIMAL, &minimal_env()).unwrap();
    let json = serde_json::to_value(valid.config.effective()).unwrap();
    assert_eq!(
        json["database"]["url"],
        serde_json::json!({ "env": "DATABASE_URL" })
    );
    assert_eq!(json["server"]["surfaces"], serde_json::json!(["agui"]));
    assert!(!json.to_string().contains("postgres://"));
}

const JWT: &str = "\
auth:
  mode: jwt
  jwt:
    issuer: https://idp.example/realms/main
    audiences: [orchestrator-web]
";

#[test]
fn the_default_mode_is_the_proxy_header_and_changes_nothing() {
    use orch_config::{AuthMode, Environment};
    let valid = load(MINIMAL, &minimal_env()).unwrap();
    assert_eq!(valid.config.auth.mode, AuthMode::ProxyHeader);
    assert!(valid.config.auth.jwt.is_none());
    assert_eq!(valid.config.server.environment, Environment::Development);
    // A development user is still allowed with the header.
    let text = format!("{MINIMAL}auth: {{ devUser: dev@example.com }}\n");
    assert!(load(&text, &minimal_env()).is_ok());
}

#[test]
fn a_jwt_configuration_has_defaults_for_the_user_claim_and_no_roles() {
    use orch_config::AuthMode;
    let valid = load(&format!("{MINIMAL}{JWT}"), &minimal_env()).unwrap();
    let auth = &valid.config.auth;
    assert_eq!(auth.mode, AuthMode::Jwt);
    let jwt = auth.jwt.as_ref().unwrap();
    assert_eq!(jwt.issuer, "https://idp.example/realms/main");
    assert_eq!(jwt.audiences, ["orchestrator-web"]);
    assert_eq!(jwt.user_claim, "email");
    assert!(jwt.jwks_url.is_none() && jwt.roles_claim.is_none());
    let json = serde_json::to_value(valid.config.effective()).unwrap();
    assert_eq!(json["auth"]["mode"], "jwt");
    assert_eq!(json["auth"]["jwt"]["userClaim"], "email");
}

#[test]
fn every_key_of_the_jwt_section_is_read() {
    let text = format!(
        "{MINIMAL}\
auth:
  mode: jwt_or_proxy_header
  jwt:
    issuer: http://mock-oidc:9000
    audiences: [a, b]
    jwksUrl: http://mock-oidc:9000/keys
    userClaim: preferred_username
    rolesClaim: realm_access.roles
"
    );
    let valid = load(&text, &minimal_env()).unwrap();
    let jwt = valid.config.auth.jwt.unwrap();
    assert_eq!(jwt.audiences, ["a", "b"]);
    assert_eq!(jwt.jwks_url.as_deref(), Some("http://mock-oidc:9000/keys"));
    assert_eq!(jwt.user_claim, "preferred_username");
    assert_eq!(jwt.roles_claim.as_deref(), Some("realm_access.roles"));
}

#[test]
fn the_auth_rules_name_the_key() {
    let cases: [(&str, &str); 12] = [
        (
            "auth: { mode: jwt }",
            "auth.jwt: required when auth.mode is jwt",
        ),
        (
            "auth: { mode: jwt_or_proxy_header }",
            "auth.jwt: required when auth.mode is jwt_or_proxy_header",
        ),
        (
            "auth: { jwt: { issuer: 'https://i.example', audiences: [a] } }",
            "auth.jwt: only with auth.mode jwt or jwt_or_proxy_header: it would silently do nothing",
        ),
        (
            "auth: { mode: jwt, devUser: dev@example.com, jwt: { issuer: 'https://i.example', audiences: [a] } }",
            "auth.devUser: only with auth.mode proxy_header: a development identity beside token validation would let anyone in",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'ftp://i.example', audiences: [a] } }",
            "auth.jwt.issuer: expected an http:// or https:// URL with a host, without credentials, query or fragment, like https://idp.example/realms/main",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'https://u:p@i.example', audiences: [a] } }",
            "auth.jwt.issuer: expected an http:// or https:// URL with a host, without credentials, query or fragment, like https://idp.example/realms/main",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'https://i.example?x=1', audiences: [a] } }",
            "auth.jwt.issuer: expected an http:// or https:// URL with a host, without credentials, query or fragment, like https://idp.example/realms/main",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'https://i.example', audiences: [' '] } }",
            "auth.jwt.audiences[0]: an audience is not empty",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'https://i.example', audiences: [a], jwksUrl: 'file:///x' } }",
            "auth.jwt.jwksUrl: expected an absolute http:// or https:// URL with a host and no user name or password",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'https://i.example', audiences: [a], userClaim: ' ' } }",
            "auth.jwt.userClaim: a claim name is not empty",
        ),
        (
            "auth: { mode: jwt, jwt: { issuer: 'https://i.example', audiences: [a], rolesClaim: ' ' } }",
            "auth.jwt.rolesClaim: a claim name is not empty",
        ),
        (
            "auth: { devUser: nobody }",
            "auth.devUser: expected an e-mail address",
        ),
    ];
    for (auth, expected) in cases {
        let errors = lines(load(&format!("{MINIMAL}{auth}\n"), &minimal_env()));
        assert!(errors.iter().any(|l| l == expected), "{auth}\n{errors:?}");
    }
    // An empty list of audiences is a shape error (the schema says at least one).
    let errors = lines(load(
        &format!(
            "{MINIMAL}auth: {{ mode: jwt, jwt: {{ issuer: 'https://i.example', audiences: [] }} }}\n"
        ),
        &minimal_env(),
    ));
    assert!(
        errors.iter().any(|l| l.starts_with("auth.jwt.audiences: ")),
        "{errors:?}"
    );
    // A mode that does not exist is a shape error naming the key.
    let errors = lines(load(
        &format!("{MINIMAL}auth: {{ mode: oidc }}\n"),
        &minimal_env(),
    ));
    assert!(
        errors.iter().any(|l| l.starts_with("auth.mode: ")),
        "{errors:?}"
    );
}

#[test]
fn a_production_process_refuses_the_proxy_header() {
    let production =
        |extra: &str| format!("{MINIMAL}server: {{ environment: production }}\n{extra}");
    for text in [production(""), production("auth: { mode: proxy_header }\n")] {
        let errors = lines(load(&text, &minimal_env()));
        assert!(
            errors
                .iter()
                .any(|l| l.starts_with("auth.mode: proxy_header is for a single user")),
            "{errors:?}"
        );
    }
    // Tokens are fine in production, and so is the one release of migration.
    assert!(load(&production(JWT), &minimal_env()).is_ok());
    let migration = production(
        "auth: { mode: jwt_or_proxy_header, jwt: { issuer: 'https://i.example', audiences: [a] } }\n",
    );
    assert!(load(&migration, &minimal_env()).is_ok());
    // In production the issuer's keys come over TLS only.
    let plain = production(
        "auth: { mode: jwt, jwt: { issuer: 'http://i.example', audiences: [a], jwksUrl: 'HTTP://i.example/keys' } }\n",
    );
    let errors = lines(load(&plain, &minimal_env()));
    for key in [
        "auth.jwt.issuer: an https:// URL",
        "auth.jwt.jwksUrl: an https:// URL",
    ] {
        assert!(
            errors.iter().any(|l| l.starts_with(key)),
            "{key}: {errors:?}"
        );
    }
    // Development is the default and takes the header.
    let development = format!("{MINIMAL}server: {{ environment: development }}\n");
    assert!(load(&development, &minimal_env()).is_ok());
    let errors = lines(load(
        &format!("{MINIMAL}server: {{ environment: staging }}\n"),
        &minimal_env(),
    ));
    assert!(
        errors.iter().any(|l| l.starts_with("server.environment: ")),
        "{errors:?}"
    );
}

// ---- the endpoints and the tasks (ADR 0035) ------------------------------------------------------

const ENDPOINTS: &str = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
models:
  endpoints:
    default: { baseUrl: 'https://models.example.com/v1', apiKey: { env: ORCH_MODEL_API_KEY }, timeoutSecs: 20 }
    small: { baseUrl: 'https://small.example.com/v1', apiKey: { file: /run/secrets/small-key } }
";

fn tasks_env() -> Fake {
    minimal_env()
        .env("ORCH_MODEL_API_KEY", "default-key")
        .file("/run/secrets/small-key", "small-key\n")
}

#[test]
fn several_named_endpoints_each_with_its_own_key_and_timeout() {
    let valid = load(ENDPOINTS, &tasks_env()).unwrap();
    let endpoints = &valid.config.models.endpoints;
    assert_eq!(endpoints.keys().collect::<Vec<_>>(), ["default", "small"]);
    assert_eq!(endpoints["default"].timeout_secs, 20);
    assert_eq!(endpoints["small"].timeout_secs, 20, "the default");
    assert_eq!(
        valid.secrets.model_api_keys["default"].expose(),
        "default-key"
    );
    assert_eq!(valid.secrets.model_api_keys["small"].expose(), "small-key");
}

#[test]
fn a_task_has_its_own_endpoint_model_prompt_and_limits_and_every_default_is_filled_in() {
    let text = format!(
        "{ENDPOINTS}tasks:
  title:
    endpoint: small
    model: small-model
  description:
    endpoint: default
    model: big-model
    system: {{ file: prompts/description.md }}
    maxTokens: 200
    language: french
    maxChars: 250
    recompute: {{ minNewMessages: 6 }}
"
    );
    let fake = tasks_env().file(
        "/etc/orchestrator/prompts/description.md",
        "  Say in two sentences what the person wants.\n\n",
    );
    let valid = load(&text, &fake).unwrap_or_else(|e| panic!("{}", render(&e)));
    let title = valid.config.tasks.title.as_ref().unwrap();
    assert_eq!(
        (title.endpoint.as_str(), title.model.as_str()),
        ("small", "small-model")
    );
    assert_eq!(title.max_tokens, 32);
    assert_eq!(title.language, orch_config::Language::Conversation);
    assert_eq!(title.system, None);
    let description = valid.config.tasks.description.as_ref().unwrap();
    assert_eq!(description.endpoint, "default");
    assert_eq!(description.max_tokens, 200);
    assert_eq!(description.language, orch_config::Language::French);
    assert_eq!(description.max_chars, 250);
    assert_eq!(description.recompute.min_new_messages, 6);
    // the file is read through the resolver, relative to the file's directory, and trimmed
    assert_eq!(valid.prompts.title, None);
    assert_eq!(
        valid.prompts.description.as_deref(),
        Some("Say in two sentences what the person wants.")
    );
    // an unset knob is its default
    let minimal = format!("{ENDPOINTS}tasks:\n  description: {{ endpoint: small, model: m }}\n");
    let valid = load(&minimal, &tasks_env()).unwrap();
    let d = valid.config.tasks.description.as_ref().unwrap();
    assert_eq!(
        (d.max_tokens, d.max_chars, d.recompute.min_new_messages),
        (160, 300, 4)
    );
    assert_eq!(d.language, orch_config::Language::Conversation);
    // and it is shown filled in
    let shown = serde_json::to_value(valid.config.effective()).unwrap();
    assert_eq!(shown["tasks"]["description"]["maxChars"], 300);
    assert_eq!(
        shown["tasks"]["description"]["recompute"]["minNewMessages"],
        4
    );
}

#[test]
fn an_inline_prompt_is_the_text() {
    let text = format!(
        "{ENDPOINTS}tasks:\n  title: {{ endpoint: small, model: m, system: {{ inline: 'Be brief.' }} }}\n"
    );
    let valid = load(&text, &tasks_env()).unwrap();
    assert_eq!(valid.prompts.title.as_deref(), Some("Be brief."));
}

#[test]
fn every_error_of_the_tasks_and_the_endpoints_is_listed_at_once() {
    let text = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
models:
  endpoints:
    default: { baseUrl: 'https://m.example.com/v1' }
    Bad_Name: { baseUrl: 'https://m.example.com/v1' }
    far: { baseUrl: 'ftp://m.example.com' }
tasks:
  title: { endpoint: nowhere, model: m, system: { inline: '   ' } }
  description: { endpoint: also-nowhere, model: m, system: { file: missing.md } }
";
    let errors = lines(load(text, &minimal_env()));
    assert_eq!(
        errors,
        [
            "models.endpoints.Bad_Name: an endpoint name is a slug: a to z, 0 to 9 and -, 1 to 32 characters",
            "models.endpoints.far.baseUrl: expected an http:// or https:// URL, like https://api.example.com/v1",
            "tasks.description.endpoint: names no endpoint of models.endpoints",
            "tasks.description.system: the file missing.md cannot be read",
            "tasks.title.endpoint: names no endpoint of models.endpoints",
            "tasks.title.system: the prompt is empty",
        ]
    );
}

#[test]
fn a_prompt_file_that_is_empty_or_too_long_is_an_error_that_names_the_file() {
    let base = format!(
        "{ENDPOINTS}tasks:\n  title: {{ endpoint: small, model: m, system: {{ file: p.md }} }}\n"
    );
    let empty = tasks_env().file("/etc/orchestrator/p.md", " \n\n ");
    assert_eq!(
        lines(load(&base, &empty)),
        ["tasks.title.system: the file p.md is empty"]
    );
    let long = tasks_env().file(
        "/etc/orchestrator/p.md",
        &"x".repeat(orch_config::MAX_PROMPT_BYTES + 1),
    );
    assert_eq!(
        lines(load(&base, &long)),
        ["tasks.title.system: the file p.md is larger than 4 KiB"]
    );
    let exact = tasks_env().file(
        "/etc/orchestrator/p.md",
        &"x".repeat(orch_config::MAX_PROMPT_BYTES),
    );
    assert!(load(&base, &exact).is_ok());
    // a file that is not UTF-8 is read as an error by the resolver, and is "cannot be read"
    let absent = tasks_env();
    assert_eq!(
        lines(load(&base, &absent)),
        ["tasks.title.system: the file p.md cannot be read"]
    );
}

#[test]
fn the_shape_of_a_task_is_checked_with_the_key_path_and_the_allowed_values() {
    let text = format!(
        "{ENDPOINTS}tasks:
  title:
    endpoint: small
    model: m
    maxTokens: 0
    language: klingon
    system: just text
  description:
    endpoint: small
    model: m
    maxTokens: 2000
    maxChars: 10
    recompute: {{ minNewMessages: 0 }}
    extra: 1
    system: {{ inline: x, file: y }}
  turnSummary: {{ endpoint: small, model: m }}
"
    );
    let errors = lines(load(&text, &tasks_env()));
    let find = |key: &str| {
        errors
            .iter()
            .find(|l| l.starts_with(&format!("{key}: ")))
            .unwrap_or_else(|| panic!("{key} in {errors:#?}"))
            .as_str()
    };
    assert_eq!(
        find("tasks.title.maxTokens"),
        "tasks.title.maxTokens: must be at least 1"
    );
    assert!(
        find("tasks.title.language")
            .contains("not an allowed value (allowed: conversation, english, french")
    );
    assert_eq!(
        find("tasks.title.system"),
        "tasks.title.system: a prompt is `{ inline: TEXT }` or `{ file: PATH }`"
    );
    assert_eq!(
        find("tasks.description.maxTokens"),
        "tasks.description.maxTokens: must be at most 1024"
    );
    assert_eq!(
        find("tasks.description.maxChars"),
        "tasks.description.maxChars: must be at least 40"
    );
    assert_eq!(
        find("tasks.description.recompute.minNewMessages"),
        "tasks.description.recompute.minNewMessages: must be at least 1"
    );
    assert_eq!(
        find("tasks.description.extra"),
        "tasks.description.extra: unknown key"
    );
    assert_eq!(
        find("tasks.description.system"),
        "tasks.description.system: a prompt is `{ inline: TEXT }` or `{ file: PATH }`"
    );
    assert!(find("tasks.turnSummary").contains("reserved for"));
    // the title's maximum is 256 and the description's 1024
    let over =
        format!("{ENDPOINTS}tasks:\n  title: {{ endpoint: small, model: m, maxTokens: 257 }}\n");
    assert_eq!(
        lines(load(&over, &tasks_env())),
        ["tasks.title.maxTokens: must be at most 256"]
    );
}

#[test]
fn a_prompt_is_not_a_value_in_an_error() {
    const SECRET: &str = "S3CR3T-PROMPT-THAT-MUST-NOT-LEAK";
    let text = format!(
        "{ENDPOINTS}tasks:\n  title: {{ endpoint: nowhere-{SECRET}, model: m, system: {{ file: ../{SECRET}.md }} }}\n  description: {{ endpoint: small, model: m, language: {SECRET}, system: [{SECRET}] }}\n"
    );
    let errors = load(&text, &tasks_env()).unwrap_err();
    let shown = format!("{} {errors:?}", render(&errors));
    // a path the file names is shown (as a secret's file is); the endpoint's name and the
    // unknown language are not
    assert!(!shown.contains(&format!("nowhere-{SECRET}")), "{shown}");
    assert!(!shown.contains(&format!("language: {SECRET}")), "{shown}");
    let shape = lines(load(
        &format!(
            "{ENDPOINTS}tasks:\n  description: {{ endpoint: small, model: m, language: {SECRET}, system: [{SECRET}] }}\n"
        ),
        &tasks_env(),
    ));
    assert!(shape.iter().all(|l| !l.contains(SECRET)), "{shape:?}");
}

#[test]
fn the_ui_section_has_its_defaults_and_is_the_only_public_part() {
    let valid = load(MINIMAL, &minimal_env()).unwrap();
    assert_eq!(valid.config.ui, orch_config::Ui::default());
    let hidden = format!("{MINIMAL}ui: {{ showDescriptions: false }}\n");
    assert!(
        !load(&hidden, &minimal_env())
            .unwrap()
            .config
            .ui
            .show_descriptions
    );
    let wrong = format!("{MINIMAL}ui: {{ showDescriptions: 'yes', theme: dark }}\n");
    assert_eq!(
        lines(load(&wrong, &minimal_env())),
        [
            "ui.showDescriptions: expected boolean",
            "ui.theme: unknown key"
        ]
    );
    // the effective configuration says it (what --print-config prints)
    let json = serde_json::to_value(valid.config.effective()).unwrap();
    assert_eq!(json["ui"], serde_json::json!({ "showDescriptions": true }));
}

/// What `--print-config` prints is a file the loader reads back: a prompt is the mapping it was
/// written as, never a YAML tag.
#[test]
fn the_printed_configuration_reads_back_with_its_tasks() {
    let text = format!(
        "{ENDPOINTS}tasks:\n  title: {{ endpoint: small, model: m, system: {{ file: p.md }} }}\n  description: {{ endpoint: small, model: m, system: {{ inline: 'Be brief.' }}, language: german }}\n"
    );
    let fake = tasks_env().file("/etc/orchestrator/p.md", "Guidance.\n");
    let valid = load(&text, &fake).unwrap();
    let printed = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!printed.contains('!'), "no YAML tag: {printed}");
    assert!(
        printed.contains("file: p.md") && printed.contains("inline: Be brief."),
        "{printed}"
    );
    let again = load(&printed, &fake).unwrap_or_else(|e| panic!("{}\n{printed}", render(&e)));
    assert_eq!(again.config, valid.config.effective());
    assert_eq!(again.prompts, valid.prompts);
}
