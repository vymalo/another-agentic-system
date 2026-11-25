//! The integrity rule (ADR 0087, the note of 2026-11-25): before the orchestrator signs, what goes into `user` must be what
//! the AG-UI request held. Each member is changed once, and the statement is refused with nothing to sign.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_prompt_statement::{
    A2aIds, Field, JTI_LEN, MAX_LIFETIME_SECS, Received, Source, Statement, StatementError,
    UserClaims, hash_of,
};
use serde_json::{Value, json};

const ENVELOPE: &str = "eyJhbGciOiJFZERTQSJ9..c2lnbmF0dXJl";

fn content() -> Value {
    json!([{ "type": "text", "text": "Run the tests again." }])
}

fn props() -> Value {
    json!({ "vymalo": { "accessTier": "read" }, "maxTurns": 12, "temperature": 0.25 })
}

fn received<'a>(content: &'a Value, props: &'a Value) -> Received<'a> {
    Received {
        thread: "thread-1",
        agent: "adam",
        msg: "msg-1",
        content,
        forwarded_props: Some(props),
    }
}

fn statement(user: Option<UserClaims>) -> Statement {
    Statement {
        aud: "https://models.test.invalid".into(),
        iat: 1_795_000_000,
        exp: 1_795_000_000 + 3600,
        jti: [7; JTI_LEN],
        src: Source::Chat,
        sub: "person-1".into(),
        azp: "app-client".into(),
        thread: "thread-1".into(),
        agent: "coder".into(),
        a2a: A2aIds {
            context_id: "ctx-1".into(),
            message_id: "a2a-msg-1".into(),
        },
        content_sha256: hash_of(&json!([{ "text": "Run the tests again." }])),
        user,
    }
}

fn bound_user() -> UserClaims {
    let (c, p) = (content(), props());
    UserClaims::from_request("https://api.test.invalid", &received(&c, &p), ENVELOPE)
}

fn refused(user: UserClaims, c: &Value, p: &Value) -> StatementError {
    statement(Some(user))
        .attest(Some(&received(c, p)))
        .unwrap_err()
}

#[test]
fn a_user_bound_to_the_request_is_attested() {
    let (c, p) = (content(), props());
    let payload = statement(Some(bound_user()))
        .attest(Some(&received(&c, &p)))
        .unwrap();
    let value: Value = serde_json::from_slice(payload.as_bytes()).unwrap();
    assert_eq!(value["user"]["thread"], "thread-1");
    assert_eq!(value["user"]["props_sha256"], hash_of(&props()));
}

#[test]
fn a_changed_content_is_refused() {
    let (p, other) = (
        props(),
        json!([{ "type": "text", "text": "Delete the branch." }]),
    );
    let error = refused(bound_user(), &other, &p);
    assert_eq!(error, StatementError::Mismatch(Field::ContentSha256));
    assert!(error.is_prompt_signature_invalid());
}

#[test]
fn a_changed_content_hash_in_user_is_refused() {
    let (c, p) = (content(), props());
    let mut user = bound_user();
    user.content_sha256 = hash_of(&json!("something else"));
    assert_eq!(
        refused(user, &c, &p),
        StatementError::Mismatch(Field::ContentSha256)
    );
}

#[test]
fn a_changed_option_is_refused() {
    let c = content();
    let other = json!({ "vymalo": { "accessTier": "full" }, "maxTurns": 12, "temperature": 0.25 });
    assert_eq!(
        refused(bound_user(), &c, &other),
        StatementError::Mismatch(Field::PropsSha256)
    );
}

#[test]
fn a_changed_props_hash_in_user_is_refused() {
    let (c, p) = (content(), props());
    let mut user = bound_user();
    user.props_sha256 = hash_of(&json!({}));
    assert_eq!(
        refused(user, &c, &p),
        StatementError::Mismatch(Field::PropsSha256)
    );
}

#[test]
fn a_changed_thread_agent_or_message_is_refused() {
    let (c, p) = (content(), props());
    type Change = fn(&mut UserClaims);
    let cases: [(Change, Field); 3] = [
        (|u| u.thread = "thread-2".into(), Field::Thread),
        (|u| u.agent = "coder".into(), Field::Agent),
        (|u| u.msg = "msg-2".into(), Field::Msg),
    ];
    for (change, field) in cases {
        let mut user = bound_user();
        change(&mut user);
        let error = refused(user, &c, &p);
        assert_eq!(error, StatementError::Mismatch(field));
        assert_eq!(error.code(), Some("prompt_signature_invalid"));
        assert_eq!(error.client_reason(), Some("invalid"));
        assert_eq!(error.log_reason(), "mismatch");
    }
}

#[test]
fn a_user_with_no_request_to_compare_with_is_refused() {
    let error = statement(Some(bound_user())).attest(None).unwrap_err();
    assert_eq!(error, StatementError::Unbound);
    assert!(error.is_prompt_signature_invalid());
}

#[test]
fn no_user_needs_no_request() {
    let payload = statement(None).attest(None).unwrap();
    let value: Value = serde_json::from_slice(payload.as_bytes()).unwrap();
    assert_eq!(value["user"], Value::Null);
    assert_eq!(value["src"], "chat");
}

#[test]
fn a_lifetime_over_a_day_or_not_after_iat_is_refused_and_is_not_the_persons_fault() {
    for exp in [
        1_795_000_000,
        1_794_999_999,
        1_795_000_000 + MAX_LIFETIME_SECS + 1,
    ] {
        let mut st = statement(None);
        st.exp = exp;
        let error = st.attest(None).unwrap_err();
        assert_eq!(error, StatementError::Lifetime);
        assert!(!error.is_prompt_signature_invalid());
        assert_eq!(error.code(), None);
    }
    let mut st = statement(None);
    st.exp = st.iat + MAX_LIFETIME_SECS;
    assert!(st.attest(None).is_ok());
}

#[test]
fn every_source_is_written_as_the_contract_names_it() {
    for (src, name) in [
        (Source::Chat, "chat"),
        (Source::Resume, "resume"),
        (Source::Schedule, "schedule"),
        (Source::Webhook, "webhook"),
        (Source::Mcp, "mcp"),
    ] {
        let mut st = statement(None);
        st.src = src;
        let value: Value = serde_json::from_slice(st.attest(None).unwrap().as_bytes()).unwrap();
        assert_eq!(value["src"], name);
        assert_eq!(src.as_str(), name);
    }
}

#[test]
fn the_payload_holds_exactly_the_contracts_members() {
    let (c, p) = (content(), props());
    let payload = statement(Some(bound_user()))
        .attest(Some(&received(&c, &p)))
        .unwrap();
    let value: Value = serde_json::from_slice(payload.as_bytes()).unwrap();
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "a2a",
            "agent",
            "aud",
            "azp",
            "content_sha256",
            "exp",
            "iat",
            "jti",
            "src",
            "sub",
            "thread",
            "typ",
            "user"
        ]
    );
    let mut user_keys: Vec<_> = value["user"].as_object().unwrap().keys().cloned().collect();
    user_keys.sort();
    assert_eq!(
        user_keys,
        [
            "agent",
            "aud",
            "content_sha256",
            "env_sha256",
            "msg",
            "props_sha256",
            "thread"
        ]
    );
    assert_eq!(value["jti"], "BwcHBwcHBwcHBwcHBwcHBw");
}

#[test]
fn the_payloads_debug_shows_its_length_and_not_the_person() {
    let payload = statement(None).attest(None).unwrap();
    let debug = format!("{payload:?}");
    assert!(!debug.contains("person-1"), "{debug}");
    assert!(
        debug.contains(&payload.as_bytes().len().to_string()),
        "{debug}"
    );
}
