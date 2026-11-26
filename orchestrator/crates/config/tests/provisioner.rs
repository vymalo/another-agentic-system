//! `provisioner`: a person's own agents on the platform. Off unless the section is present; a good
//! section; every kind of mistake named by its key; the client secret resolved and never printed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{ProvisionedKind, ProvisionerCredentials};
use support::{Fake, lines, load};

const HEAD: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

fn env() -> Fake {
    Fake::default()
        .env("DATABASE_URL", "postgres://u:pw@db/orch")
        .env("PLATFORM_CLIENT_SECRET", "hunter2-hunter2")
}

const GOOD: &str = "\
provisioner:
  url: http://platform-api:8080
  kinds: [coder, workspace]
  forms:
    coder: { kind: coder, model: { endpoint: main } }
    workspace: { kind: workspace }
  credentials:
    type: dev-oidc
    tokenUrl: http://mock-oidc:8080/token
    clientId: orchestrator
    clientSecret: { env: PLATFORM_CLIENT_SECRET }
    user: dev@example.com
    audience: platform
    roles: [\"platform:agents.own\"]
";

fn with(section: &str) -> String {
    format!("{HEAD}{section}")
}

#[test]
fn a_file_without_the_section_is_off_and_prints_nothing_of_it() {
    let valid = load(HEAD, &env()).unwrap();
    assert!(valid.config.provisioner.is_none());
    assert!(valid.secrets.provisioner_client_secret.is_none());
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!shown.contains("provisioner"), "{shown}");
}

#[test]
fn a_good_section_is_read_with_its_defaults_and_its_secret_resolved() {
    let valid = load(&with(GOOD), &env()).unwrap();
    let section = valid.config.provisioner.as_ref().unwrap();
    assert_eq!(
        section.kinds,
        [ProvisionedKind::Coder, ProvisionedKind::Workspace]
    );
    assert_eq!(section.request_timeout_secs, 5);
    assert_eq!(section.ready_timeout_secs, 120);
    assert!(matches!(
        section.credentials,
        ProvisionerCredentials::DevOidc { .. }
    ));
    let secret = valid.secrets.provisioner_client_secret.as_ref().unwrap();
    assert_eq!(secret.expose(), "hunter2-hunter2");
    let debug = format!("{:?}", valid.secrets);
    assert!(!debug.contains("hunter2"), "{debug}");
}

#[test]
fn every_mistake_is_named_by_its_key() {
    let bad = with(
        "\
provisioner:
  url: ftp://platform
  kinds: [coder, coder]
  forms:
    coder: { name: x }
    workspace: {}
  credentials:
    type: dev-oidc
    tokenUrl: \"http://u:p@issuer/token\"
    clientId: \"  \"
    clientSecret: { env: NOT_SET }
",
    );
    let text = lines(load(&bad, &env())).join("\n");
    for key in [
        "provisioner.url",
        "provisioner.kinds[1]",
        "provisioner.forms.coder.name",
        "provisioner.forms.workspace",
        "provisioner.credentials.tokenUrl",
        "provisioner.credentials.clientId",
        "provisioner.credentials.clientSecret",
    ] {
        assert!(text.contains(key), "{key} not named in:\n{text}");
    }
}

#[test]
fn a_kind_needs_its_form_and_unknown_things_are_refused() {
    let no_form = GOOD.replace("    workspace: { kind: workspace }\n", "");
    let text = lines(load(&with(&no_form), &env())).join("\n");
    assert!(text.contains("provisioner.forms.workspace"), "{text}");

    let shared = GOOD.replace("kinds: [coder, workspace]", "kinds: [chat]");
    assert!(
        load(&with(&shared), &env()).is_err(),
        "the chat agent is never provisioned"
    );

    let unknown = GOOD.replace("  url: http", "  extra: 1\n  url: http");
    assert!(load(&with(&unknown), &env()).is_err());

    let out_of_range = GOOD.replace("kinds:", "requestTimeoutSecs: 0\n  kinds:");
    assert!(load(&with(&out_of_range), &env()).is_err());
}
