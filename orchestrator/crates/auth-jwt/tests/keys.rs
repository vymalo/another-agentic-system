//! Real keys, as an issuer may publish them: the leading zero octets of an Ed25519 key, of the coordinates of a
//! P-256 key and of the modulus and exponent of an RSA key (kept, cut, or covered by a sign byte), the padding
//! some issuers write, and the tokens these keys sign. Over real HTTP, against a server that serves the key
//! set a test builds. Two keys under one `kid`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize, PublicKeyComponents};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RSA_PKCS1_SHA256,
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE;
use common::{
    USER, assert_accepted, authenticator, b64, check, good_claims, refusal, serve_keys, token,
};
use orch_auth_jwt::JwtAuth;
use orch_ports::{AuthError, Authenticator};
use serde_json::{Value, json};

/// An authenticator for an issuer whose key set is `keys`, and that issuer's name.
async fn issuer_with(keys: Vec<Value>) -> (JwtAuth, String) {
    let base = serve_keys(keys).await;
    (authenticator(&base), base)
}

/// A token for `issuer` signed by `sign`, naming the key `kid`.
fn signed(alg: &str, kid: &str, issuer: &str, sign: impl FnOnce(&[u8]) -> Vec<u8>) -> String {
    token(alg, kid, &good_claims(issuer), sign)
}

async fn accepted(auth: &JwtAuth, token: &str, what: &str) {
    let principal = assert_accepted(check(auth, token).await, what);
    assert_eq!(principal.user.as_str(), USER, "{what}");
}

/// The key set has no key this build can verify with: the issuer is `Unavailable`.
async fn assert_not_a_key(keys: Vec<Value>, what: &str) {
    let (auth, _) = issuer_with(keys).await;
    let error = auth.ready().await.unwrap_err();
    assert!(
        matches!(error, AuthError::Unavailable { .. }),
        "{what}: {error:?}"
    );
}

// ---- Ed25519 ---------------------------------------------------------------------------------------------------

/// An Ed25519 key whose public key starts with a zero octet (about one in 256).
fn ed25519_starting_with_zero() -> Ed25519KeyPair {
    for _ in 0..20_000 {
        let key = Ed25519KeyPair::generate().unwrap();
        if key.public_key().as_ref()[0] == 0 {
            return key;
        }
    }
    panic!("no Ed25519 key with a leading zero in 20000 tries");
}

#[tokio::test]
async fn an_ed25519_key_whose_public_key_starts_with_a_zero_octet_verifies() {
    let key = ed25519_starting_with_zero();
    let public = key.public_key().as_ref().to_vec();
    assert_eq!((public.len(), public[0]), (32, 0));
    let (auth, issuer) = issuer_with(vec![
        json!({"kty": "OKP", "crv": "Ed25519", "kid": "zero", "x": b64(&public)}),
    ])
    .await;
    let signed = signed("EdDSA", "zero", &issuer, |m| key.sign(m).as_ref().to_vec());
    accepted(&auth, &signed, "an Ed25519 key with a leading zero").await;
}

#[tokio::test]
async fn an_ed25519_key_of_another_length_is_not_a_key() {
    // Raw octets, exactly 32: a zero octet is a part of the key, not padding to be taken off.
    let key = Ed25519KeyPair::generate().unwrap();
    let public = key.public_key().as_ref().to_vec();
    for (what, x) in [
        ("31 octets", public[1..].to_vec()),
        ("33 octets, a zero in front", [&[0u8][..], &public].concat()),
        ("none", Vec::new()),
    ] {
        assert_not_a_key(
            vec![json!({"kty": "OKP", "crv": "Ed25519", "kid": "k", "x": b64(&x)})],
            what,
        )
        .await;
    }
}

#[tokio::test]
async fn an_ed25519_key_written_with_padding_verifies() {
    // RFC 7515 forbids the `=` padding of base64url, and some issuers write it all the same.
    let key = Ed25519KeyPair::generate().unwrap();
    let padded = URL_SAFE.encode(key.public_key().as_ref());
    assert!(padded.ends_with('='), "{padded}");
    let (auth, issuer) = issuer_with(vec![
        json!({"kty": "OKP", "crv": "Ed25519", "kid": "padded", "x": padded}),
    ])
    .await;
    let signed = signed("EdDSA", "padded", &issuer, |m| {
        key.sign(m).as_ref().to_vec()
    });
    accepted(&auth, &signed, "an Ed25519 key written with padding").await;
}

// ---- P-256 -------------------------------------------------------------------------------------------------------

struct Ec {
    key: EcdsaKeyPair,
    x: Vec<u8>,
    y: Vec<u8>,
}

impl Ec {
    /// A P-256 key whose `x` (or, with `in_y`, whose `y`) starts with a zero octet.
    fn starting_with_zero(in_y: bool) -> Self {
        for _ in 0..20_000 {
            let key = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
            let point = key.public_key().as_ref().to_vec();
            assert_eq!((point.len(), point[0]), (65, 4));
            let (x, y) = (point[1..33].to_vec(), point[33..65].to_vec());
            if (if in_y { y[0] } else { x[0] }) == 0 {
                return Self { key, x, y };
            }
        }
        panic!("no P-256 key with a leading zero in 20000 tries");
    }

    fn jwk(&self, x: &str, y: &str) -> Value {
        json!({"kty": "EC", "crv": "P-256", "kid": "ec", "x": x, "y": y})
    }

    fn token(&self, issuer: &str) -> String {
        signed("ES256", "ec", issuer, |m| {
            self.key
                .sign(&SystemRandom::new(), m)
                .unwrap()
                .as_ref()
                .to_vec()
        })
    }
}

/// `bytes` without its leading zero octets.
fn trimmed(bytes: &[u8]) -> Vec<u8> {
    let zeros = bytes.iter().take_while(|b| **b == 0).count();
    bytes[zeros..].to_vec()
}

#[tokio::test]
async fn a_p256_coordinate_with_a_leading_zero_verifies_however_the_issuer_writes_it() {
    for in_y in [false, true] {
        let ec = Ec::starting_with_zero(in_y);
        let coordinate = if in_y { &ec.y } else { &ec.x };
        let other = b64(if in_y { &ec.x } else { &ec.y });
        let write = |c: String| {
            if in_y {
                ec.jwk(&other, &c)
            } else {
                ec.jwk(&c, &other)
            }
        };
        for (what, written) in [
            ("in full, 32 octets", b64(coordinate)),
            (
                "with the zeros cut, 31 octets or fewer",
                b64(trimmed(coordinate)),
            ),
            (
                "with a sign byte, 33 octets",
                b64([&[0u8][..], coordinate].concat()),
            ),
            ("with the padding of base64", URL_SAFE.encode(coordinate)),
        ] {
            let (auth, issuer) = issuer_with(vec![write(written)]).await;
            accepted(
                &auth,
                &ec.token(&issuer),
                &format!("{} {what}", if in_y { "y" } else { "x" }),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_p256_coordinate_that_cannot_be_one_is_not_a_key() {
    let ec = Ec::starting_with_zero(false);
    let (x, y) = (b64(&ec.x), b64(&ec.y));
    let too_long = b64([&[1u8][..], &ec.x].concat());
    for (what, jwk) in [
        ("33 significant octets", ec.jwk(&too_long, &y)),
        ("an empty x", ec.jwk("", &y)),
        ("an empty y", ec.jwk(&x, "")),
    ] {
        assert_not_a_key(vec![jwk], what).await;
    }
}

// ---- RSA ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_rsa_key_verifies_with_a_sign_byte_in_front_of_its_modulus_and_its_exponent() {
    let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
    let components = PublicKeyComponents::<Vec<u8>>::from(key.public_key());
    assert_eq!(components.n.len(), 256, "a 2048-bit modulus");
    assert!(
        components.n[0] >= 0x80,
        "a high bit: a DER integer needs a sign byte"
    );
    let sign = |m: &[u8]| {
        let mut signature = vec![0; key.public_modulus_len()];
        key.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), m, &mut signature)
            .unwrap();
        signature
    };
    let with_zero = |bytes: &[u8]| [&[0u8][..], bytes].concat();
    for (what, n, e) in [
        (
            "as the key writes them",
            b64(&components.n),
            b64(&components.e),
        ),
        (
            "with a sign byte before n and e written AAEAAQ",
            b64(with_zero(&components.n)),
            "AAEAAQ".to_owned(),
        ),
        (
            "with a sign byte before n only",
            b64(with_zero(&components.n)),
            "AQAB".to_owned(),
        ),
        (
            "with the padding of base64",
            URL_SAFE.encode(&components.n),
            URL_SAFE.encode(&components.e),
        ),
    ] {
        let (auth, issuer) =
            issuer_with(vec![json!({"kty": "RSA", "kid": "rsa", "n": n, "e": e})]).await;
        accepted(&auth, &signed("RS256", "rsa", &issuer, sign), what).await;
    }
}

#[tokio::test]
async fn an_rsa_modulus_under_2048_bits_is_not_made_big_enough_by_a_sign_byte() {
    // 255 significant octets with a zero in front is 256 octets long and still a 2040-bit modulus.
    let n = [&[0u8][..], &[0xC3u8; 255]].concat();
    assert_not_a_key(
        vec![json!({"kty": "RSA", "kid": "small", "n": b64(&n), "e": "AQAB"})],
        "a 2040-bit modulus",
    )
    .await;
    // An exponent of zeros is none.
    assert_not_a_key(
        vec![json!({"kty": "RSA", "kid": "no-e", "n": b64([0xC3u8; 256]), "e": b64([0u8, 0])})],
        "an exponent of zeros",
    )
    .await;
}

// ---- a kid that two keys share -----------------------------------------------------------------------------------

#[tokio::test]
async fn every_key_of_a_shared_kid_is_tried_and_not_only_the_first() {
    // A JWKS may hold two keys under one kid (a rotation that kept the name, two issuers merged): the token is
    // good if any of them signed it.
    let (first, second, stranger) = (
        Ed25519KeyPair::generate().unwrap(),
        Ed25519KeyPair::generate().unwrap(),
        Ed25519KeyPair::generate().unwrap(),
    );
    let jwk = |key: &Ed25519KeyPair| json!({"kty": "OKP", "crv": "Ed25519", "kid": "shared", "x": b64(key.public_key().as_ref())});
    let (auth, issuer) = issuer_with(vec![jwk(&first), jwk(&second)]).await;
    for (name, key) in [("the first", &first), ("the second", &second)] {
        let signed = signed("EdDSA", "shared", &issuer, |m| {
            key.sign(m).as_ref().to_vec()
        });
        accepted(&auth, &signed, &format!("signed by {name} key of the kid")).await;
    }
    // A key that is not in the set is still a bad signature, and a token with no kid still needs one key only.
    let forged = signed("EdDSA", "shared", &issuer, |m| {
        stranger.sign(m).as_ref().to_vec()
    });
    let why = refusal(check(&auth, &forged).await, "signed by none of them");
    assert!(why.contains("signature"), "{why}");
    let anonymous = format!(
        "{}.{}",
        b64(br#"{"alg":"EdDSA","typ":"JWT"}"#),
        forged.split_once('.').unwrap().1
    );
    let why = refusal(
        check(&auth, &anonymous).await,
        "no kid and two keys of the kind",
    );
    assert!(why.contains("signing key"), "{why}");
}

#[tokio::test]
async fn the_claims_are_read_from_the_key_that_signed_not_from_the_first_of_the_kid() {
    // The first key's signature does not verify; the second's does, and the token has expired: the word is
    // "expired", which only a key that verified can say.
    let (first, second) = (
        Ed25519KeyPair::generate().unwrap(),
        Ed25519KeyPair::generate().unwrap(),
    );
    let jwk = |key: &Ed25519KeyPair| json!({"kty": "OKP", "crv": "Ed25519", "kid": "shared", "x": b64(key.public_key().as_ref())});
    let (auth, issuer) = issuer_with(vec![jwk(&first), jwk(&second)]).await;
    let mut claims = good_claims(&issuer);
    claims.insert("exp".into(), json!(common::now() - 3600));
    let expired = token("EdDSA", "shared", &claims, |m| {
        second.sign(m).as_ref().to_vec()
    });
    let why = refusal(
        check(&auth, &expired).await,
        "expired, signed by the second",
    );
    assert!(why.contains("expired"), "{why}");
}
