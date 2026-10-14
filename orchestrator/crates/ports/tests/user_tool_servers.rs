//! The definitions of a person's MCP servers (ADR 0046, decision 2), through the public API: the
//! id, the four kinds of authentication, the header names that are refused, the state and what a
//! definition may hold.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{Classify, ErrorClass};
use orch_ports::{
    CredentialRef, DefinitionError, MAX_USER_TOOL_SERVERS, ToolServerStatus, UnavailableReason,
    UserAuth, UserToolServer, UserToolServerId, UserToolServerState, is_reserved_header,
};

// ------------------------------------------------------------------------------------------ id

#[test]
fn an_id_is_u_dash_and_ten_lowercase_letters_or_digits() {
    for good in [
        "u-abcdefghij",
        "u-0123456789",
        "u-a1b2c3d4e5",
        "u-zzzzzzzzzz",
    ] {
        let id = UserToolServerId::parse(good).expect(good);
        assert_eq!(id.as_str(), good);
        assert_eq!(id.to_string(), good);
        assert_eq!(good.parse::<UserToolServerId>().unwrap(), id);
    }
    for bad in [
        "",
        "u-",
        "u-abc",
        "u-abcdefghi",
        "u-abcdefghijk",
        "U-abcdefghij",
        "u_abcdefghij",
        "x-abcdefghij",
        "abcdefghijkl",
        "u-ABCDEFGHIJ",
        "u-abcdefghi!",
        "u-abcdefghi-",
        "u-abcdefghi ",
        " u-abcdefghij",
        "u-abcdefghij\n",
        "u-abcdefghi\u{e9}",
        "u-abcdefgh\u{ff21}",
        "u-abcdefghij#flow",
        "u--bcdefghij",
        "u-../../etc/",
    ] {
        assert!(UserToolServerId::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn an_id_is_made_of_the_entropy_it_is_given() {
    let a = UserToolServerId::from_entropy([0; 16]);
    let b = UserToolServerId::from_entropy([0; 16]);
    let c = UserToolServerId::from_entropy([1; 16]);
    assert_eq!(a, b);
    assert_ne!(a, c);
    for entropy in [[0; 16], [0xff; 16], [7; 16], *b"0123456789abcdef"] {
        let id = UserToolServerId::from_entropy(entropy);
        assert_eq!(UserToolServerId::parse(id.as_str()).unwrap(), id);
    }
}

#[test]
fn a_generated_id_is_valid_and_not_repeated() {
    let ids: Vec<UserToolServerId> = (0..2000).map(|_| UserToolServerId::generate()).collect();
    for id in &ids {
        assert_eq!(UserToolServerId::parse(id.as_str()).as_ref(), Ok(id));
    }
    let mut sorted: Vec<&str> = ids.iter().map(UserToolServerId::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
    // all 36 symbols turn up in 20 thousand characters: the alphabet is not cut short
    let seen: std::collections::BTreeSet<char> =
        ids.iter().flat_map(|id| id.as_str()[2..].chars()).collect();
    assert_eq!(seen.len(), 36, "{seen:?}");
}

#[test]
fn an_id_reads_and_writes_as_its_text() {
    let id = UserToolServerId::parse("u-abcdefghij").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"u-abcdefghij\"");
    let back: UserToolServerId = serde_json::from_str("\"u-abcdefghij\"").unwrap();
    assert_eq!(back, id);
    // reading an id is checking it
    assert!(serde_json::from_str::<UserToolServerId>("\"u-abc\"").is_err());
    assert!(serde_json::from_str::<UserToolServerId>("\"deployment-id\"").is_err());
}

// -------------------------------------------------------------------------------- the headers

#[test]
fn an_api_key_header_is_a_header_name_the_orchestrator_does_not_own() {
    for good in [
        "X-Api-Key",
        "x-api-key",
        "Api-Key",
        "X-Custom-Token-2",
        "apikey",
        "X-GitHub-Token",
        // a name that starts like an owned one is not one, apart from `Mcp-`
        "Mcp",
        "Acceptable",
        "Hostname",
        "Content-Typed",
        "Last-Event-Id-2",
    ] {
        assert!(UserAuth::api_key(good).is_ok(), "{good}");
    }
    for owned in [
        "Host",
        "host",
        "HOST",
        "Accept",
        "aCCept",
        "Content-Type",
        "content-type",
        "CONTENT-TYPE",
        "Last-Event-ID",
        "last-event-id",
        "Mcp-Session-Id",
        "Mcp-Protocol-Version",
        "mcp-protocol-version",
        "MCP-Anything",
        "mcp-x",
        "Mcp-",
        // the bearer kind is where Authorization is set
        "Authorization",
        "authorization",
    ] {
        assert_eq!(
            UserAuth::api_key(owned).unwrap_err(),
            DefinitionError::ReservedHeader,
            "{owned}"
        );
    }
}

#[test]
fn an_api_key_header_must_be_a_header_name() {
    for bad in [
        "",
        " ",
        "X Api Key",
        "X-Api-Key:",
        "X-Api-Key\r\nHost: evil",
        "X-Api-Key\n",
        "X_Api_Key",
        "X.Api.Key",
        "caf\u{e9}",
        "X-Ap\u{ef}-Key",
        "X-Api-Key ",
        &"A".repeat(65),
    ] {
        assert_eq!(
            UserAuth::api_key(bad).unwrap_err(),
            DefinitionError::BadHeader,
            "{bad:?}"
        );
    }
    assert!(UserAuth::api_key(&"A".repeat(64)).is_ok());
}

#[test]
fn the_reserved_header_check_is_the_one_the_tool_client_uses() {
    for owned in [
        "authorization",
        "Accept",
        "CONTENT-TYPE",
        "host",
        "Mcp-Session-Id",
        "mcp-protocol-version",
        "Last-Event-ID",
    ] {
        assert!(is_reserved_header(owned), "{owned}");
    }
    for free in ["X-Api-Key", "Mcp-Other", "Accepts", "Hostile", ""] {
        assert!(!is_reserved_header(free), "{free}");
    }
}

// --------------------------------------------------------------------------------- the kinds

#[test]
fn the_four_kinds_and_what_each_holds() {
    let none = UserAuth::None;
    let bearer = UserAuth::Bearer;
    let key = UserAuth::api_key("X-Api-Key").unwrap();
    let oauth = UserAuth::oauth2(
        vec!["repo".into(), "read:org".into()],
        Some("client-1".into()),
    )
    .unwrap();

    assert_eq!(
        [none.kind(), bearer.kind(), key.kind(), oauth.kind()],
        ["none", "bearer", "api_key", "oauth2"]
    );
    assert!(!none.needs_credential());
    assert!(bearer.needs_credential() && key.needs_credential() && oauth.needs_credential());

    assert_eq!(key.header_name(), Some("X-Api-Key"));
    assert_eq!(none.header_name(), None);
    assert_eq!(bearer.header_name(), None);
    assert_eq!(oauth.header_name(), None);
    assert_eq!(oauth.scopes(), ["repo", "read:org"]);
    assert_eq!(oauth.client_id(), Some("client-1"));
    assert!(key.scopes().is_empty() && key.client_id().is_none());
}

#[test]
fn the_kinds_are_written_as_the_adr_names_them() {
    let cases = [
        (UserAuth::None, r#"{"kind":"none"}"#),
        (UserAuth::Bearer, r#"{"kind":"bearer"}"#),
        (
            UserAuth::api_key("X-Api-Key").unwrap(),
            r#"{"kind":"api_key","header":"X-Api-Key"}"#,
        ),
        (
            UserAuth::oauth2(vec!["repo".into()], Some("c1".into())).unwrap(),
            r#"{"kind":"oauth2","scopes":["repo"],"client_id":"c1"}"#,
        ),
        (
            UserAuth::oauth2(Vec::new(), None).unwrap(),
            r#"{"kind":"oauth2","scopes":[],"client_id":null}"#,
        ),
    ];
    for (auth, json) in cases {
        assert_eq!(serde_json::to_string(&auth).unwrap(), json);
        assert_eq!(serde_json::from_str::<UserAuth>(json).unwrap(), auth);
    }
    assert!(serde_json::from_str::<UserAuth>(r#"{"kind":"basic"}"#).is_err());
}

#[test]
fn a_kind_read_from_storage_is_checked_before_it_is_used() {
    // reading is not trusting: a definition is built through `new`, which checks the kind
    let stored: UserAuth =
        serde_json::from_str(r#"{"kind":"api_key","header":"Host"}"#).expect("it reads");
    let id = UserToolServerId::parse("u-abcdefghij").unwrap();
    let error = UserToolServer::new(id, "n", "https://x.example/mcp", stored).unwrap_err();
    assert_eq!(error, DefinitionError::ReservedHeader);
}

#[test]
fn scopes_are_oauth_scope_tokens() {
    assert!(
        UserAuth::oauth2(
            vec!["repo".into(), "read:org".into(), "a.b/c-d_e".into()],
            None
        )
        .is_ok()
    );
    for bad in [
        "",
        "two words",
        "with\"quote",
        "back\\slash",
        "tab\t",
        "caf\u{e9}",
        "line\n",
    ] {
        assert_eq!(
            UserAuth::oauth2(vec![bad.to_owned()], None).unwrap_err(),
            DefinitionError::BadScope,
            "{bad:?}"
        );
    }
    assert!(UserAuth::oauth2(vec!["s".repeat(128)], None).is_ok());
    assert_eq!(
        UserAuth::oauth2(vec!["s".repeat(129)], None).unwrap_err(),
        DefinitionError::BadScope
    );
    assert!(UserAuth::oauth2((0..32).map(|n| format!("s{n}")).collect(), None).is_ok());
    assert_eq!(
        UserAuth::oauth2((0..33).map(|n| format!("s{n}")).collect(), None).unwrap_err(),
        DefinitionError::TooManyScopes
    );
}

#[test]
fn a_client_id_is_visible_ascii() {
    assert!(UserAuth::oauth2(Vec::new(), Some("Iv1.8a61f9b3a7aba766".into())).is_ok());
    for bad in ["", "two words", "caf\u{e9}", "line\n"] {
        assert_eq!(
            UserAuth::oauth2(Vec::new(), Some(bad.to_owned())).unwrap_err(),
            DefinitionError::BadClientId,
            "{bad:?}"
        );
    }
    assert!(UserAuth::oauth2(Vec::new(), Some("c".repeat(256))).is_ok());
    assert_eq!(
        UserAuth::oauth2(Vec::new(), Some("c".repeat(257))).unwrap_err(),
        DefinitionError::BadClientId
    );
}

// ----------------------------------------------------------------------------- the definition

fn id() -> UserToolServerId {
    UserToolServerId::parse("u-abcdefghij").unwrap()
}

#[test]
fn a_new_definition_has_been_added_and_has_no_credential() {
    let definition = UserToolServer::new(
        id(),
        "My GitHub",
        "https://mcp.example.com/mcp",
        UserAuth::Bearer,
    )
    .unwrap();
    assert_eq!(definition.id(), &id());
    assert_eq!(definition.name(), "My GitHub");
    assert_eq!(definition.url(), "https://mcp.example.com/mcp");
    assert_eq!(definition.auth(), &UserAuth::Bearer);
    assert_eq!(definition.state(), UserToolServerState::Added);
    assert!(definition.credential().is_none());

    let connected = definition
        .with_state(UserToolServerState::Connected)
        .with_credential(CredentialRef::new("cred-1").unwrap());
    assert_eq!(connected.state(), UserToolServerState::Connected);
    assert_eq!(connected.credential().unwrap().as_str(), "cred-1");
}

#[test]
fn a_name_is_one_to_eighty_characters_without_control_characters() {
    let make =
        |name: &str| UserToolServer::new(id(), name, "https://x.example/mcp", UserAuth::None);
    assert_eq!(make("  My GitHub  ").unwrap().name(), "My GitHub");
    assert!(make("名前のサーバー").is_ok());
    assert!(make(&"n".repeat(80)).is_ok());
    for bad in [
        "",
        "   ",
        "line\nbreak",
        "tab\there",
        "bell\u{7}",
        &"n".repeat(81),
    ] {
        assert_eq!(make(bad).unwrap_err(), DefinitionError::BadName, "{bad:?}");
    }
    // eighty characters, not eighty bytes
    assert!(make(&"é".repeat(80)).is_ok());
}

#[test]
fn a_url_is_printable_ascii_of_at_most_two_kib() {
    let make = |url: &str| UserToolServer::new(id(), "n", url, UserAuth::None);
    assert!(make("https://mcp.example.com/mcp").is_ok());
    assert!(make(&format!("https://x.example/{}", "a".repeat(2000))).is_ok());
    for bad in [
        "",
        "https://x.example/ mcp",
        "https://x.example/\n",
        "https://x.example/caf\u{e9}",
        &format!("https://x.example/{}", "a".repeat(2048)),
    ] {
        assert_eq!(make(bad).unwrap_err(), DefinitionError::BadUrl, "{bad:?}");
    }
}

#[test]
fn a_definition_does_not_print_a_secret_it_never_had() {
    // it holds an id of a credential and no secret: the whole definition can be shown
    let definition = UserToolServer::new(id(), "n", "https://x.example/mcp", UserAuth::Bearer)
        .unwrap()
        .with_credential(CredentialRef::new("cred-1").unwrap());
    let text = format!("{definition:?}");
    assert!(
        text.contains("cred-1") && text.contains("u-abcdefghij"),
        "{text}"
    );
}

#[test]
fn every_definition_error_is_the_senders_mistake() {
    for error in [
        DefinitionError::BadId,
        DefinitionError::BadName,
        DefinitionError::BadUrl,
        DefinitionError::BadHeader,
        DefinitionError::ReservedHeader,
        DefinitionError::BadScope,
        DefinitionError::TooManyScopes,
        DefinitionError::BadClientId,
        DefinitionError::BadCredentialRef,
        DefinitionError::BadState,
    ] {
        assert_eq!(error.class(), ErrorClass::Invalid, "{error}");
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn a_credential_reference_is_an_opaque_id_of_visible_ascii() {
    assert_eq!(
        CredentialRef::new("cr-0123abcd").unwrap().as_str(),
        "cr-0123abcd"
    );
    assert!(CredentialRef::new(&"c".repeat(128)).is_ok());
    for bad in ["", "two words", "caf\u{e9}", "line\n", &"c".repeat(129)] {
        assert_eq!(
            CredentialRef::new(bad).unwrap_err(),
            DefinitionError::BadCredentialRef,
            "{bad:?}"
        );
    }
}

// -------------------------------------------------------------------------------- the state

#[test]
fn the_states_are_the_lifecycle_of_the_adr() {
    let all = [
        (UserToolServerState::Added, "added"),
        (UserToolServerState::Connected, "connected"),
        (UserToolServerState::NeedsReauth, "needs_reauth"),
        (UserToolServerState::Revoked, "revoked"),
    ];
    for (state, text) in all {
        assert_eq!(state.as_str(), text);
        assert_eq!(text.parse::<UserToolServerState>().unwrap(), state);
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            format!("\"{text}\"")
        );
        assert_eq!(
            serde_json::from_str::<UserToolServerState>(&format!("\"{text}\"")).unwrap(),
            state
        );
    }
    assert!("unavailable".parse::<UserToolServerState>().is_err());
    assert!("Connected".parse::<UserToolServerState>().is_err());
}

#[test]
fn only_a_connected_server_is_offered() {
    for (state, offered) in [
        (UserToolServerState::Added, false),
        (UserToolServerState::Connected, true),
        (UserToolServerState::NeedsReauth, false),
        (UserToolServerState::Revoked, false),
    ] {
        assert_eq!(state.is_offered(), offered, "{state:?}");
    }
}

#[test]
fn the_status_a_person_sees_is_ready_needs_auth_or_unavailable() {
    assert_eq!(
        UserToolServerState::Connected.status(),
        ToolServerStatus::Ready
    );
    for state in [
        UserToolServerState::Added,
        UserToolServerState::NeedsReauth,
        UserToolServerState::Revoked,
    ] {
        assert_eq!(state.status(), ToolServerStatus::NeedsAuth, "{state:?}");
    }
    // `unavailable` is never stored: the API says it, with the reason
    let down = ToolServerStatus::Unavailable(UnavailableReason::ServerDown);
    assert_eq!(down.as_str(), "unavailable");
    assert_eq!(ToolServerStatus::Ready.as_str(), "ready");
    assert_eq!(ToolServerStatus::NeedsAuth.as_str(), "needs_auth");
    assert_eq!(down.reason(), Some(UnavailableReason::ServerDown));
    assert_eq!(ToolServerStatus::Ready.reason(), None);
    let reasons = [
        (UnavailableReason::NotPermitted, "not_permitted"),
        (UnavailableReason::EgressRefused, "egress_refused"),
        (UnavailableReason::ServerDown, "server_down"),
        (UnavailableReason::BrokerDown, "broker_down"),
    ];
    for (reason, text) in reasons {
        assert_eq!(reason.as_str(), text);
    }
}

#[test]
fn the_limit_is_eight_per_person() {
    assert_eq!(MAX_USER_TOOL_SERVERS, 8);
}
