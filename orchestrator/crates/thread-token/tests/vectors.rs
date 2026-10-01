//! Known-answer vectors: a fixed key, fixed claims and a fixed time give exactly these bytes.
//!
//! The expected tokens were computed with an independent implementation (Python's `hmac`,
//! `hashlib` and `base64`, 2026-10-01), not with this crate, and are written in
//! `docs/api/thread-tools-v1.md`. A change that moves a byte here is a change of the contract: a
//! new URI, not an edit.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::{AgentId, Caller, ThreadId, ToolsGrant};
use orch_thread_token::{Claims, ThreadToolsIssuer, ThreadToolsKeys, TokenError, mint, verify};
use secrecy::{ExposeSecret, SecretString};

/// 2026-10-01T12:00:00Z.
const ISSUED: i64 = 1_790_856_000;

// The keys of these vectors are made up for the tests (a counting sequence, and a descending one).
const KEY_1: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const KID_1: &str = "6c86c6aac5fb24bc";
const KEY_2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
const KID_2: &str = "8588cdfcd6d2b0d5";

const THREAD: &str = "01927a4e-3b00-7000-8000-000000000001";

const VECTOR_1_HEADER: &str = r#"{"alg":"HS256","typ":"JWT","kid":"6c86c6aac5fb24bc"}"#;
const VECTOR_1_CLAIMS: &str = r#"{"iss":"orch","aud":"thread-tools","sub":"01927a4e-3b00-7000-8000-000000000001","job":3,"agt":"coder","caller":"main","depth":0,"jti":"5b0b9c2e-7f61-4d1c-9a43-2f3f6d0f9c11","iat":1790856000,"exp":1790863200}"#;
// nosemgrep: a known-answer test vector, signed with the made-up test key above, not a credential
const VECTOR_1: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6IjZjODZjNmFhYzVmYjI0YmMifQ.eyJpc3MiOiJvcmNoIiwiYXVkIjoidGhyZWFkLXRvb2xzIiwic3ViIjoiMDE5MjdhNGUtM2IwMC03MDAwLTgwMDAtMDAwMDAwMDAwMDAxIiwiam9iIjozLCJhZ3QiOiJjb2RlciIsImNhbGxlciI6Im1haW4iLCJkZXB0aCI6MCwianRpIjoiNWIwYjljMmUtN2Y2MS00ZDFjLTlhNDMtMmYzZjZkMGY5YzExIiwiaWF0IjoxNzkwODU2MDAwLCJleHAiOjE3OTA4NjMyMDB9.jloITlRjG0bpM62IXeLGZq-mUH1badzKsCjKn0E5vvM";

const VECTOR_2_CLAIMS: &str = r#"{"iss":"orch","aud":"thread-tools","sub":"01927a4e-3b00-7000-8000-000000000001","job":3,"agt":"researcher","caller":"ask:2","depth":1,"jti":"a7d2e1c0-0b3f-4e55-8d7a-9c1e4f6b2a30","iat":1790856000,"exp":1790856060}"#;
// nosemgrep: a known-answer test vector, signed with the made-up test key above, not a credential
const VECTOR_2: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6Ijg1ODhjZGZjZDZkMmIwZDUifQ.eyJpc3MiOiJvcmNoIiwiYXVkIjoidGhyZWFkLXRvb2xzIiwic3ViIjoiMDE5MjdhNGUtM2IwMC03MDAwLTgwMDAtMDAwMDAwMDAwMDAxIiwiam9iIjozLCJhZ3QiOiJyZXNlYXJjaGVyIiwiY2FsbGVyIjoiYXNrOjIiLCJkZXB0aCI6MSwianRpIjoiYTdkMmUxYzAtMGIzZi00ZTU1LThkN2EtOWMxZTRmNmIyYTMwIiwiaWF0IjoxNzkwODU2MDAwLCJleHAiOjE3OTA4NTYwNjB9.aIdaJXz3PRX8xbeV4LHMn9V3bH1tpaoa_ReeOq5STmg";

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(seconds).unwrap()
}

fn keys() -> ThreadToolsKeys {
    ThreadToolsKeys::new(secret(KEY_1), None).unwrap()
}

fn claims_1() -> Claims {
    Claims {
        thread: THREAD.parse().unwrap(),
        job: 3,
        agent: AgentId::new("coder"),
        caller: Caller::Main,
        depth: 0,
        message_id: "5b0b9c2e-7f61-4d1c-9a43-2f3f6d0f9c11".to_owned(),
        issued_at: at(ISSUED),
        expires_at: at(ISSUED + 7200),
    }
}

fn claims_2() -> Claims {
    Claims {
        agent: AgentId::new("researcher"),
        caller: Caller::Ask(2),
        depth: 1,
        message_id: "a7d2e1c0-0b3f-4e55-8d7a-9c1e4f6b2a30".to_owned(),
        expires_at: at(ISSUED + 60),
        ..claims_1()
    }
}

fn segments(token: &str) -> Vec<String> {
    token.split('.').map(str::to_owned).collect()
}

#[test]
fn the_kid_is_the_first_16_hex_characters_of_the_sha256_of_the_key() {
    assert_eq!(keys().current_kid(), KID_1);
    let rotated = ThreadToolsKeys::new(secret(KEY_2), Some(secret(KEY_1))).unwrap();
    assert_eq!(rotated.current_kid(), KID_2);
    assert_eq!(rotated.previous_kid(), Some(KID_1));
}

#[test]
fn vector_1_the_main_agent_is_exactly_these_bytes() {
    let token = mint(&keys(), &claims_1()).unwrap();
    assert_eq!(token.expose_secret(), VECTOR_1);

    // The segments say what the contract says they say.
    let parts = segments(VECTOR_1);
    use base64::Engine as _;
    let text = |segment: &str| {
        String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(segment)
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(text(&parts[0]), VECTOR_1_HEADER);
    assert_eq!(text(&parts[1]), VECTOR_1_CLAIMS);

    assert_eq!(
        verify(&keys(), VECTOR_1, at(ISSUED + 1)).unwrap(),
        claims_1()
    );
}

#[test]
fn vector_2_an_asked_agent_signed_with_the_other_key_is_exactly_these_bytes() {
    let signer = ThreadToolsKeys::new(secret(KEY_2), Some(secret(KEY_1))).unwrap();
    let token = mint(&signer, &claims_2()).unwrap();
    assert_eq!(token.expose_secret(), VECTOR_2);
    use base64::Engine as _;
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segments(VECTOR_2)[1].as_str())
        .unwrap();
    assert_eq!(String::from_utf8(claims).unwrap(), VECTOR_2_CLAIMS);

    // Verified by a replica that holds the key as current, or as previous (a rotation).
    let as_current = ThreadToolsKeys::new(secret(KEY_2), None).unwrap();
    let as_previous = ThreadToolsKeys::new(secret(KEY_1), Some(secret(KEY_2))).unwrap();
    for replica in [as_current, as_previous] {
        assert_eq!(
            verify(&replica, VECTOR_2, at(ISSUED + 5)).unwrap(),
            claims_2()
        );
    }
    // A replica that holds neither does not know the key.
    assert_eq!(
        verify(&keys(), VECTOR_2, at(ISSUED + 5)),
        Err(TokenError::UnknownKey)
    );
}

#[test]
fn the_issuer_gives_the_url_the_token_and_the_expiry() {
    let issuer = ThreadToolsIssuer::new(
        keys(),
        "http://orchestrator:8080/",
        std::time::Duration::from_secs(7200),
    )
    .unwrap();
    assert_eq!(issuer.base_url(), "http://orchestrator:8080");
    assert_eq!(issuer.host(), "orchestrator:8080");
    let grant = ToolsGrant::main(
        THREAD.parse::<ThreadId>().unwrap(),
        3,
        AgentId::new("coder"),
    );
    // A time with a fraction: the claims are whole seconds, the vector's bytes.
    let now = Timestamp::new(ISSUED, 500_000_000).unwrap();
    let minted = issuer
        .grant(&grant, "5b0b9c2e-7f61-4d1c-9a43-2f3f6d0f9c11", now)
        .unwrap();
    assert_eq!(
        minted.url,
        format!("http://orchestrator:8080/thread-tools/{THREAD}/mcp")
    );
    assert_eq!(minted.token.expose_secret(), VECTOR_1);
    assert_eq!(minted.expires_at.to_string(), "2026-10-01T14:00:00Z");
}

#[test]
fn a_token_is_valid_from_its_issue_to_its_expiry_and_thirty_seconds_beyond_each() {
    let keys = keys();
    let claims = claims_1();
    // 30 s of skew before the issue
    assert!(verify(&keys, VECTOR_1, at(ISSUED - 30)).is_ok());
    assert_eq!(
        verify(&keys, VECTOR_1, at(ISSUED - 31)),
        Err(TokenError::NotYetValid)
    );
    // the whole lifetime
    assert!(verify(&keys, VECTOR_1, at(ISSUED)).is_ok());
    assert!(verify(&keys, VECTOR_1, at(claims.expires_at.as_second())).is_ok());
    // 30 s after the expiry: still read, the 31st is expired ("exp is later than now minus 30 s")
    assert!(verify(&keys, VECTOR_1, at(claims.expires_at.as_second() + 29)).is_ok());
    assert_eq!(
        verify(&keys, VECTOR_1, at(claims.expires_at.as_second() + 30)),
        Err(TokenError::Expired)
    );
}

#[test]
fn only_the_one_algorithm_and_type_pass_whatever_the_signature() {
    use base64::Engine as _;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let payload = b64(VECTOR_1_CLAIMS.as_bytes());
    // A token signed with the right key but another header, as a forger who had the key would
    // write it: the header alone refuses it.
    let sign = |header: &str| {
        let input = format!("{}.{payload}", b64(header.as_bytes()));
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(KEY_1.as_bytes()).unwrap();
        mac.update(input.as_bytes());
        format!("{input}.{}", b64(&mac.finalize().into_bytes()))
    };
    let good = format!(r#"{{"alg":"HS256","typ":"JWT","kid":"{KID_1}"}}"#);
    assert!(verify(&keys(), &sign(&good), at(ISSUED)).is_ok());
    for header in [
        r#"{"alg":"none","typ":"JWT","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"HS512","typ":"JWT","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"hs256","typ":"JWT","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"RS256","typ":"JWT","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"HS256","typ":"jwt","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"HS256","typ":"at+jwt","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"HS256","kid":"6c86c6aac5fb24bc"}"#,
        r#"{"alg":"HS256","typ":"JWT"}"#,
        r#"{"alg":"HS256","typ":"JWT","kid":"6c86c6aac5fb24bc","crit":["exp"]}"#,
        r#"{"alg":"HS256","typ":"JWT","kid":"6c86c6aac5fb24bc","jku":"https://x.example/keys"}"#,
        "[]",
        "null",
        "{}",
    ] {
        assert_eq!(
            verify(&keys(), &sign(header), at(ISSUED)),
            Err(TokenError::Header),
            "{header}"
        );
    }
    // The right header with a kid nobody holds
    let unknown = r#"{"alg":"HS256","typ":"JWT","kid":"0000000000000000"}"#;
    assert_eq!(
        verify(&keys(), &sign(unknown), at(ISSUED)),
        Err(TokenError::UnknownKey)
    );
    // An unsigned token in the `none` style: an empty signature segment
    let unsigned = format!(
        "{}.{payload}.",
        b64(br#"{"alg":"none","typ":"JWT","kid":"6c86c6aac5fb24bc"}"#)
    );
    assert!(verify(&keys(), &unsigned, at(ISSUED)).is_err());
}

#[test]
fn claims_that_do_not_agree_are_refused_even_when_signed() {
    use base64::Engine as _;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header = b64(format!(r#"{{"alg":"HS256","typ":"JWT","kid":"{KID_1}"}}"#).as_bytes());
    let sign = |claims: &str| {
        let input = format!("{header}.{}", b64(claims.as_bytes()));
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(KEY_1.as_bytes()).unwrap();
        mac.update(input.as_bytes());
        format!("{input}.{}", b64(&mac.finalize().into_bytes()))
    };
    // Substitutions into the good claims, each one breaking a rule
    let with = |from: &str, to: &str| VECTOR_1_CLAIMS.replacen(from, to, 1);
    assert!(verify(&keys(), &sign(VECTOR_1_CLAIMS), at(ISSUED)).is_ok());
    let cases = [
        (
            with(r#""iss":"orch""#, r#""iss":"other""#),
            TokenError::Issuer,
        ),
        (
            with(r#""aud":"thread-tools""#, r#""aud":"x""#),
            TokenError::Audience,
        ),
        (with(r#""job":3"#, r#""job":0"#), TokenError::Claims),
        (
            with(r#""caller":"main""#, r#""caller":"root""#),
            TokenError::Claims,
        ),
        (
            with(r#""caller":"main""#, r#""caller":"ask:0""#),
            TokenError::Claims,
        ),
        // a main caller with a depth
        (with(r#""depth":0"#, r#""depth":1"#), TokenError::Claims),
        // an asked agent at depth 0
        (
            with(
                r#""caller":"main","depth":0"#,
                r#""caller":"ask:1","depth":0"#,
            ),
            TokenError::Claims,
        ),
        (with(r#""depth":0"#, r#""depth":256"#), TokenError::Claims),
        (with(r#""agt":"coder""#, r#""agt":"""#), TokenError::Claims),
        (
            with(
                r#""sub":"01927a4e-3b00-7000-8000-000000000001""#,
                r#""sub":"not-a-uuid""#,
            ),
            TokenError::Claims,
        ),
        (
            with(r#""iat":1790856000"#, r#""iat":"1790856000""#),
            TokenError::Claims,
        ),
        // expiry before the issue
        (
            with(r#""exp":1790863200"#, r#""exp":1790855999"#),
            TokenError::Claims,
        ),
        // a member the contract does not have
        (
            with(r#""job":3"#, r#""job":3,"scope":"admin""#),
            TokenError::Claims,
        ),
        // a member missing
        (
            with(r#""jti":"5b0b9c2e-7f61-4d1c-9a43-2f3f6d0f9c11","#, ""),
            TokenError::Claims,
        ),
        ("[]".to_owned(), TokenError::Claims),
        ("{}".to_owned(), TokenError::Claims),
    ];
    for (claims, want) in cases {
        assert_eq!(
            verify(&keys(), &sign(&claims), at(ISSUED)),
            Err(want),
            "{claims}"
        );
    }
}

#[test]
fn a_token_that_is_not_a_compact_jws_is_malformed() {
    let long = "a".repeat(2049);
    for bad in [
        "",
        ".",
        "..",
        "a.b",
        "a.b.c.d",
        "not a token",
        // padded
        &format!("{}=", VECTOR_1),
        // the standard alphabet
        &VECTOR_1.replace('-', "+").replace('_', "/"),
    ] {
        let got = verify(&keys(), bad, at(ISSUED));
        assert!(got.is_err(), "{bad:?}");
    }
    assert_eq!(verify(&keys(), &long, at(ISSUED)), Err(TokenError::TooLong));
    // Exactly at the limit is read (and is malformed here, not too long).
    let limit = "a".repeat(2048);
    assert_ne!(
        verify(&keys(), &limit, at(ISSUED)),
        Err(TokenError::TooLong)
    );
}

#[test]
fn keys_are_checked() {
    use orch_thread_token::{KeyError, MIN_KEY_BYTES};
    assert_eq!(
        ThreadToolsKeys::new(secret(&"k".repeat(MIN_KEY_BYTES - 1)), None).unwrap_err(),
        KeyError::TooShort { which: "current" }
    );
    assert!(ThreadToolsKeys::new(secret(&"k".repeat(MIN_KEY_BYTES)), None).is_ok());
    assert_eq!(
        ThreadToolsKeys::new(secret(KEY_1), Some(secret("short"))).unwrap_err(),
        KeyError::TooShort { which: "previous" }
    );
    assert_eq!(
        ThreadToolsKeys::new(secret(KEY_1), Some(secret(KEY_1))).unwrap_err(),
        KeyError::Same
    );
    // `Debug` names the keys by their kid and never shows one.
    let shown = format!(
        "{:?}",
        ThreadToolsKeys::new(secret(KEY_2), Some(secret(KEY_1))).unwrap()
    );
    assert!(shown.contains(KID_1) && shown.contains(KID_2), "{shown}");
    assert!(!shown.contains(KEY_1) && !shown.contains(KEY_2), "{shown}");
}

#[test]
fn the_issuer_is_checked_and_the_grant_never_shows_its_token() {
    use orch_thread_token::{IssuerError, ThreadToolsGrant};
    use std::time::Duration;
    for bad in [
        "",
        "orchestrator:8080",
        "ftp://orchestrator",
        "http://",
        "http://user:pw@orchestrator",
        "http://orchestrator?x=1",
        "http://orchestrator#x",
    ] {
        assert!(
            matches!(
                ThreadToolsIssuer::new(keys(), bad, Duration::from_secs(7200)),
                Err(IssuerError::BadUrl(_))
            ),
            "{bad:?}"
        );
    }
    for bad in [0, 59, 86_401] {
        assert_eq!(
            ThreadToolsIssuer::new(keys(), "https://orch.example.com", Duration::from_secs(bad))
                .unwrap_err(),
            IssuerError::BadTtl,
            "{bad}"
        );
    }
    assert_eq!(
        ThreadToolsIssuer::new(
            keys(),
            "https://orch.example.com",
            Duration::from_millis(60_500)
        )
        .unwrap_err(),
        IssuerError::BadTtl
    );
    for ok in [60, 7200, 86_400] {
        assert!(
            ThreadToolsIssuer::new(keys(), "https://orch.example.com", Duration::from_secs(ok))
                .is_ok()
        );
    }
    let issuer = ThreadToolsIssuer::new(
        keys(),
        "https://orch.example.com:8443/prefix",
        Duration::from_secs(7200),
    )
    .unwrap();
    assert_eq!(issuer.host(), "orch.example.com:8443");
    assert_eq!(issuer.ttl(), Duration::from_secs(7200));
    let grant = ToolsGrant::main(THREAD.parse().unwrap(), 1, AgentId::new("coder"));
    let minted: ThreadToolsGrant = issuer.grant(&grant, "m-1", at(ISSUED)).unwrap();
    assert_eq!(
        minted.url,
        format!("https://orch.example.com:8443/prefix/thread-tools/{THREAD}/mcp")
    );
    let token = minted.token.expose_secret().to_owned();
    for shown in [
        format!("{minted:?}"),
        format!("{issuer:?}"),
        format!("{:?}", issuer.keys()),
    ] {
        assert!(!shown.contains(&token), "{shown}");
        assert!(!shown.contains(KEY_1), "{shown}");
    }
    assert!(format!("{minted:?}").contains("[redacted]"));
    // An inconsistent grant, or no message id, is never minted.
    let bad_grant = ToolsGrant {
        depth: 1,
        ..grant.clone()
    };
    assert!(issuer.grant(&bad_grant, "m-1", at(ISSUED)).is_err());
    assert!(issuer.grant(&grant, "", at(ISSUED)).is_err());
}
