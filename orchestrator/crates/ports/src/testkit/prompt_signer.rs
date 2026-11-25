//! `PromptSigner` conformance cases: what the verifier behind the model gateway relies on from any signer of the
//! orchestrator's prompt statement (ADR 0087, the note of 2026-11-25), stated once and run against every adapter
//! (`orch-prompt-signer-static`, and the one over a PKI).
//!
//! An implementation supplies a fresh signer (`async fn() -> Option<S>`; `None` skips the suite) and the means to make
//! it unavailable, `unavailable = path::to::async_fn`, an `async fn(&S)` after which the signer has no certificate it
//! may sign with (a static signer is switched off; a PKI signer's certificate is let expire under a fake clock).
//!
//! Each case checks one rule, and a signer that breaks only that rule fails only that case (the static adapter's
//! `tests/kit_catches.rs` shows it). What is deliberately not asserted: the certificate chain (whether it builds to a
//! root, its SAN, its validity, the CRL), which is the verifier's to judge; the leaf is read only for its key.

use std::future::Future;
use std::time::Duration;

use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use orch_core::{Classify, ErrorClass};
use orch_prompt_statement::{
    A2aIds, AttestedPayload, JTI_LEN, Received, STATEMENT_TYP, Source, Statement, UserClaims,
    canonical, hash_of,
};
use serde_json::{Value, json};

use crate::{PromptSigner, SignedStatement, SignerError, SignerState};

/// The SubjectPublicKeyInfo of an Ed25519 key up to the key (RFC 8410): the 32 bytes that follow are the key.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

async fn within<T>(fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), fut)
        .await
        .expect("the case took more than 20 seconds")
}

/// A payload the integrity rule passed: statement `n`, with a person's `user` bound to its request.
pub fn payload(n: u8) -> AttestedPayload {
    let content = json!([{ "type": "text", "text": format!("prompt number {n}, déjà vu 🙂") }]);
    let props = json!({ "vymalo": { "accessTier": "read" }, "maxTurns": n, "temperature": 0.25 });
    let msg = format!("msg-{n}");
    let received = Received {
        thread: "thread-conformance",
        agent: "adam",
        msg: &msg,
        content: &content,
        forwarded_props: Some(&props),
    };
    Statement {
        aud: "https://models.test.invalid".into(),
        iat: 1_795_000_000,
        exp: 1_795_000_000 + 3600,
        jti: [n; JTI_LEN],
        src: Source::Chat,
        sub: "person-conformance".into(),
        azp: "app-conformance".into(),
        thread: "thread-conformance".into(),
        agent: "coder".into(),
        a2a: A2aIds {
            context_id: "ctx-conformance".into(),
            message_id: format!("a2a-msg-{n}"),
        },
        content_sha256: hash_of(&content),
        user: Some(UserClaims::from_request(
            "https://api.test.invalid",
            &received,
            "eyJhbGciOiJFZERTQSJ9..c2ln",
        )),
    }
    .attest(Some(&received))
    .expect("the conformance payload is attested")
}

/// The three parts of a compact JWS, decoded: header bytes, payload bytes, signature bytes, and the signing input.
pub struct Parts {
    /// The protected header's bytes.
    pub header: Vec<u8>,
    /// The attached payload's bytes.
    pub payload: Vec<u8>,
    /// The signature's bytes.
    pub signature: Vec<u8>,
    /// `header.payload` as written.
    pub signing_input: String,
}

/// Splits and decodes a compact JWS; panics with what is wrong.
pub fn parts(statement: &SignedStatement) -> Parts {
    let text = statement.as_str();
    let segments: Vec<&str> = text.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a compact JWS has three parts separated by dots"
    );
    let decode = |what: &str, s: &str| {
        URL_SAFE_NO_PAD
            .decode(s)
            .unwrap_or_else(|_| panic!("the {what} is not base64url without padding"))
    };
    let (signing_input, _) = text.rsplit_once('.').expect("three parts");
    Parts {
        header: decode("header", segments[0]),
        payload: decode("payload", segments[1]),
        signature: decode("signature", segments[2]),
        signing_input: signing_input.to_owned(),
    }
}

/// The DER of each `x5c` certificate; panics when the header has none.
pub fn x5c(header: &[u8]) -> Vec<Vec<u8>> {
    let value: Value = serde_json::from_slice(header).expect("the protected header is JSON");
    let chain = value["x5c"]
        .as_array()
        .expect("the protected header has an x5c array");
    assert!(!chain.is_empty(), "x5c names at least the leaf");
    chain
        .iter()
        .map(|c| {
            STANDARD
                .decode(c.as_str().expect("an x5c entry is a string"))
                .expect("an x5c entry is standard base64 DER")
        })
        .collect()
}

/// The Ed25519 key of a certificate: the 32 bytes after the one Ed25519 SubjectPublicKeyInfo prefix in its DER.
pub fn ed25519_key_of(der: &[u8]) -> [u8; 32] {
    let at = der
        .windows(ED25519_SPKI_PREFIX.len())
        .position(|w| w == ED25519_SPKI_PREFIX)
        .expect("the leaf holds an Ed25519 public key");
    der[at + ED25519_SPKI_PREFIX.len()..][..32]
        .try_into()
        .expect("32 bytes of key")
}

async fn sign<S: PromptSigner>(signer: &S, payload: &AttestedPayload) -> SignedStatement {
    signer
        .sign(payload)
        .await
        .expect("a signer that is not unavailable signs")
}

/// The payload travels attached, byte for byte: the verifier reads the statement from the header and nothing else.
pub async fn the_payload_is_attached_byte_for_byte<S: PromptSigner>(signer: S) {
    within(async {
        let payload = payload(1);
        let statement = sign(&signer, &payload).await;
        assert_eq!(parts(&statement).payload, payload.as_bytes());
    })
    .await;
}

/// The protected header is exactly `{"alg":"EdDSA","typ":"prompt-statement/v1","x5c":[...]}`, in RFC 8785 form.
pub async fn the_protected_header_is_the_statements<S: PromptSigner>(signer: S) {
    within(async {
        let statement = sign(&signer, &payload(2)).await;
        let header = parts(&statement).header;
        let value: Value = serde_json::from_slice(&header).expect("the protected header is JSON");
        assert_eq!(value["alg"], "EdDSA", "alg");
        assert_eq!(value["typ"], STATEMENT_TYP, "typ");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("the protected header is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["alg", "typ", "x5c"], "the header holds nothing else");
        x5c(&header);
        assert_eq!(header, canonical(&value), "the header is RFC 8785 JSON");
    })
    .await;
}

/// The signature is Ed25519 over the signing input, under the key of the first `x5c` certificate.
pub async fn the_signature_verifies_under_the_leaf_key<S: PromptSigner>(signer: S) {
    within(async {
        let statement = sign(&signer, &payload(3)).await;
        let parts = parts(&statement);
        let leaf = x5c(&parts.header).remove(0);
        let key = ed25519_key_of(&leaf);
        UnparsedPublicKey::new(&ED25519, key)
            .verify(parts.signing_input.as_bytes(), &parts.signature)
            .expect("the signature verifies under the leaf's key");
    })
    .await;
}

/// Two payloads signed at once give two statements, each carrying and signing its own payload.
pub async fn each_payload_gets_its_own_statement<S: PromptSigner>(signer: S) {
    within(async {
        let (a, b) = (payload(4), payload(5));
        let (sa, sb) = futures::join!(sign(&signer, &a), sign(&signer, &b));
        assert_ne!(sa, sb, "two payloads, two statements");
        for (statement, payload) in [(&sa, &a), (&sb, &b)] {
            let parts = parts(statement);
            assert_eq!(parts.payload, payload.as_bytes(), "each carries its own");
            let key = ed25519_key_of(&x5c(&parts.header)[0]);
            UnparsedPublicKey::new(&ED25519, key)
                .verify(parts.signing_input.as_bytes(), &parts.signature)
                .expect("each is signed over its own payload");
        }
    })
    .await;
}

/// A signer that signs does not say it is unavailable.
pub async fn a_signer_that_signs_is_not_unavailable<S: PromptSigner>(signer: S) {
    within(async {
        assert_ne!(
            signer.state(),
            SignerState::Unavailable,
            "a fresh signer can sign"
        );
        sign(&signer, &payload(6)).await;
        assert_ne!(signer.state(), SignerState::Unavailable);
    })
    .await;
}

/// An unavailable signer says so and refuses, with an error that may succeed later and names no secret.
pub async fn an_unavailable_signer_refuses_and_says_so<S: PromptSigner>(
    signer: S,
    make_unavailable: impl AsyncFnOnce(&S),
) {
    within(async {
        make_unavailable(&signer).await;
        assert_eq!(signer.state(), SignerState::Unavailable);
        let error = signer
            .sign(&payload(7))
            .await
            .expect_err("an unavailable signer signs nothing");
        assert!(
            matches!(error, SignerError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
    })
    .await;
}
