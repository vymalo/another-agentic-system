//! `userToolServers` (ADR 0046, slice 4a): whether people may add MCP servers of their own, what the
//! orchestrator may connect to for them, and the OAuth client it presents. Off when absent; every
//! mistake listed at once and naming the key; the id rule that only the feature being on makes; and
//! the permission `tools.user.add`, which no role holds by default.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{
    AuthPermission, DEFAULT_OAUTH_CLIENT_NAME, MAX_EGRESS_PATTERNS, OAUTH_CALLBACK_PATH,
};
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
    assert_eq!(section.oauth.callback_url, None);
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
        "{HEAD}server:\n  publicUrl: https://chat.example.com\nuserToolServers:\n  enabled: true\nsecrets:\n  store: pg\n  pg:\n    keys: [{{ id: k1, key: {{ env: K1 }} }}]\n    current: k1\n"
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

// ---- the OAuth callback and the document (slice 4c) --------------------------------------------

/// The keys of `secrets`, so that credentials are kept and an `oauth2` server can be authorized.
const SECRETS: &str =
    "secrets:\n  store: pg\n  pg:\n    keys: [{ id: k1, key: { env: K1 } }]\n    current: k1\n";

fn with_keys() -> Fake {
    env().env(
        "K1",
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    )
}

/// A file with `server` (the lines under `server:`, none for no section) and `oauth` (the lines under
/// `userToolServers.oauth`, none for no key), the feature on and credentials kept.
fn oauth_file(server: &str, oauth: &str) -> String {
    let server = if server.is_empty() {
        String::new()
    } else {
        format!("server:\n{server}")
    };
    let oauth = if oauth.is_empty() {
        String::new()
    } else {
        format!("  oauth:\n{oauth}")
    };
    format!("{HEAD}{server}userToolServers:\n  enabled: true\n{oauth}{SECRETS}")
}

#[test]
fn the_callback_is_the_public_url_and_the_path_unless_a_url_is_given() {
    assert_eq!(OAUTH_CALLBACK_PATH, "/api/user-tool-servers/oauth/callback");
    let derived = load(
        &oauth_file("  publicUrl: https://chat.example.com\n", ""),
        &with_keys(),
    )
    .unwrap();
    assert_eq!(
        derived.config.user_tool_callback_url().as_deref(),
        Some("https://chat.example.com/api/user-tool-servers/oauth/callback")
    );
    // an explicit address wins, and does not need the public URL
    let given = load(
        &oauth_file(
            "",
            "    callbackUrl: https://api.example.com/api/user-tool-servers/oauth/callback\n",
        ),
        &with_keys(),
    )
    .unwrap();
    assert_eq!(
        given.config.user_tool_callback_url().as_deref(),
        Some("https://api.example.com/api/user-tool-servers/oauth/callback")
    );
    let both = load(
        &oauth_file(
            "  publicUrl: https://chat.example.com\n",
            "    callbackUrl: https://api.example.com/api/user-tool-servers/oauth/callback\n",
        ),
        &with_keys(),
    )
    .unwrap();
    assert_eq!(
        both.config.user_tool_callback_url().as_deref(),
        Some("https://api.example.com/api/user-tool-servers/oauth/callback")
    );
    // nothing to derive it from
    let none = load(&file("  {}\n"), &env()).unwrap();
    assert_eq!(none.config.user_tool_callback_url(), None);
}

#[test]
fn credentials_kept_need_an_address_for_the_browser_to_come_back_to() {
    let errors = lines(load(&oauth_file("", ""), &with_keys()));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("userToolServers.oauth.callbackUrl: required when"),
        "{errors:?}"
    );
    // either way of saying it is enough
    assert!(
        load(
            &oauth_file("  publicUrl: https://chat.example.com\n", ""),
            &with_keys()
        )
        .is_ok()
    );
    assert!(
        load(
            &oauth_file(
                "",
                "    callbackUrl: https://chat.example.com/api/user-tool-servers/oauth/callback\n"
            ),
            &with_keys()
        )
        .is_ok()
    );
    // nothing is kept with no secrets section, so nothing is authorized and nothing is required
    assert!(load(&file("  enabled: true\n"), &env()).is_ok());
    // and the feature off asks for nothing
    let off = format!("{HEAD}userToolServers:\n  enabled: false\n{SECRETS}");
    assert!(load(&off, &with_keys()).is_ok());
}

#[test]
fn a_public_url_that_oauth_does_not_take_as_a_redirect_is_named_when_credentials_are_kept() {
    // the name `localhost` can be pointed elsewhere: OAuth takes the loopback address
    let errors = lines(load(
        &oauth_file("  publicUrl: http://localhost:3000\n", ""),
        &with_keys(),
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("server.publicUrl:"), "{errors:?}");
    // an explicit callback settles it
    assert!(
        load(
            &oauth_file(
                "  publicUrl: http://localhost:3000\n",
                "    callbackUrl: http://127.0.0.1:3000/api/user-tool-servers/oauth/callback\n"
            ),
            &with_keys()
        )
        .is_ok()
    );
    // and so does the loopback address
    assert!(
        load(
            &oauth_file("  publicUrl: http://127.0.0.1:8080\n", ""),
            &with_keys()
        )
        .is_ok()
    );
    assert!(
        load(
            &oauth_file("  publicUrl: http://[::1]:8080\n", ""),
            &with_keys()
        )
        .is_ok()
    );
    // a public URL other keys use is not judged by this rule while no credentials are kept
    assert!(
        load(
            &format!("{HEAD}server:\n  publicUrl: http://localhost:3000\nuserToolServers:\n  enabled: true\n"),
            &env()
        )
        .is_ok()
    );
}

#[test]
fn the_callback_url_is_one_a_browser_may_come_back_to() {
    let good = [
        "https://chat.example.com/api/user-tool-servers/oauth/callback",
        "https://chat.example.com:8443/api/user-tool-servers/oauth/callback",
        "http://127.0.0.1:8080/api/user-tool-servers/oauth/callback",
        "http://[::1]:8080/api/user-tool-servers/oauth/callback",
        // the path is the one the URL has once it is read, which is the one the broker sends
        "https://chat.example.com/api/./user-tool-servers/oauth/../oauth/callback",
    ];
    for url in good {
        assert!(
            load(
                &oauth_file("", &format!("    callbackUrl: {url}\n")),
                &with_keys()
            )
            .is_ok(),
            "{url}"
        );
    }
    let bad = [
        "http://chat.example.com/cb",
        "http://localhost:8080/cb",
        "http://127.0.0.2:8080/cb",
        "ftp://chat.example.com/cb",
        "https://chat.example.com/cb?x=1",
        "https://chat.example.com/cb#frag",
        "https://user:pw@chat.example.com/cb",
        "https://chat.example.com:0/cb",
        "not a url",
        "/relative/cb",
    ];
    for url in bad {
        let errors = lines(load(
            &oauth_file("", &format!("    callbackUrl: \"{url}\"\n")),
            &with_keys(),
        ));
        assert_eq!(errors.len(), 1, "{url}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.oauth.callbackUrl: expected an https URL"),
            "{url}: {errors:?}"
        );
        assert!(!errors[0].contains("pw@"), "never echoed: {errors:?}");
    }
}

/// The router mounts the callback at one path and nowhere else (`axum` matches exactly), so a
/// `callbackUrl` with any other path would have every authorization end at a 404 and the Client ID
/// Metadata Document list a redirect that nothing serves. The key may move the **origin** (an API
/// host of its own), never the path (ADR 0046, slice 4c review).
#[test]
fn a_callback_url_whose_path_is_not_the_served_one_is_refused_and_the_expected_path_is_named() {
    let bad = [
        "https://api.example.com/cb",
        "https://api.example.com/",
        "https://api.example.com",
        "https://api.example.com/oauth/callback",
        // axum does not redirect, fold the case or decode: each of these is a 404 there
        "https://api.example.com/api/user-tool-servers/oauth/callback/",
        "https://api.example.com/API/user-tool-servers/oauth/callback",
        "https://api.example.com/api/user-tool-servers/oauth%2Fcallback",
        "https://api.example.com/api/user-tool-servers/oauth/callbackx",
        "https://api.example.com/x/api/user-tool-servers/oauth/callback",
        "https://api.example.com//api/user-tool-servers/oauth/callback",
        "http://127.0.0.1:8080/cb",
    ];
    for url in bad {
        let errors = lines(load(
            &oauth_file("", &format!("    callbackUrl: \"{url}\"\n")),
            &with_keys(),
        ));
        assert_eq!(errors.len(), 1, "{url}: {errors:?}");
        assert!(
            errors[0].starts_with(
                "userToolServers.oauth.callbackUrl: its path must be \
                 /api/user-tool-servers/oauth/callback"
            ),
            "{url}: {errors:?}"
        );
    }
    // it is the shape that is refused first when there is no URL to speak of
    let errors = lines(load(
        &oauth_file(
            "",
            "    callbackUrl: \"/api/user-tool-servers/oauth/callback\"\n",
        ),
        &with_keys(),
    ));
    assert!(
        errors[0].starts_with("userToolServers.oauth.callbackUrl: expected an https URL"),
        "{errors:?}"
    );
    // the check does not depend on credentials being kept, nor on the public URL
    for text in [
        file("  enabled: true\n  oauth:\n    callbackUrl: https://api.example.com/cb\n"),
        oauth_file(
            "  publicUrl: https://chat.example.com\n",
            "    callbackUrl: https://api.example.com/cb\n",
        ),
    ] {
        let errors = lines(load(&text, &with_keys()));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.oauth.callbackUrl: its path must be"),
            "{errors:?}"
        );
    }
    // a public URL with a path of its own does not move the callback: the path is the route's
    let derived = load(
        &oauth_file("  publicUrl: https://chat.example.com/some/prefix\n", ""),
        &with_keys(),
    )
    .unwrap();
    assert_eq!(
        derived.config.user_tool_callback_url().as_deref(),
        Some("https://chat.example.com/api/user-tool-servers/oauth/callback")
    );
}

#[test]
fn production_takes_an_https_callback_only() {
    let production = |oauth: &str| {
        format!(
            "{HEAD}server:\n  environment: production\nauth:\n  mode: jwt\n  jwt: {{ issuer: \"https://idp.example/realms/main\", audiences: [web] }}\nuserToolServers:\n  enabled: true\n  oauth:\n{oauth}{SECRETS}"
        )
    };
    assert!(
        load(
            &production(
                "    callbackUrl: https://chat.example.com/api/user-tool-servers/oauth/callback\n"
            ),
            &with_keys()
        )
        .is_ok()
    );
    let errors = lines(load(
        &production("    callbackUrl: http://127.0.0.1:8080/cb\n"),
        &with_keys(),
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .starts_with("userToolServers.oauth.callbackUrl: expected an https URL with a host"),
        "{errors:?}"
    );
    assert!(
        !errors[0].contains("127.0.0.1"),
        "production does not offer the loopback: {errors:?}"
    );
}

#[test]
fn the_document_is_served_on_a_path_nothing_else_is_served_at() {
    let doc = |path: &str| {
        load(
            &oauth_file(
                "  publicUrl: https://chat.example.com\n",
                &format!("    clientMetadataUrl: https://chat.example.com{path}\n"),
            ),
            &with_keys(),
        )
    };
    for ok in [
        "/oauth/client-metadata.json",
        "/.well-known/oauth-client/another-agentic-system",
        "/apiary/client.json",
        "/mcpx",
        "/s/doc",
    ] {
        assert!(doc(ok).is_ok(), "{ok}: {:?}", lines(doc(ok)));
    }
    for reserved in [
        "/api/public/oauth/client.json",
        "/api/x",
        "/agui/x",
        "/mcp/x",
        "/thread-tools/x",
        "/webhooks/x",
        "/healthz/x",
        "/readyz/x",
        "/metrics/x",
        "/oauth2/x",
    ] {
        let errors = lines(doc(reserved));
        assert_eq!(errors.len(), 1, "{reserved}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.oauth.clientMetadataUrl: its path is under"),
            "{reserved}: {errors:?}"
        );
    }
    // the path of a route is the route: `/mcp` itself, with no more
    assert_eq!(lines(doc("/mcp")).len(), 1);
}

#[test]
fn the_document_is_on_the_origin_of_the_callback() {
    let with = |server: &str, oauth: &str| load(&oauth_file(server, oauth), &with_keys());
    // the same origin as the derived callback
    assert!(
        with(
            "  publicUrl: https://chat.example.com\n",
            "    clientMetadataUrl: https://chat.example.com/oauth/c.json\n"
        )
        .is_ok()
    );
    // another host, another port, another scheme: the orchestrator would serve a document nobody asks for
    for other in [
        "https://docs.example.com/oauth/c.json",
        "https://chat.example.com:8443/oauth/c.json",
    ] {
        let errors = lines(with(
            "  publicUrl: https://chat.example.com\n",
            &format!("    clientMetadataUrl: {other}\n"),
        ));
        assert_eq!(errors.len(), 1, "{other}: {errors:?}");
        assert!(
            errors[0].starts_with("userToolServers.oauth.clientMetadataUrl: must be on the origin"),
            "{other}: {errors:?}"
        );
    }
    // an explicit callback is the origin the document is compared with
    assert!(
        with(
            "  publicUrl: https://chat.example.com\n",
            "    callbackUrl: https://api.example.com/api/user-tool-servers/oauth/callback\n    clientMetadataUrl: https://api.example.com/oauth/c.json\n"
        )
        .is_ok()
    );
    // with no callback known (the feature without credentials) there is nothing to compare
    assert!(
        load(
            &file("  enabled: true\n  oauth:\n    clientMetadataUrl: https://chat.example.com/oauth/c.json\n"),
            &env()
        )
        .is_ok()
    );
}
