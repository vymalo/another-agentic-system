//! DPoP (RFC 9449, ADR 0054) as the JWT authenticator reads it: the proof's every check, the
//! binding of the token to the proof's key, the bearer that is refused when its token is bound, and
//! what a deployment without DPoP does. Tokens come from a `TestIdp`, proofs from a `DpopKey`.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use orch_auth_jwt::testkit::{DpopKey, TestIdp};
use orch_auth_jwt::{BuildError, DpopConfig, JwtAuth, JwtConfig};
use orch_ports::testkit::bearer::{Alg, Signing};
use orch_ports::{
    AuthError, Authenticator, CredentialKind, Credentials, DpopCredentials, Principal,
};
use serde_json::{Map, Value, json};

const AUDIENCE: &str = "orchestrator-web";
const ALICE: &str = "alice@example.com";
const ORIGIN: &str = "https://chat.example.com";
const PATH: &str = "/api/threads";
const URL: &str = "https://chat.example.com/api/threads";

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

fn config(idp: &TestIdp) -> JwtConfig {
    JwtConfig::new(idp.issuer(), [AUDIENCE]).without_system_proxy()
}

fn with_dpop(idp: &TestIdp) -> JwtAuth {
    JwtAuth::new(config(idp).with_dpop(DpopConfig::new([ORIGIN]))).unwrap()
}

/// A DPoP request: `GET /api/threads` with `token` and `proofs`.
async fn dpop(auth: &JwtAuth, token: &str, proofs: &[&str]) -> Result<Principal, AuthError> {
    request(auth, token, proofs, "GET", PATH).await
}

async fn request(
    auth: &JwtAuth,
    token: &str,
    proofs: &[&str],
    method: &str,
    path: &str,
) -> Result<Principal, AuthError> {
    auth.authenticate(&Credentials {
        dpop: Some(DpopCredentials {
            token,
            proofs,
            method,
            path,
        }),
        ..Credentials::default()
    })
    .await
}

async fn bearer(auth: &JwtAuth, token: &str) -> Result<Principal, AuthError> {
    auth.authenticate(&Credentials {
        bearer: Some(token),
        ..Credentials::default()
    })
    .await
}

/// Which credential a refusal is about, `None` for a result that is not a refusal.
fn refused_as(result: &Result<Principal, AuthError>) -> Option<CredentialKind> {
    match result {
        Err(AuthError::Invalid { credential, .. }) => Some(*credential),
        _ => None,
    }
}

fn is_bad_proof(result: &Result<Principal, AuthError>) -> bool {
    refused_as(result) == Some(CredentialKind::DpopProof)
}

fn is_bad_token(result: &Result<Principal, AuthError>) -> bool {
    refused_as(result) == Some(CredentialKind::DpopToken)
}

/// A token bound to `key`, and a good proof for it.
fn pair(idp: &TestIdp, key: &DpopKey) -> (String, String) {
    let token = idp.bound_token(AUDIENCE, ALICE, key);
    let proof = key.proof("GET", URL, &token);
    (token, proof)
}

/// A proof of `claims` with the good header, signed by `key`, for `token`.
fn proof_with(key: &DpopKey, edit: impl FnOnce(&mut Map<String, Value>), token: &str) -> String {
    let mut claims = DpopKey::claims("GET", URL, token);
    edit(&mut claims);
    key.sign(&key.header(), &claims)
}

// --- what is accepted ---

#[tokio::test]
async fn a_valid_es256_proof_with_a_bound_token_is_accepted() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let (token, proof) = pair(&idp, &key);
    let principal = dpop(&auth, &token, &[&proof]).await.unwrap();
    assert_eq!(principal.user.as_str(), ALICE);
    assert_eq!(principal.email.as_deref(), Some(ALICE));
}

#[tokio::test]
async fn a_valid_eddsa_proof_with_a_bound_token_is_accepted() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::ed25519();
    let (token, proof) = pair(&idp, &key);
    assert_eq!(
        dpop(&auth, &token, &[&proof]).await.unwrap().user.as_str(),
        ALICE
    );
}

#[tokio::test]
async fn the_principal_of_a_dpop_request_expires_with_the_token_so_streams_end_with_it() {
    // The edge ends a stream at `Principal::expires_at` plus the leeway (ADR 0033): a DPoP
    // principal must carry the token's `exp` as a bearer's does.
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let claims = idp.bound_claims(AUDIENCE, ALICE, &key);
    let exp = claims["exp"].as_i64().unwrap();
    let token = idp.mint(&claims, Signing::Published(Alg::Rs256));
    let proof = key.proof("GET", URL, &token);
    let principal = dpop(&auth, &token, &[&proof]).await.unwrap();
    assert_eq!(principal.expires_at.unwrap().as_second(), exp);
}

#[tokio::test]
async fn the_roles_of_the_token_are_read_as_for_a_bearer() {
    let idp = TestIdp::start().await;
    let auth = JwtAuth::new(
        config(&idp)
            .with_roles_claim("realm_access.roles")
            .with_dpop(DpopConfig::new([ORIGIN])),
    )
    .unwrap();
    let key = DpopKey::es256();
    let mut claims = idp.bound_claims(AUDIENCE, ALICE, &key);
    claims.insert("realm_access".into(), json!({"roles": ["admin"]}));
    let token = idp.mint(&claims, Signing::Published(Alg::Rs256));
    let proof = key.proof("GET", URL, &token);
    let principal = dpop(&auth, &token, &[&proof]).await.unwrap();
    assert!(principal.roles.iter().any(|r| r.as_str() == "admin"));
}

#[tokio::test]
async fn any_of_the_public_origins_is_accepted_and_nothing_else() {
    let idp = TestIdp::start().await;
    let auth =
        JwtAuth::new(config(&idp).with_dpop(DpopConfig::new([ORIGIN, "http://localhost:3000"])))
            .unwrap();
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for ok in [URL, "http://localhost:3000/api/threads"] {
        let proof = key.proof("GET", ok, &token);
        dpop(&auth, &token, &[&proof]).await.unwrap();
    }
    for bad in [
        "https://localhost:3000/api/threads",
        "http://localhost/api/threads",
        "https://chat.example.com:8443/api/threads",
    ] {
        let proof = key.proof("GET", bad, &token);
        assert!(is_bad_proof(&dpop(&auth, &token, &[&proof]).await), "{bad}");
    }
}

#[tokio::test]
async fn the_query_and_the_fragment_of_htu_are_ignored() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for htu in [
        "https://chat.example.com/api/threads?limit=5&x=y",
        "https://chat.example.com/api/threads#frag",
        "https://CHAT.example.com:443/api/threads",
    ] {
        let proof = key.proof("GET", htu, &token);
        dpop(&auth, &token, &[&proof])
            .await
            .unwrap_or_else(|e| panic!("{htu}: {e}"));
    }
}

// --- what is refused about the proof ---

#[tokio::test]
async fn the_method_must_be_the_requests() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    let proof = key.proof("POST", URL, &token);
    assert!(is_bad_proof(&dpop(&auth, &token, &[&proof]).await));
    // and `htm` is case-sensitive: the method is `GET`.
    let lower = key.proof("get", URL, &token);
    assert!(is_bad_proof(&dpop(&auth, &token, &[&lower]).await));
}

#[tokio::test]
async fn the_url_must_be_a_public_origin_and_the_requests_path() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for htu in [
        "https://evil.example.com/api/threads",
        "http://chat.example.com/api/threads",
        "https://chat.example.com/api/other",
        "https://chat.example.com/api/threads/",
        "https://user@chat.example.com/api/threads",
        "not a url",
        "",
    ] {
        let proof = key.proof("GET", htu, &token);
        assert!(
            is_bad_proof(&dpop(&auth, &token, &[&proof]).await),
            "{htu:?}"
        );
    }
    // The path is the request's, not the proof's alone: a proof for `/api/threads` is no proof for
    // another path.
    let proof = key.proof("GET", URL, &token);
    assert!(is_bad_proof(
        &request(&auth, &token, &[&proof], "GET", "/api/me").await
    ));
}

#[tokio::test]
async fn iat_must_be_within_sixty_seconds_past_and_five_ahead() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for (offset, accepted) in [
        (-50, true),
        (-70, false),
        (3, true),
        (10, false),
        (3600, false),
    ] {
        let proof = proof_with(
            &key,
            |c| {
                c.insert("iat".into(), json!(now() + offset));
            },
            &token,
        );
        let result = dpop(&auth, &token, &[&proof]).await;
        assert_eq!(result.is_ok(), accepted, "iat {offset:+}s: {result:?}");
        if !accepted {
            assert!(is_bad_proof(&result), "iat {offset:+}s");
        }
    }
}

#[tokio::test]
async fn the_window_is_configurable() {
    let idp = TestIdp::start().await;
    let auth = JwtAuth::new(config(&idp).with_dpop(
        DpopConfig::new([ORIGIN]).with_window(Duration::from_secs(600), Duration::from_secs(120)),
    ))
    .unwrap();
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for offset in [-500, 100] {
        let proof = proof_with(
            &key,
            |c| {
                c.insert("iat".into(), json!(now() + offset));
            },
            &token,
        );
        dpop(&auth, &token, &[&proof]).await.unwrap();
    }
    let proof = proof_with(
        &key,
        |c| {
            c.insert("iat".into(), json!(now() - 700));
        },
        &token,
    );
    assert!(is_bad_proof(&dpop(&auth, &token, &[&proof]).await));
}

#[tokio::test]
async fn a_proof_is_good_once() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let (token, proof) = pair(&idp, &key);
    dpop(&auth, &token, &[&proof]).await.unwrap();
    let again = dpop(&auth, &token, &[&proof]).await;
    assert!(is_bad_proof(&again), "{again:?}");
    // A fresh proof for the same token and key is good: the key is not what was spent.
    let next = key.proof("GET", URL, &token);
    dpop(&auth, &token, &[&next]).await.unwrap();
}

#[tokio::test]
async fn the_same_jti_of_another_key_is_not_a_replay() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let (a, b) = (DpopKey::es256(), DpopKey::es256());
    let (ta, tb) = (
        idp.bound_token(AUDIENCE, ALICE, &a),
        idp.bound_token(AUDIENCE, ALICE, &b),
    );
    let one = proof_with(
        &a,
        |c| {
            c.insert("jti".into(), json!("same"));
        },
        &ta,
    );
    let two = proof_with(
        &b,
        |c| {
            c.insert("jti".into(), json!("same"));
        },
        &tb,
    );
    dpop(&auth, &ta, &[&one]).await.unwrap();
    dpop(&auth, &tb, &[&two]).await.unwrap();
}

#[tokio::test]
async fn a_request_that_is_refused_does_not_spend_its_proof() {
    // The proof is remembered last, once the token is valid and bound: a caller who holds no good
    // token cannot burn another's `jti`, and a proof refused for its token can be sent again.
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let mut expired = idp.bound_claims(AUDIENCE, ALICE, &key);
    expired.insert("exp".into(), json!(now() - 3600));
    expired.insert("iat".into(), json!(now() - 7200));
    let token = idp.mint(&expired, Signing::Published(Alg::Rs256));
    let proof = key.proof("GET", URL, &token);
    assert!(is_bad_token(&dpop(&auth, &token, &[&proof]).await));
    assert!(
        is_bad_token(&dpop(&auth, &token, &[&proof]).await),
        "still the token, not a replay"
    );
}

#[tokio::test]
async fn concurrent_requests_with_one_proof_let_exactly_one_in() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let (token, proof) = pair(&idp, &key);
    let (token, proof) = (std::sync::Arc::new(token), std::sync::Arc::new(proof));
    // Warm the keys, so the race is about the proof.
    bearer(&auth, &idp.token(AUDIENCE, ALICE)).await.unwrap();
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let (auth, token, proof) = (auth.clone(), token.clone(), proof.clone());
        tasks.push(tokio::spawn(async move {
            dpop(&auth, &token, &[&proof]).await.is_ok()
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        accepted += usize::from(task.await.unwrap());
    }
    assert_eq!(accepted, 1);
}

#[tokio::test]
async fn a_full_replay_cache_refuses_as_unavailable_and_never_forgets() {
    let idp = TestIdp::start().await;
    let mut dpop_config = DpopConfig::new([ORIGIN]);
    dpop_config.replay_capacity = 2;
    let auth = JwtAuth::new(config(&idp).with_dpop(dpop_config)).unwrap();
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    let first = key.proof("GET", URL, &token);
    dpop(&auth, &token, &[&first]).await.unwrap();
    dpop(&auth, &token, &[&key.proof("GET", URL, &token)])
        .await
        .unwrap();
    let third = dpop(&auth, &token, &[&key.proof("GET", URL, &token)]).await;
    assert!(
        matches!(third, Err(AuthError::Unavailable { .. })),
        "{third:?}"
    );
    // The first is still remembered.
    assert!(is_bad_proof(&dpop(&auth, &token, &[&first]).await));
}

#[tokio::test]
async fn ath_must_be_the_hash_of_this_token() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    let other = idp.bound_token(AUDIENCE, "bob@example.com", &key);
    // A proof made for another token of the same key.
    let proof = key.proof("GET", URL, &other);
    assert!(is_bad_proof(&dpop(&auth, &token, &[&proof]).await));
    for ath in [json!(""), json!("AAAA"), json!(5)] {
        let proof = proof_with(
            &key,
            |c| {
                c.insert("ath".into(), ath.clone());
            },
            &token,
        );
        assert!(is_bad_proof(&dpop(&auth, &token, &[&proof]).await), "{ath}");
    }
    let no_ath = proof_with(
        &key,
        |c| {
            c.remove("ath");
        },
        &token,
    );
    assert!(is_bad_proof(&dpop(&auth, &token, &[&no_ath]).await));
}

#[tokio::test]
async fn a_proof_missing_a_claim_is_refused() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for claim in ["jti", "htm", "htu", "iat"] {
        let proof = proof_with(
            &key,
            |c| {
                c.remove(claim);
            },
            &token,
        );
        assert!(
            is_bad_proof(&dpop(&auth, &token, &[&proof]).await),
            "no {claim}"
        );
    }
    let empty = proof_with(
        &key,
        |c| {
            c.insert("jti".into(), json!(""));
        },
        &token,
    );
    assert!(is_bad_proof(&dpop(&auth, &token, &[&empty]).await));
    let long = proof_with(
        &key,
        |c| {
            c.insert("jti".into(), json!("x".repeat(300)));
        },
        &token,
    );
    assert!(is_bad_proof(&dpop(&auth, &token, &[&long]).await));
}

#[tokio::test]
async fn a_key_with_a_private_member_is_refused() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    for key in [DpopKey::es256(), DpopKey::ed25519()] {
        let token = idp.bound_token(AUDIENCE, ALICE, &key);
        for member in ["d", "p", "q", "dp", "dq", "qi", "oth", "k"] {
            let mut header = key.header();
            header["jwk"]
                .as_object_mut()
                .unwrap()
                .insert(member.into(), json!("AAAA"));
            let proof = key.sign(&header, &DpopKey::claims("GET", URL, &token));
            assert!(
                is_bad_proof(&dpop(&auth, &token, &[&proof]).await),
                "{} with {member}",
                key.alg()
            );
        }
    }
}

#[tokio::test]
async fn typ_must_be_dpop_jwt() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    for typ in [
        Some("JWT"),
        Some("dpop"),
        Some("DPOP+JWT"),
        Some("at+jwt"),
        None,
    ] {
        let mut header = key.header();
        match typ {
            Some(typ) => {
                header.insert("typ".into(), json!(typ));
            }
            None => {
                header.remove("typ");
            }
        }
        let proof = key.sign(&header, &DpopKey::claims("GET", URL, &token));
        assert!(
            is_bad_proof(&dpop(&auth, &token, &[&proof]).await),
            "{typ:?}"
        );
    }
}

#[tokio::test]
async fn only_es256_and_eddsa_are_read_as_algorithms_of_a_proof() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    // HMAC and RSA and `none`, and an algorithm that does not exist, over a valid ES256 signature.
    for alg in [
        "HS256", "HS512", "RS256", "PS256", "ES384", "none", "None", "nonsense",
    ] {
        let mut header = key.header();
        header.insert("alg".into(), json!(alg));
        let proof = key.sign(&header, &DpopKey::claims("GET", URL, &token));
        assert!(is_bad_proof(&dpop(&auth, &token, &[&proof]).await), "{alg}");
    }
    // An unsigned `none` proof, as an attacker would make it.
    let mut header = key.header();
    header.insert("alg".into(), json!("none"));
    let unsigned = format!(
        "{}.{}.",
        base64url(&serde_json::to_vec(&header).unwrap()),
        base64url(&serde_json::to_vec(&DpopKey::claims("GET", URL, &token)).unwrap())
    );
    assert!(is_bad_proof(&dpop(&auth, &token, &[&unsigned]).await));
}

fn base64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[tokio::test]
async fn the_key_must_be_the_one_that_signed_and_of_the_algorithms_kind() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let (a, b) = (DpopKey::es256(), DpopKey::es256());
    let token = idp.bound_token(AUDIENCE, ALICE, &a);
    // Signed by B, announcing A's key: the signature does not verify with it.
    let forged = b.sign(&a.header(), &DpopKey::claims("GET", URL, &token));
    assert!(is_bad_proof(&dpop(&auth, &token, &[&forged]).await));
    // An Ed25519 key under `alg: ES256`, and a P-256 key under `alg: EdDSA`.
    let ed = DpopKey::ed25519();
    let mut header = ed.header();
    header.insert("alg".into(), json!("ES256"));
    let mixed = ed.sign(&header, &DpopKey::claims("GET", URL, &token));
    assert!(is_bad_proof(&dpop(&auth, &token, &[&mixed]).await));
    let mut header = a.header();
    header.insert("alg".into(), json!("EdDSA"));
    let mixed = a.sign(&header, &DpopKey::claims("GET", URL, &token));
    assert!(is_bad_proof(&dpop(&auth, &token, &[&mixed]).await));
    // No key, a key that is not an object, a key of another curve, a coordinate that is too short.
    for jwk in [
        None,
        Some(json!("x")),
        Some(json!({"kty": "EC", "crv": "P-384", "x": "AAAA", "y": "AAAA"})),
        Some(json!({"kty": "EC", "crv": "P-256", "x": "AAAA", "y": "AAAA"})),
        Some(json!({"kty": "oct"})),
        Some(json!({"kty": "RSA", "n": "AQAB", "e": "AQAB"})),
    ] {
        let mut header = a.header();
        match &jwk {
            Some(jwk) => {
                header.insert("jwk".into(), jwk.clone());
            }
            None => {
                header.remove("jwk");
            }
        }
        let proof = a.sign(&header, &DpopKey::claims("GET", URL, &token));
        assert!(
            is_bad_proof(&dpop(&auth, &token, &[&proof]).await),
            "{jwk:?}"
        );
    }
}

#[tokio::test]
async fn a_tampered_proof_is_refused() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let (token, proof) = pair(&idp, &key);
    let mut parts: Vec<String> = proof.split('.').map(str::to_owned).collect();
    let mut claims: Map<String, Value> = serde_json::from_slice(&base64_decode(&parts[1])).unwrap();
    claims.insert("htm".into(), json!("GET")); // unchanged value, other bytes
    claims.insert("extra".into(), json!(1));
    parts[1] = base64url(&serde_json::to_vec(&claims).unwrap());
    assert!(is_bad_proof(
        &dpop(&auth, &token, &[&parts.join(".")]).await
    ));
    for malformed in ["", "a", "a.b", "a.b.c", "a.b.c.d", "....", "e30.e30.e30"] {
        assert!(
            is_bad_proof(&dpop(&auth, &token, &[malformed]).await),
            "{malformed:?}"
        );
    }
    let huge = format!("{proof}{}", "A".repeat(9000));
    assert!(is_bad_proof(&dpop(&auth, &token, &[&huge]).await));
}

fn base64_decode(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .unwrap()
}

#[tokio::test]
async fn exactly_one_proof_is_needed() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    let one = key.proof("GET", URL, &token);
    let two = key.proof("GET", URL, &token);
    assert!(is_bad_proof(&dpop(&auth, &token, &[]).await), "none");
    assert!(
        is_bad_proof(&dpop(&auth, &token, &[&one, &two]).await),
        "two"
    );
    assert!(
        is_bad_proof(&dpop(&auth, &token, &[&one, &one]).await),
        "the same twice"
    );
    // Refused, so not spent: the one that was refused for being two is good alone.
    dpop(&auth, &token, &[&one]).await.unwrap();
}

// --- the binding of the token to the proof's key ---

#[tokio::test]
async fn the_token_must_be_bound_to_the_proofs_key() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let (owner, thief) = (DpopKey::es256(), DpopKey::es256());
    let token = idp.bound_token(AUDIENCE, ALICE, &owner);
    // A thief with the token and a proof of their own key (everything else in order).
    let proof = thief.proof("GET", URL, &token);
    let result = dpop(&auth, &token, &[&proof]).await;
    assert!(is_bad_token(&result), "{result:?}");
    // The key's algorithm does not matter, the thumbprint does.
    let ed = DpopKey::ed25519();
    let result = dpop(&auth, &token, &[&ed.proof("GET", URL, &token)]).await;
    assert!(is_bad_token(&result), "{result:?}");
}

#[tokio::test]
async fn a_token_with_no_binding_is_refused_with_the_dpop_scheme() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.token(AUDIENCE, ALICE);
    let result = dpop(&auth, &token, &[&key.proof("GET", URL, &token)]).await;
    assert!(is_bad_token(&result), "{result:?}");
    // `cnf` without `jkt`, or with a `jkt` that is not text.
    for cnf in [
        json!({}),
        json!({"x5t#S256": "abc"}),
        json!({"jkt": 5}),
        json!("jkt"),
        json!(null),
    ] {
        let mut claims = idp.claims(AUDIENCE, ALICE);
        claims.insert("cnf".into(), cnf.clone());
        let token = idp.mint(&claims, Signing::Published(Alg::Rs256));
        let result = dpop(&auth, &token, &[&key.proof("GET", URL, &token)]).await;
        assert!(is_bad_token(&result), "{cnf}: {result:?}");
    }
}

#[tokio::test]
async fn the_token_is_validated_as_for_a_bearer_and_refused_as_a_dpop_token() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let mint = |edit: &dyn Fn(&mut Map<String, Value>), signing| {
        let mut claims = idp.bound_claims(AUDIENCE, ALICE, &key);
        edit(&mut claims);
        idp.mint(&claims, signing)
    };
    let cases = [
        (
            "expired",
            mint(
                &|c| {
                    c.insert("exp".into(), json!(now() - 3600));
                },
                Signing::Published(Alg::Rs256),
            ),
        ),
        (
            "wrong audience",
            mint(
                &|c| {
                    c.insert("aud".into(), json!("other"));
                },
                Signing::Published(Alg::Rs256),
            ),
        ),
        (
            "wrong issuer",
            mint(
                &|c| {
                    c.insert("iss".into(), json!("https://evil.example"));
                },
                Signing::Published(Alg::Rs256),
            ),
        ),
        (
            "unpublished key",
            mint(&|_| {}, Signing::Unpublished(Alg::Rs256)),
        ),
        ("alg none", mint(&|_| {}, Signing::NoneAlg)),
        ("hs256", mint(&|_| {}, Signing::Hs256WithPublicKey)),
    ];
    for (what, token) in cases {
        let result = dpop(&auth, &token, &[&key.proof("GET", URL, &token)]).await;
        assert!(is_bad_token(&result), "{what}: {result:?}");
    }
    let result = dpop(&auth, "", &[&key.proof("GET", URL, "")]).await;
    assert!(is_bad_token(&result), "no token: {result:?}");
}

#[tokio::test]
async fn a_bound_token_is_refused_as_a_bearer_when_dpop_is_configured() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let key = DpopKey::es256();
    let token = idp.bound_token(AUDIENCE, ALICE, &key);
    let result = bearer(&auth, &token).await;
    assert!(is_bad_token(&result), "{result:?}");
    // Any `cnf` binds: a token bound to a certificate is no bearer either.
    let mut claims = idp.claims(AUDIENCE, ALICE);
    claims.insert("cnf".into(), json!({"x5t#S256": "abc"}));
    let token = idp.mint(&claims, Signing::Published(Alg::Rs256));
    assert!(is_bad_token(&bearer(&auth, &token).await));
}

#[tokio::test]
async fn a_plain_bearer_is_unchanged_when_dpop_is_configured() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    assert_eq!(
        bearer(&auth, &idp.token(AUDIENCE, ALICE))
            .await
            .unwrap()
            .user
            .as_str(),
        ALICE
    );
    // and its refusals are the bearer's.
    let mut claims = idp.claims(AUDIENCE, ALICE);
    claims.insert("exp".into(), json!(now() - 3600));
    let expired = idp.mint(&claims, Signing::Published(Alg::Rs256));
    assert_eq!(
        refused_as(&bearer(&auth, &expired).await),
        Some(CredentialKind::Bearer)
    );
    assert!(matches!(
        auth.authenticate(&Credentials::default()).await,
        Err(AuthError::Missing)
    ));
}

// --- DPoP not configured ---

#[tokio::test]
async fn without_dpop_configured_the_dpop_scheme_is_refused_and_a_bearer_is_as_before() {
    let idp = TestIdp::start().await;
    let auth = JwtAuth::new(config(&idp)).unwrap();
    assert!(!auth.accepts_dpop());
    assert!(with_dpop(&idp).accepts_dpop());
    let key = DpopKey::es256();
    let (token, proof) = pair(&idp, &key);
    let result = dpop(&auth, &token, &[&proof]).await;
    // Refused, as a bearer would be: this deployment does not say it reads DPoP.
    assert_eq!(
        refused_as(&result),
        Some(CredentialKind::Bearer),
        "{result:?}"
    );
    // A bearer is exactly what it was, a bound token included: nothing was configured to refuse it.
    assert_eq!(bearer(&auth, &token).await.unwrap().user.as_str(), ALICE);
    assert_eq!(
        bearer(&auth, &idp.token(AUDIENCE, ALICE))
            .await
            .unwrap()
            .user
            .as_str(),
        ALICE
    );
}

#[tokio::test]
async fn a_dpop_request_never_falls_back_to_the_bearer() {
    // Even a request whose token alone would be a good bearer is refused when its proof is bad.
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let token = idp.token(AUDIENCE, ALICE);
    assert!(is_bad_proof(&dpop(&auth, &token, &[]).await));
    assert!(is_bad_proof(&dpop(&auth, &token, &["garbage"]).await));
}

// --- what a refusal says ---

#[tokio::test]
async fn a_refusal_never_shows_a_token_or_a_proof() {
    let idp = TestIdp::start().await;
    let auth = with_dpop(&idp);
    let (owner, thief) = (DpopKey::es256(), DpopKey::es256());
    let token = idp.bound_token(AUDIENCE, ALICE, &owner);
    let proof = thief.proof("POST", URL, &token);
    let secrets = [
        token.as_str(),
        proof.as_str(),
        token.split('.').nth(1).unwrap(),
        proof.split('.').nth(1).unwrap(),
    ];
    let mut results = vec![
        dpop(&auth, &token, &[&proof]).await,
        dpop(&auth, &token, &[&thief.proof("GET", URL, &token)]).await,
        bearer(&auth, &token).await,
    ];
    results.push(dpop(&auth, &token, &[&proof, &proof]).await);
    for result in results {
        let shown = format!("{result:?}");
        let error = result.unwrap_err().to_string();
        for secret in secrets {
            assert!(
                !shown.contains(secret) && !error.contains(secret),
                "{shown}"
            );
        }
    }
}

#[tokio::test]
async fn dpop_needs_origins_and_they_must_be_origins() {
    let idp = TestIdp::start().await;
    let build = |origins: &[&str]| {
        JwtAuth::new(config(&idp).with_dpop(DpopConfig::new(origins.iter().copied())))
    };
    assert!(matches!(build(&[]), Err(BuildError::NoDpopOrigin)));
    for bad in [
        "chat.example.com",
        "https://chat.example.com/app",
        "ftp://x.example",
        "https://u:p@x.example",
        "https://x.example?q=1",
    ] {
        assert!(
            matches!(build(&[bad]), Err(BuildError::BadDpopOrigin)),
            "{bad}"
        );
    }
    assert!(build(&[ORIGIN, "http://localhost:3000"]).is_ok());
    let zero = DpopConfig::new([ORIGIN]).with_window(Duration::ZERO, Duration::ZERO);
    assert!(matches!(
        JwtAuth::new(config(&idp).with_dpop(zero)),
        Err(BuildError::BadDpopWindow)
    ));
}
