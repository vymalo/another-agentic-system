//! `orch-signed-request` against `cratestack-cose` used the way a tool server uses it, directly: what the orchestrator signs,
//! `open_claims` opens with the library's own resolver; the library's own Ed25519 signer writes the committed vector; and what
//! it signs on its own, the reference verifier judges. The vector against the orchestrator's writer, and the reference
//! verifier's refusals (another key, a flipped bit), are in `vectors.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack_cose::{
    ClaimsError, CwtClaims, Ed25519Signer, StaticVerifierResolver, open_claims, sign_claims,
};
use orch_signed_request::verify::{Incoming, Object, Verifier, VerifyError};
use orch_signed_request::{Method, Request, Signer, SigningKey, TYP, kid_of};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SEED: &[u8; 32] = b"throwaway-ed25519-seed-do-not-us";
const AUDIENCE: &str = "https://tools.example/mcp";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn vectors() -> Value {
    serde_json::from_str(include_str!("vectors/v1.json")).unwrap()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn ours(ttl: u64) -> Signer {
    Signer::new(SigningKey::from_seed(SEED).unwrap(), AUDIENCE, ttl).unwrap()
}

fn theirs() -> (Ed25519Signer, StaticVerifierResolver) {
    let signer = Ed25519Signer::from_seed(SEED);
    let resolver = StaticVerifierResolver::new().with_key(signer.verify_key());
    (signer, resolver)
}

/// Requests that stretch the encoding: an empty body, a large one, non-ASCII in the owner, a long path.
fn requests() -> Vec<(Method, String, Vec<u8>, Uuid, String)> {
    let thread = Uuid::from_u128(0x0194_a6c2_6b1e_7c3a_9f40_5d2e_8b7a_1c33);
    vec![
        (
            Method::Post,
            "/mcp".into(),
            br#"{"jsonrpc":"2.0","id":1}"#.to_vec(),
            thread,
            "amina@example.com".into(),
        ),
        (Method::Get, "/".into(), Vec::new(), thread, "a@b.c".into()),
        (
            Method::Delete,
            "/mcp/a%20b".into(),
            Vec::new(),
            thread,
            "zoë.müller@exämple.com".into(),
        ),
        (
            Method::Post,
            format!("/{}", "p".repeat(511)),
            vec![0xff; 100_000],
            Uuid::nil(),
            "o".repeat(320),
        ),
        (
            Method::Post,
            "/q".into(),
            "héllo \"quoted\" \\ \n tab\t".as_bytes().to_vec(),
            Uuid::max(),
            "x@y.z".into(),
        ),
    ]
}

#[tokio::test]
async fn the_real_open_claims_opens_what_the_orchestrator_signs() {
    let (_, resolver) = theirs();
    let signer = ours(30);
    for (method, path, body, thread, owner) in requests() {
        let request = Request {
            method,
            path: &path,
            body: &body,
            thread,
            owner: &owner,
        };
        let header = signer.sign(&request, 1_790_000_000).await.unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(&header).unwrap();

        let opened = open_claims(&resolver, &bytes, TYP, b"").await.unwrap();
        // the issuer is the key: the kid we derive is the one the real library derives
        assert_eq!(opened.kid, signer.kid());
        assert_eq!(
            opened.kid,
            kid_of(&SigningKey::from_seed(SEED).unwrap().public_key())
        );
        assert_eq!(format!("{:?}", opened.alg), "Ed25519");
        // the CWT claims
        assert_eq!(opened.claims.aud.as_deref(), Some(AUDIENCE));
        assert_eq!(opened.claims.iat, Some(1_790_000_000));
        assert_eq!(opened.claims.exp, Some(1_790_000_030));
        assert_eq!(opened.claims.cti.as_ref().map(Vec::len), Some(16));
        // the payload: the five members, as the contract says
        let payload: Value = serde_json::from_slice(&opened.payload).unwrap();
        assert_eq!(payload.as_object().unwrap().len(), 5);
        // written in the order of the contract, no whitespace (a parsed map would sort the members, so the bytes are read)
        let text = std::str::from_utf8(&opened.payload).unwrap();
        let members = [
            "\"method\":",
            "\"path\":",
            "\"body_sha256\":",
            "\"thread\":",
            "\"owner\":",
        ];
        let at: Vec<usize> = members.iter().map(|m| text.find(m).unwrap()).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(text.starts_with("{\"method\":") && !text.contains(": ") && !text.contains(", "));
        assert_eq!(payload["method"], method.as_str());
        assert_eq!(payload["path"], path);
        assert_eq!(payload["body_sha256"], hex(&Sha256::digest(&body)));
        assert_eq!(payload["thread"], thread.hyphenated().to_string());
        assert_eq!(payload["owner"], owner);
    }
}

#[tokio::test]
async fn the_orchestrator_signs_the_contract_s_typ_and_an_empty_external_aad() {
    let (_, resolver) = theirs();
    let signer = ours(30);
    let thread = Uuid::from_u128(1);
    let header = signer
        .sign(
            &Request {
                method: Method::Post,
                path: "/mcp",
                body: b"{}",
                thread,
                owner: "a@b.c",
            },
            1_790_000_000,
        )
        .await
        .unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(&header).unwrap();
    // another typ, another external aad
    assert!(matches!(
        open_claims(&resolver, &bytes, "application/other+cwt", b"").await,
        Err(ClaimsError::WrongType)
    ));
    assert!(matches!(
        open_claims(&resolver, &bytes, TYP, b"x").await,
        Err(ClaimsError::Unverified)
    ));
}

#[tokio::test]
async fn the_librarys_own_signer_writes_the_committed_vector() {
    let v = vectors();
    let (signer, _) = theirs();
    let claims = CwtClaims {
        aud: Some(v["audience"].as_str().unwrap().to_owned()),
        exp: Some(v["iat"].as_u64().unwrap() + v["ttl_secs"].as_u64().unwrap()),
        iat: v["iat"].as_u64(),
        cti: Some(unhex(v["cti"].as_str().unwrap())),
    };
    let payload = v["payload"].as_str().unwrap().as_bytes();
    let real = sign_claims(&signer, TYP, &claims, payload, b"")
        .await
        .unwrap();
    // the same bytes as the orchestrator's key (held by another Ed25519 implementation) writes in `vectors.rs`
    assert_eq!(hex(&real), v["cose_hex"].as_str().unwrap());
    assert_eq!(URL_SAFE_NO_PAD.encode(&real), v["header"].as_str().unwrap());
}

#[tokio::test]
async fn what_the_real_sign_claims_writes_the_orchestrators_reference_verifier_accepts() {
    // the other direction, for a request the real library signs on its own: the receiving side of the contract is not only
    // the inverse of our own writer
    let (signer, _) = theirs();
    let public = SigningKey::from_seed(SEED).unwrap().public_key();
    let body = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call"}"#;
    let thread = Uuid::from_u128(42);
    let payload = format!(
        r#"{{"method":"POST","path":"/mcp","body_sha256":"{}","thread":"{}","owner":"amina@example.com"}}"#,
        hex(&Sha256::digest(body)),
        thread.hyphenated()
    );
    let claims = CwtClaims {
        aud: Some(AUDIENCE.to_owned()),
        exp: Some(1_790_000_030),
        iat: Some(1_790_000_000),
        cti: Some(vec![5; 16]),
    };
    let real = sign_claims(&signer, TYP, &claims, payload.as_bytes(), b"")
        .await
        .unwrap();
    let header = URL_SAFE_NO_PAD.encode(real);
    let verified = Verifier::new(AUDIENCE, 30)
        .pin(public)
        .unwrap()
        .verify(
            &[&header],
            &Incoming {
                method: "POST",
                path: "/mcp",
                query: None,
                body,
                content_encoding: None,
            },
            1_790_000_010,
        )
        .await
        .unwrap();
    assert_eq!(
        (verified.thread, verified.owner.as_str()),
        (thread, "amina@example.com")
    );
}

#[tokio::test]
async fn the_reference_verifier_names_another_typ_as_the_wrong_type() {
    // signed by the pinned key and well formed: only the domain separator differs
    let (signer, _) = theirs();
    let public = SigningKey::from_seed(SEED).unwrap().public_key();
    let claims = CwtClaims {
        aud: Some(AUDIENCE.to_owned()),
        exp: Some(1_790_000_030),
        iat: Some(1_790_000_000),
        cti: Some(vec![5; 16]),
    };
    let other = sign_claims(&signer, "application/other+cwt", &claims, b"{}", b"")
        .await
        .unwrap();
    let refused = Verifier::new(AUDIENCE, 30)
        .pin(public)
        .unwrap()
        .verify(
            &[&URL_SAFE_NO_PAD.encode(other)],
            &Incoming {
                method: "POST",
                path: "/mcp",
                query: None,
                body: b"{}",
                content_encoding: None,
            },
            1_790_000_010,
        )
        .await
        .unwrap_err();
    assert_eq!(refused, VerifyError::Object(Object::WrongType));
}
