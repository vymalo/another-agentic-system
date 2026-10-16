//! How the claims of a token whose signature verifies are read: `iss` is one text, the user is somebody, the
//! reason a token with several claims wrong is refused for is always the same one, and the leeway of the times
//! has an edge. Over real HTTP, against a server that serves an Ed25519 key, with tokens it signs.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use common::{
    AUDIENCE, USER, assert_accepted, b64, check, config, good_claims, now, refusal, serve_keys,
    token,
};
use orch_auth_jwt::{JwtAuth, LEEWAY_SECS};
use serde_json::{Map, Value, json};

/// An issuer with one Ed25519 key, an authenticator for it, and what signs its tokens.
struct Rig {
    auth: JwtAuth,
    issuer: String,
    key: Ed25519KeyPair,
}

impl Rig {
    async fn new() -> Self {
        Self::with(|config| config).await
    }

    async fn with(
        adjust: impl FnOnce(orch_auth_jwt::JwtConfig) -> orch_auth_jwt::JwtConfig,
    ) -> Self {
        let key = Ed25519KeyPair::generate().unwrap();
        let jwk = json!({"kty": "OKP", "crv": "Ed25519", "kid": "k", "x": b64(key.public_key().as_ref())});
        let issuer = serve_keys(vec![jwk]).await;
        let auth = JwtAuth::new(adjust(config(&issuer))).unwrap();
        Rig { auth, issuer, key }
    }

    fn claims(&self) -> Map<String, Value> {
        good_claims(&self.issuer)
    }

    fn mint(&self, claims: &Map<String, Value>) -> String {
        token("EdDSA", "k", claims, |m| self.key.sign(m).as_ref().to_vec())
    }

    async fn check(
        &self,
        claims: &Map<String, Value>,
    ) -> Result<orch_ports::Principal, orch_ports::AuthError> {
        check(&self.auth, &self.mint(claims)).await
    }
}

// ---- iss ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_issuer_is_one_text_equal_to_the_configured_one() {
    let rig = Rig::new().await;
    let issuer = rig.issuer.clone();
    assert_accepted(rig.check(&rig.claims()).await, "the issuer");
    let mut claims = rig.claims();
    for wrong in [
        json!(format!("{issuer}/")),
        json!(issuer.to_uppercase()),
        json!("https://evil.example"),
        json!(""),
    ] {
        claims.insert("iss".into(), wrong.clone());
        let why = refusal(rig.check(&claims).await, &format!("iss {wrong}"));
        assert!(why.contains("not for this API"), "iss {wrong}: {why}");
    }
    for not_text in [json!(7), json!(null), json!({"a": issuer})] {
        claims.insert("iss".into(), not_text.clone());
        refusal(rig.check(&claims).await, &format!("iss {not_text}"));
    }
    claims.remove("iss");
    let why = refusal(rig.check(&claims).await, "no iss");
    assert!(why.contains("\"iss\""), "{why}");
}

#[tokio::test]
async fn a_list_that_holds_the_issuer_is_not_the_issuer() {
    // The library accepts a list that holds the issuer, and so a token that names another issuer beside ours. An
    // `iss` is one text (RFC 7519 4.1.1: a StringOrURI).
    let rig = Rig::new().await;
    let issuer = rig.issuer.clone();
    let mut claims = rig.claims();
    for list in [
        json!([issuer, "https://evil.example"]),
        json!(["https://evil.example", issuer]),
        json!([issuer]),
        json!([]),
    ] {
        claims.insert("iss".into(), list.clone());
        let why = refusal(rig.check(&claims).await, &format!("iss {list}"));
        assert!(why.contains("not for this API"), "iss {list}: {why}");
    }
}

// ---- aud ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn one_of_the_audiences_must_be_among_aud() {
    let rig = Rig::with(|mut config| {
        config.audiences = vec![AUDIENCE.to_owned(), "second".to_owned()];
        config
    })
    .await;
    let mut claims = rig.claims();
    for fine in [
        json!(AUDIENCE),
        json!("second"),
        json!([AUDIENCE]),
        json!(["x", "second", "y"]),
    ] {
        claims.insert("aud".into(), fine.clone());
        assert_accepted(rig.check(&claims).await, &format!("aud {fine}"));
    }
    for wrong in [
        json!("someone-else"),
        json!(""),
        json!([]),
        json!(["x", "y"]),
        json!([AUDIENCE, 5]),
        json!(5),
        json!(null),
        json!({"a": AUDIENCE}),
    ] {
        claims.insert("aud".into(), wrong.clone());
        let result = rig.check(&claims).await;
        assert!(
            matches!(result, Err(orch_ports::AuthError::Invalid { .. })),
            "aud {wrong}: {result:?}"
        );
    }
    claims.remove("aud");
    let why = refusal(rig.check(&claims).await, "no aud");
    assert!(why.contains("\"aud\""), "{why}");
}

// ---- the user ----------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_user_that_is_blank_or_holds_a_control_character_is_a_missing_claim() {
    // The user is an e-mail (or an opaque subject) that is written to the log, a header and a row: one that is only
    // white space, or that a line of a log would have to escape, is not a person. Whichever claim names the user.
    for claim in ["email", "sub", "preferred_username"] {
        let rig = Rig::with(|config| config.with_user_claim(claim)).await;
        let mut claims = rig.claims();
        for bad in [
            "",
            "  ",
            " \t ",
            "\n",
            "a\nb\0",
            "a\u{7f}",
            "\u{85}",
            "\u{1b}[31mred",
            "tab\there",
            "alice@example.com\n",
            "\talice@example.com",
        ] {
            claims.insert(claim.into(), json!(bad));
            let why = refusal(rig.check(&claims).await, &format!("{claim} {bad:?}"));
            assert!(
                why.contains(&format!("\"{claim}\"")),
                "{claim} {bad:?}: {why}"
            );
            assert!(!why.contains("alice"), "a value in the reason: {why}");
        }
        for not_text in [json!(5), json!(null), json!(["a@b"]), json!(true)] {
            claims.insert(claim.into(), not_text.clone());
            refusal(rig.check(&claims).await, &format!("{claim} {not_text}"));
        }
        claims.remove(claim);
        refusal(rig.check(&claims).await, &format!("no {claim}"));
        // A user is trimmed and lower-cased by the key it becomes, and a name with a space or an accent is a name.
        for (written, kept) in [
            (" Alice@Example.com ", "alice@example.com"),
            ("Zoë Ünï", "zoë ünï"),
            ("idp|42", "idp|42"),
        ] {
            claims.insert(claim.into(), json!(written));
            let principal = assert_accepted(rig.check(&claims).await, written);
            assert_eq!(principal.user.as_str(), kept);
        }
    }
}

// ---- the order of the reasons ------------------------------------------------------------------------------------

#[tokio::test]
async fn when_several_claims_are_missing_the_reason_is_always_the_same() {
    // The order is the order of the checks: exp, iss, aud, iat, the user. (A set of claims the library required
    // was read in the order of a hash set, so a token that lacked several was refused with another reason from one
    // request to the next.)
    let rig = Rig::new().await;
    let reason = |claims: Map<String, Value>| {
        let rig = &rig;
        async move { refusal(rig.check(&claims).await, "a token missing claims") }
    };
    for _ in 0..40 {
        let mut claims = rig.claims();
        for claim in ["exp", "iss", "aud", "iat", "email"] {
            claims.remove(claim);
        }
        let why = reason(claims.clone()).await;
        assert!(
            why.contains("\"exp\""),
            "no exp, iss, aud, iat or e-mail: {why}"
        );
        claims.insert("exp".into(), json!(now() + 300));
        let why = reason(claims.clone()).await;
        assert!(why.contains("\"iss\""), "no iss, aud, iat or e-mail: {why}");
        claims.insert("iss".into(), json!(rig.issuer));
        let why = reason(claims.clone()).await;
        assert!(why.contains("\"aud\""), "no aud, iat or e-mail: {why}");
        claims.insert("aud".into(), json!(AUDIENCE));
        let why = reason(claims.clone()).await;
        assert!(why.contains("\"iat\""), "no iat or e-mail: {why}");
        claims.insert("iat".into(), json!(now()));
        let why = reason(claims.clone()).await;
        assert!(why.contains("\"email\""), "no e-mail: {why}");
        claims.insert("email".into(), json!(USER));
        assert_accepted(rig.check(&claims).await, "all there");
    }
}

#[tokio::test]
async fn the_times_come_before_the_issuer_and_the_audience() {
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.insert("exp".into(), json!(now() - 3600));
    claims.insert("iss".into(), json!("https://evil.example"));
    claims.insert("aud".into(), json!("someone-else"));
    let why = refusal(rig.check(&claims).await, "expired, and for another issuer");
    assert!(why.contains("expired"), "{why}");
}

// ---- the times ---------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_leeway_is_a_minute_and_the_edge_is_where_it_is() {
    // The edge is `LEEWAY_SECS`, not 30 and not 120; 5 seconds of margin to either side keep a slow machine from
    // moving a token across it between the clock of the test and the clock of the check.
    assert_eq!(LEEWAY_SECS, 60);
    let rig = Rig::new().await;
    let mut claims = rig.claims();

    for (ago, accepted) in [
        (3600, false),
        (120, false),
        (65, false),
        (55, true),
        (30, true),
    ] {
        claims.insert("exp".into(), json!(now() - ago));
        let result = rig.check(&claims).await;
        if accepted {
            assert_accepted(result, &format!("expired {ago} s ago, inside the leeway"));
        } else {
            let why = refusal(result, &format!("expired {ago} s ago, past the leeway"));
            assert!(why.contains("expired"), "{why}");
        }
    }
    claims.insert("exp".into(), json!(now() + 3600));

    for (ahead, accepted) in [
        (3600, false),
        (120, false),
        (65, false),
        (55, true),
        (30, true),
    ] {
        claims.insert("nbf".into(), json!(now() + ahead));
        let result = rig.check(&claims).await;
        if accepted {
            assert_accepted(result, &format!("valid in {ahead} s, inside the leeway"));
        } else {
            let why = refusal(result, &format!("valid in {ahead} s, past the leeway"));
            assert!(why.contains("not valid yet"), "{why}");
        }
    }
    claims.insert("nbf".into(), json!(now() - 10));
    assert_accepted(rig.check(&claims).await, "valid since 10 s");
}
