//! `userToolServers` (ADR 0046, slice 4a): whether people may add MCP servers of their own, what the
//! orchestrator may connect to for them, and the OAuth client it presents. Off when absent; every
//! mistake listed at once and naming the key; the id rule that only the feature being on makes; and
//! the permission `tools.user.add`, which no role holds by default.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{AuthPermission, DEFAULT_OAUTH_CLIENT_NAME, MAX_EGRESS_PATTERNS};
use support::{Fake, lines, load};

const HEAD: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

fn env() -> Fake {
    Fake::default().env("DATABASE_URL", "postgres://u:pw@db/orch")
}

fn file(section: &str) -> String {
    format!("{HEAD}userToolServers:\n{section}")
}

#[test]
fn a_file_without_the_section_has_the_feature_off_and_prints_nothing_of_it() {
    let valid = load(HEAD, &env()).unwrap();
    assert!(valid.config.user_tool_servers.is_none());
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!shown.contains("userToolServers"), "{shown}");
}

#[test]
fn an_empty_section_is_off_and_has_the_defaults() {
    let valid = load(&file("  {}\n"), &env()).unwrap();
    let section = valid.config.user_tool_servers.as_ref().unwrap();
    assert!(!section.enabled);
    assert!(section.egress.allow.is_empty());
    assert!(section.egress.deny.is_empty());
    assert!(!section.egress.allow_http);
    assert_eq!(section.oauth.client_name, DEFAULT_OAUTH_CLIENT_NAME);
    assert_eq!(section.oauth.client_metadata_url, None);
}

#[test]
fn a_section_names_what_the_deployment_allows() {
    let valid = load(
        &file(
            "  enabled: true
  egress:
    allow: [mcp.example.com, \"*.tools.example.org\", 203.0.113.7, \"[2001:db8::7]\", 2001:db8::8]
    deny: [evil.example.com, \"*.internal\"]
  oauth:
    clientName: Acme Chat
    clientMetadataUrl: https://chat.example.com/.well-known/oauth-client.json
",
        ),
        &env(),
    )
    .unwrap();
    let section = valid.config.user_tool_servers.as_ref().unwrap();
    assert!(section.enabled);
    assert_eq!(section.egress.allow.len(), 5);
    assert_eq!(section.egress.deny, ["evil.example.com", "*.internal"]);
    assert_eq!(section.oauth.client_name, "Acme Chat");
    assert_eq!(
        section.oauth.client_metadata_url.as_deref(),
        Some("https://chat.example.com/.well-known/oauth-client.json")
    );
}

#[test]
fn a_section_that_is_checked_whether_it_is_on_or_off() {
    // a typo is heard now, not on the day the feature is turned on
    for enabled in ["false", "true"] {
        let errors = lines(load(
            &file(&format!(
                "  enabled: {enabled}\n  egress:\n    allow: [\"not a host\"]\n"
            )),
            &env(),
        ));
        assert_eq!(errors.len(), 1, "enabled: {enabled}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.egress.allow[0]: not a host pattern"),
            "{errors:?}"
        );
    }
}

/// The longest label and the longest name a pattern may be, and the first byte beyond each
/// (RFC 1035: 63 bytes a label, 253 a name). The boundaries are pinned on both sides, so that a rule
/// which is one byte too strict or one too loose fails here.
fn host_boundaries() -> (Vec<String>, Vec<String>) {
    let label = |n: usize| "a".repeat(n);
    // 63 + 1 + 63 + 1 + 63 + 1 + 61 = 253 bytes; with a last label of 62, 254
    let name = |last: usize| format!("{0}.{0}.{0}.{1}", label(63), label(last));
    let accepted = vec![
        format!("{}.com", label(63)),
        name(61),
        // one trailing dot is ignored, so this is 254 bytes written and 253 read
        format!("{}.", name(61)),
        format!("*.{}", name(61)),
        format!("*.{}.com", label(63)),
    ];
    let refused = vec![
        format!("{}.com", label(64)),
        name(62),
        format!("{}.", name(62)),
        format!("*.{}", name(62)),
        format!("*.{}.com", label(64)),
    ];
    (accepted, refused)
}

#[test]
fn a_host_pattern_may_be_the_longest_name_and_label_and_no_longer() {
    let (accepted, refused) = host_boundaries();
    assert_eq!(accepted[1].len(), 253);
    assert_eq!(refused[1].len(), 254);
    for entry in &accepted {
        let valid = load(
            &file(&format!(
                "  egress:\n    allow: [{entry:?}]\n    deny: [{entry:?}]\n"
            )),
            &env(),
        );
        assert!(
            valid.is_ok(),
            "{entry:?} is within the limits: {:?}",
            lines(valid)
        );
    }
    for entry in &refused {
        let errors = lines(load(
            &file(&format!("  egress:\n    deny: [{entry:?}]\n")),
            &env(),
        ));
        assert_eq!(errors.len(), 1, "{entry:?}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.egress.deny[0]: "),
            "{entry:?}: {errors:?}"
        );
    }
}

#[test]
fn a_host_pattern_is_what_the_egress_guard_reads() {
    let good = [
        "mcp.example.com",
        "MCP.Example.COM",
        "mcp.example.com.",
        "localhost",
        "a_b.example.com",
        "*.example.com",
        "*.internal",
        "203.0.113.7",
        "[2001:db8::7]",
        "2001:db8::7",
        "[::ffff:8.8.8.8]",
        "xn--bcher-kva.example",
        "münchen.example",
    ];
    let bad = [
        "",
        " ",
        "mcp.example.com/path",
        "https://mcp.example.com",
        "mcp.example.com:443",
        "user@mcp.example.com",
        "*",
        "*.",
        "*.*.example.com",
        "*.203.0.113.7",
        "mcp.*.com",
        "mcp..example.com",
        ".example.com",
        "mcp example.com",
        // addresses in a spelling only the URL parser reads
        "010.0.0.1",
        "127.1",
        "0x7f.1",
        "2130706433",
        "[2001:db8::7%eth0]",
        "[1.2.3.4]",
        &"a".repeat(64),
        &format!("{}.com", "a.".repeat(130)),
    ];
    for entry in good {
        let valid = load(
            &file(&format!(
                "  egress:\n    allow: [{entry:?}]\n    deny: [{entry:?}]\n"
            )),
            &env(),
        );
        assert!(
            valid.is_ok(),
            "{entry:?} should be a host pattern: {:?}",
            lines(valid)
        );
    }
    for entry in bad {
        let errors = lines(load(
            &file(&format!("  egress:\n    deny: [{entry:?}]\n")),
            &env(),
        ));
        assert_eq!(errors.len(), 1, "{entry:?}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.egress.deny[0]: "),
            "{entry:?}: {errors:?}"
        );
    }
}

#[test]
fn a_bad_entry_is_named_by_its_key_and_never_echoed() {
    let errors = lines(load(
        &file("  egress:\n    allow: [\"https://s3cr3t-entry.example.com/x\"]\n"),
        &env(),
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("userToolServers.egress.allow[0]: "),
        "{errors:?}"
    );
    assert!(!errors[0].contains("s3cr3t-entry"), "{errors:?}");
}

#[test]
fn an_entry_listed_twice_in_one_list_is_an_error_naming_the_second() {
    let errors = lines(load(
        &file(
            "  egress:\n    allow: [a.example.com, b.example.com, a.example.com]\n    deny: [x.example.com, x.example.com]\n",
        ),
        &env(),
    ));
    assert_eq!(
        errors,
        [
            "userToolServers.egress.allow[2]: is listed more than once",
            "userToolServers.egress.deny[1]: is listed more than once"
        ]
    );
}

#[test]
fn a_list_is_bounded() {
    let many = (0..=MAX_EGRESS_PATTERNS)
        .map(|n| format!("h{n}.example.com"))
        .collect::<Vec<_>>()
        .join(", ");
    let errors = lines(load(
        &file(&format!("  egress:\n    allow: [{many}]\n")),
        &env(),
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("userToolServers.egress.allow: "),
        "{errors:?}"
    );
    let some = &many[..many.rfind(',').unwrap()];
    assert!(load(&file(&format!("  egress:\n    allow: [{some}]\n")), &env()).is_ok());
}

#[test]
fn plain_http_is_for_development_and_refused_in_production() {
    let section = "  egress:\n    allowHttp: true\n";
    let development = load(&file(section), &env()).unwrap();
    assert!(
        development
            .config
            .user_tool_servers
            .as_ref()
            .unwrap()
            .egress
            .allow_http
    );
    let explicit =
        format!("{HEAD}server:\n  environment: development\nuserToolServers:\n{section}");
    assert!(load(&explicit, &env()).is_ok());

    // refused in production, on or off: a file copied from staging is heard of at once
    for enabled in ["false", "true"] {
        let text = format!(
            "{HEAD}server:\n  environment: production\nauth:\n  mode: jwt\n  jwt: {{ issuer: \"https://idp.example/realms/main\", audiences: [web] }}\nuserToolServers:\n  enabled: {enabled}\n  egress:\n    allowHttp: true\n"
        );
        let errors = lines(load(&text, &env()));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].starts_with(
                "userToolServers.egress.allowHttp: refused when server.environment is production"
            ),
            "{errors:?}"
        );
    }
    // and allowed to stay off in production
    let text = format!(
        "{HEAD}server:\n  environment: production\nauth:\n  mode: jwt\n  jwt: {{ issuer: \"https://idp.example/realms/main\", audiences: [web] }}\nuserToolServers:\n  enabled: true\n"
    );
    assert!(
        load(&text, &env()).is_ok(),
        "{:?}",
        lines(load(&text, &env()))
    );
}

#[test]
fn the_oauth_client_has_a_name_and_a_document_the_broker_can_use() {
    for name in ["", "   ", "line\\nbreak", &"n".repeat(81)] {
        let errors = lines(load(
            &file(&format!("  oauth:\n    clientName: \"{name}\"\n")),
            &env(),
        ));
        assert_eq!(
            errors,
            [
                "userToolServers.oauth.clientName: a name is 1 to 80 characters of one line, and not blank"
            ],
            "{name:?}"
        );
    }
    assert!(
        load(
            &file(&format!(
                "  oauth:\n    clientName: \"{}\"\n",
                "n".repeat(80)
            )),
            &env()
        )
        .is_ok()
    );

    let good = [
        "https://chat.example.com/oauth/client.json",
        "https://chat.example.com:8443/c",
        "https://chat.example.com/c?v=1",
    ];
    for url in good {
        assert!(
            load(
                &file(&format!("  oauth:\n    clientMetadataUrl: {url}\n")),
                &env()
            )
            .is_ok(),
            "{url}"
        );
    }
    let bad = [
        "http://chat.example.com/oauth/client.json",
        "https://chat.example.com",
        "https://chat.example.com/",
        "https://chat.example.com/c#frag",
        "https://user:pw@chat.example.com/c",
        "https://user@chat.example.com/c",
        "not a url",
        "/relative/path",
        &format!("https://chat.example.com/{}", "p".repeat(2048)),
    ];
    for url in bad {
        let errors = lines(load(
            &file(&format!("  oauth:\n    clientMetadataUrl: \"{url}\"\n")),
            &env(),
        ));
        assert_eq!(errors.len(), 1, "{url}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.oauth.clientMetadataUrl: expected an https URL"),
            "{url}: {errors:?}"
        );
        // a URL can carry a secret in its user information: it is not echoed
        assert!(!errors[0].contains("pw@"), "{errors:?}");
    }
}

#[test]
fn an_unknown_key_of_the_section_is_named() {
    let errors = lines(load(
        &file("  enabled: true\n  egress:\n    allowPrivate: true\n"),
        &env(),
    ));
    assert_eq!(errors, ["userToolServers.egress.allowPrivate: unknown key"]);
    let errors = lines(load(&file("  enabled: yes please\n"), &env()));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("userToolServers.enabled: "),
        "{errors:?}"
    );
}

const SERVERS: &str = "\
toolServers:
  - { id: websearch, name: Web search, url: \"https://search.example.com/mcp\" }
  - { id: u-web, name: Mine, url: \"https://mine.example.com/mcp\" }
  - { id: u-0123456789, name: Alike, url: \"https://alike.example.com/mcp\" }
  - { id: pu-1, name: Not a prefix, url: \"https://p.example.com/mcp\" }
";

#[test]
fn a_deployment_server_may_not_have_the_id_of_a_persons_while_the_feature_is_on() {
    let on = format!("{HEAD}userToolServers:\n  enabled: true\n{SERVERS}");
    let errors = lines(load(&on, &env()));
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].starts_with("toolServers[1].id: starts with u-"),
        "{errors:?}"
    );
    assert!(
        errors[1].starts_with("toolServers[2].id: starts with u-"),
        "{errors:?}"
    );

    // off, or no section: the ids are free, as they have always been
    let off = format!("{HEAD}userToolServers:\n  enabled: false\n{SERVERS}");
    assert!(load(&off, &env()).is_ok());
    let none = format!("{HEAD}{SERVERS}");
    assert!(load(&none, &env()).is_ok());
    // and with the feature on, ids that do not start with u- are fine
    let fine = format!(
        "{HEAD}userToolServers:\n  enabled: true\ntoolServers:\n  - {{ id: websearch, name: Web search, url: \"https://search.example.com/mcp\" }}\n  - {{ id: pu-1, name: Not a prefix, url: \"https://p.example.com/mcp\" }}\n  - {{ id: u, name: Short, url: \"https://s.example.com/mcp\" }}\n"
    );
    assert!(
        load(&fine, &env()).is_ok(),
        "{:?}",
        lines(load(&fine, &env()))
    );
}

#[test]
fn the_feature_on_with_no_secrets_section_is_valid_and_only_none_servers_will_be_possible() {
    // ADR 0046, decision 3, "fail closed": the composition root gives a broker that refuses
    let valid = load(&file("  enabled: true\n"), &env()).unwrap();
    assert!(valid.config.user_tool_servers.as_ref().unwrap().enabled);
    assert!(valid.config.secrets.is_none());
}

#[test]
fn the_feature_on_with_a_secrets_section_resolves_its_keys_as_before() {
    let text = format!(
        "{HEAD}userToolServers:\n  enabled: true\nsecrets:\n  store: pg\n  pg:\n    keys: [{{ id: k1, key: {{ env: K1 }} }}]\n    current: k1\n"
    );
    let valid = load(
        &text,
        &env().env(
            "K1",
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        ),
    )
    .unwrap();
    assert_eq!(valid.secrets.user_secret_keys.len(), 1);
}

// ---- the permission ---------------------------------------------------------------------------

#[test]
fn tools_user_add_is_a_permission_a_role_lists_to_hold() {
    let valid = load(
        &format!(
            "{HEAD}auth:
  roles:
    maker: {{ permissions: [agent.read, agent.invoke, thread.read, thread.write, tools.user.add] }}
    plain: {{ permissions: [agent.read, agent.invoke, thread.read, thread.write] }}
    root: {{ permissions: [admin] }}
  defaultRole: plain
"
        ),
        &env(),
    )
    .unwrap();
    let roles = valid.config.auth.roles.as_ref().unwrap();
    assert!(
        roles["maker"]
            .permissions
            .contains(&AuthPermission::ToolsUserAdd)
    );
    assert!(
        !roles["plain"]
            .permissions
            .contains(&AuthPermission::ToolsUserAdd)
    );
    assert!(
        !roles["root"]
            .permissions
            .contains(&AuthPermission::ToolsUserAdd),
        "admin does not imply it"
    );
    assert_eq!(AuthPermission::ToolsUserAdd.as_str(), "tools.user.add");
}

#[test]
fn a_role_that_holds_only_tools_user_add_takes_no_scope_and_no_agents() {
    // it takes no scope and is not over agents: the keys would do nothing
    let errors = lines(load(
        &format!(
            "{HEAD}auth:
  roles:
    maker: {{ permissions: [tools.user.add], scope: own, agents: [chat] }}
"
        ),
        &env(),
    ));
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors
            .iter()
            .any(|l| l.starts_with("auth.roles.maker.scope: ")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|l| l.starts_with("auth.roles.maker.agents: ")),
        "{errors:?}"
    );

    // twice is an error, as for every permission
    let errors = lines(load(
        &format!(
            "{HEAD}auth:\n  roles:\n    maker: {{ permissions: [tools.user.add, tools.user.add] }}\n"
        ),
        &env(),
    ));
    assert_eq!(
        errors,
        ["auth.roles.maker.permissions[1]: tools.user.add is listed twice"]
    );
}
