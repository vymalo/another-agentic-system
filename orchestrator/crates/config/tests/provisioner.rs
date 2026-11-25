//! `provisioner`: a person's own agents. Off when absent; every mistake listed at once and naming
//! the key; the development issuer is refused in production.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::ProvisionedKind;
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
        .env("DEV_SECRET", "dev-client-secret")
}

fn file(section: &str) -> String {
    format!("{HEAD}provisioner:\n{section}")
}

const GOOD: &str = "  url: http://platform-api:8080
  kinds: [coder, workspace]
  credentials:
    dev:
      url: http://mock-oidc:8099
      clientId: dev-chat
      clientSecret: { env: DEV_SECRET }
";

#[test]
fn a_file_without_the_section_has_it_off_and_prints_nothing_of_it() {
    let valid = load(HEAD, &env()).unwrap();
    assert!(valid.config.provisioner.is_none());
    assert!(valid.secrets.provisioner_dev_client_secret.is_none());
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!shown.contains("provisioner"), "{shown}");
}

#[test]
fn a_section_is_read_with_its_defaults_and_its_secret() {
    let valid = load(&file(GOOD), &env()).unwrap();
    let section = valid.config.provisioner.as_ref().unwrap();
    assert_eq!(
        section.kinds,
        [ProvisionedKind::Coder, ProvisionedKind::Workspace]
    );
    assert_eq!(section.request_timeout_secs, 5);
    assert_eq!(section.ready_timeout_secs, 120);
    assert_eq!(
        valid
            .secrets
            .provisioner_dev_client_secret
            .as_ref()
            .unwrap()
            .expose(),
        "dev-client-secret"
    );
}

#[test]
fn every_mistake_is_listed_at_once_naming_the_key() {
    let bad = file(
        "  url: ftp://platform
  kinds: [coder, coder]
  credentials:
    dev:
      url: not a url
      clientId: \" \"
      clientSecret: { env: DEV_SECRET }
",
    );
    let all = lines(load(&bad, &env())).join("\n");
    for key in [
        "provisioner.url",
        "provisioner.kinds[1]",
        "provisioner.credentials.dev.url",
        "provisioner.credentials.dev.clientId",
    ] {
        assert!(all.contains(key), "{key} missing from:\n{all}");
    }
}

#[test]
fn no_kind_is_a_shape_error_and_no_credential_source_a_rule_error() {
    let shape = lines(load(&file("  url: http://p:8080\n  kinds: []\n"), &env())).join("\n");
    assert!(shape.contains("provisioner.kinds"), "{shape}");
    let rule = lines(load(
        &file("  url: http://p:8080\n  kinds: [coder]\n"),
        &env(),
    ))
    .join("\n");
    assert!(rule.contains("provisioner.credentials"), "{rule}");
}

#[test]
fn a_timeout_out_of_range_is_a_shape_error_naming_the_key() {
    for (key, value) in [("requestTimeoutSecs", 0), ("readyTimeoutSecs", 601)] {
        let text = file(&format!("{GOOD}  {key}: {value}\n"));
        let all = lines(load(&text, &env())).join("\n");
        assert!(all.contains(&format!("provisioner.{key}")), "{all}");
    }
}

#[test]
fn a_secret_that_does_not_resolve_is_named() {
    let all = lines(load(
        &file(GOOD),
        &Fake::default().env("DATABASE_URL", "postgres://u:pw@db/o"),
    ))
    .join("\n");
    assert!(
        all.contains("provisioner.credentials.dev.clientSecret"),
        "{all}"
    );
}

#[test]
fn the_development_issuer_is_refused_in_production() {
    let text = format!("{HEAD}server:\n  environment: production\nprovisioner:\n{GOOD}");
    let all = lines(load(&text, &env())).join("\n");
    assert!(all.contains("provisioner.credentials.dev"), "{all}");
    assert!(all.contains("production"), "{all}");
}
