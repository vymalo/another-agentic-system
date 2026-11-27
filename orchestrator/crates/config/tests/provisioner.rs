//! `provisioner`: a person's own agents on the platform. Off unless the section is present; a good
//! section; every kind of mistake named by its key; the client secret resolved and never printed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{ProvisionedKind, ProvisionerCredentials};
use support::{Fake, lines, load};

/// A file with only what is required and no `auth` section.
const BARE: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

/// The one `auth` a `provisioner` section is read beside: tokens, and a person who is the `sub`.
const JWT_SUB: &str = concat!(
    "auth:\n",
    "  mode: jwt\n",
    "  jwt: { issuer: 'https://idp.example/realms/main', audiences: [web], userClaim: sub }\n",
);

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
    format!("{BARE}{JWT_SUB}{section}")
}

#[test]
fn a_file_without_the_section_is_off_and_prints_nothing_of_it() {
    // the default auth, `proxy_header` with no token claim: only a `provisioner` section asks for
    // the `sub`
    let valid = load(BARE, &env()).unwrap();
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
fn a_form_that_names_who_is_charged_is_refused_at_that_key() {
    // the account is the person's, set per agent: one in the file would charge everybody to it
    let charged = GOOD.replace(
        "    workspace: { kind: workspace }\n",
        "    workspace: { kind: workspace, account: acct-from-the-file }\n",
    );
    let text = lines(load(&with(&charged), &env())).join("\n");
    assert!(
        text.contains("provisioner.forms.workspace.account"),
        "{text}"
    );
    assert!(!text.contains("acct-from-the-file"), "{text}");
}

#[test]
fn a_provisioner_is_refused_at_startup_unless_every_person_is_the_sub() {
    let jwt = |mode: &str, claim: &str| {
        format!(
            "auth:\n  mode: {mode}\n  jwt: {{ issuer: 'https://idp.example/realms/main', \
             audiences: [web]{claim} }}\n"
        )
    };
    for auth in [
        // today's default claim, spelled out and not; another claim the alphabet would pass
        jwt("jwt", ", userClaim: email"),
        jwt("jwt", ""),
        jwt("jwt", ", userClaim: preferred_username"),
        // the identity header: beside the tokens, alone, and its development user
        jwt("jwt_or_proxy_header", ", userClaim: sub"),
        String::new(),
        "auth:\n  mode: proxy_header\n  devUser: dev@example.com\n".to_owned(),
    ] {
        let errors = lines(load(&format!("{BARE}{auth}{GOOD}"), &env()));
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("provisioner:") && e.contains("userClaim: sub")),
            "{auth:?}: {errors:?}"
        );
        assert!(errors.iter().all(|e| !e.contains("hunter2")), "{errors:?}");
    }
    load(&with(GOOD), &env()).unwrap();
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

const PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";

const PKI: &str = "\
provisioner:
  url: https://platform-api.internal
  kinds: [coder]
  forms:
    coder: { kind: coder }
  credentials:
    type: openbao-pki
    address: https://bao.internal:8200
    caFile: bao-ca.pem
    authRole: orchestrator-platform
    pkiMount: pki-workload
    pkiRole: orchestrator-platform-client
    spiffeId: spiffe://example.org/ns/orchestrator/sa/orchestrator
    serverCaFile: platform-ca.pem
";

fn pki_files() -> Fake {
    env()
        .file("/etc/orchestrator/bao-ca.pem", PEM)
        .file("/etc/orchestrator/platform-ca.pem", PEM)
}

#[test]
fn the_workload_certificate_is_read_with_its_defaults_and_needs_no_secret() {
    let valid = load(&with(PKI), &pki_files()).unwrap();
    let section = valid.config.provisioner.as_ref().unwrap();
    let ProvisionerCredentials::OpenbaoPki {
        kubernetes_mount,
        ttl_secs,
        renewal_margin_secs,
        service_account_token_file,
        ..
    } = &section.credentials
    else {
        panic!("not the openbao-pki credentials: {:?}", section.credentials);
    };
    assert_eq!(kubernetes_mount, "kubernetes");
    assert_eq!((*ttl_secs, *renewal_margin_secs), (86_400, 28_800));
    assert!(service_account_token_file.is_none());
    // No client secret is resolved, and the two authorities are read.
    assert!(valid.secrets.provisioner_client_secret.is_none());
    assert!(
        valid
            .secrets
            .provisioner_openbao_ca
            .as_ref()
            .unwrap()
            .expose()
            .contains("BEGIN CERTIFICATE")
    );
    assert!(valid.secrets.provisioner_server_ca.is_some());
}

#[test]
fn a_workload_certificate_without_authorities_uses_the_system_roots() {
    let bare = PKI
        .replace("    caFile: bao-ca.pem\n", "")
        .replace("    serverCaFile: platform-ca.pem\n", "");
    let valid = load(&with(&bare), &env()).unwrap();
    assert!(valid.secrets.provisioner_openbao_ca.is_none());
    assert!(valid.secrets.provisioner_server_ca.is_none());
}

#[test]
fn every_mistake_of_the_workload_certificate_is_named_by_its_key() {
    let bad = with(
        "\
provisioner:
  url: http://platform-api.internal
  kinds: [coder]
  forms:
    coder: { kind: coder }
  credentials:
    type: openbao-pki
    address: http://bao.internal
    caFile: missing.pem
    kubernetesMount: a/../b
    authRole: a/b
    serviceAccountTokenFile: relative/token
    pkiMount: a/../b
    pkiRole: x y
    spiffeId: spiffe://Example.org
    ttlSecs: 60
    renewalMarginSecs: 60
    serverCaFile: not-pem.pem
",
    );
    let text = lines(load(
        &bad,
        &env().file("/etc/orchestrator/not-pem.pem", "hello"),
    ))
    .join("\n");
    for key in [
        "provisioner.url",
        "provisioner.credentials.address",
        "provisioner.credentials.caFile",
        "provisioner.credentials.kubernetesMount",
        "provisioner.credentials.authRole",
        "provisioner.credentials.serviceAccountTokenFile",
        "provisioner.credentials.pkiMount",
        "provisioner.credentials.pkiRole",
        "provisioner.credentials.spiffeId",
        "provisioner.credentials.renewalMarginSecs",
        "provisioner.credentials.serverCaFile",
    ] {
        assert!(text.contains(key), "{key} not named in:\n{text}");
    }
}

/// A number out of its range, or an empty name, is the shape pass's to refuse (the schema, which
/// runs before the rules and stops the load), and the schema reports it at the variant of the
/// credentials it is in (`oneOf`), not at the key: so these name `provisioner.credentials`, and the
/// rules' own messages (the test above) are for what the schema cannot say.
#[test]
fn a_value_out_of_the_schemas_range_is_refused_at_the_credentials() {
    for (from, to) in [
        ("    pkiMount: pki-workload\n", "    pkiMount: \"\"\n"),
        ("    serverCaFile:", "    ttlSecs: 30\n    serverCaFile:"),
    ] {
        let text = PKI.replace(from, to);
        let shown = lines(load(&with(&text), &pki_files())).join("\n");
        assert!(
            shown.contains("provisioner.credentials"),
            "{to:?} not refused at the credentials: {shown}"
        );
    }
}

#[test]
fn the_margin_must_be_less_than_the_life() {
    for margin in ["86400", "100000"] {
        let text = PKI.replace(
            "    serverCaFile:",
            &format!("    renewalMarginSecs: {margin}\n    serverCaFile:"),
        );
        let shown = lines(load(&with(&text), &pki_files())).join("\n");
        assert!(
            shown.contains("provisioner.credentials.renewalMarginSecs"),
            "{margin}: {shown}"
        );
    }
    let just_under = PKI.replace(
        "    serverCaFile:",
        "    renewalMarginSecs: 86399\n    serverCaFile:",
    );
    load(&with(&just_under), &pki_files()).unwrap();
}
