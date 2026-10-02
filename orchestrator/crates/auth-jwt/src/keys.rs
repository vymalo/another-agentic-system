//! The issuer's keys: a JWKS read into the verification keys this authenticator can use, and the
//! choice of the one key that may verify a token.
//!
//! Only keys that can verify what the allowed algorithms sign are kept: an RSA key of at least
//! 2048 bits (RS256, RS384), a P-256 key (ES256), an Ed25519 key (EdDSA). Anything else in the
//! set (a symmetric key, a P-384 key, a key marked for encryption) is skipped, never an error
//! for the rest. A key never decides its own algorithm: the token's `alg` must be one of the
//! allowed ones **and** of the key's family, whatever the token or the key's `alg` says.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde::Deserialize;
use serde_json::Value;

/// The algorithms a token may be signed with. Nothing else is read: not `none`, not the HMAC
/// family (whose "key" would be a public key the attacker has), not RS512, PS* or ES384.
pub(crate) const ALLOWED: [Algorithm; 4] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::ES256,
    Algorithm::EdDSA,
];

/// The smallest RSA modulus, in bytes (2048 bits).
const MIN_RSA_MODULUS_BYTES: usize = 256;

/// The kind of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Rsa,
    EcP256,
    Ed25519,
}

/// The kind of key an allowed algorithm needs; `None` for an algorithm that is not allowed.
pub(crate) fn kind_of(alg: Algorithm) -> Option<Kind> {
    match alg {
        Algorithm::RS256 | Algorithm::RS384 => Some(Kind::Rsa),
        Algorithm::ES256 => Some(Kind::EcP256),
        Algorithm::EdDSA => Some(Kind::Ed25519),
        _ => None,
    }
}

/// The JWK members this reads (RFC 7517, RFC 7518 §6, RFC 8037). Unknown members are ignored.
#[derive(Debug, Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    alg: Option<String>,
    key_ops: Option<Vec<String>>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

/// A key that can verify.
pub(crate) struct Key {
    /// `kid`, when the key has one.
    pub(crate) kid: Option<String>,
    /// The algorithm the key declares (`alg`), when it does: a token's algorithm must be it.
    declared: Option<Algorithm>,
    kind: Kind,
    pub(crate) decoding: DecodingKey,
}

/// The keys of one fetch of the JWKS.
#[derive(Default)]
pub(crate) struct KeySet {
    keys: Vec<Key>,
}

impl std::fmt::Debug for KeySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeySet")
            .field("keys", &self.keys.len())
            .finish()
    }
}

/// How many keys were read and how many were skipped, for the log.
pub(crate) struct Parsed {
    pub(crate) set: KeySet,
    pub(crate) skipped: usize,
}

fn algorithm(name: &str) -> Option<Algorithm> {
    ALLOWED.into_iter().find(|alg| {
        matches!(
            (alg, name),
            (Algorithm::RS256, "RS256")
                | (Algorithm::RS384, "RS384")
                | (Algorithm::ES256, "ES256")
                | (Algorithm::EdDSA, "EdDSA")
        )
    })
}

fn b64_len(text: &str) -> Option<usize> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .ok()
        .map(|bytes| bytes.iter().skip_while(|b| **b == 0).count())
}

/// One JWK as a key, or `None` when this authenticator cannot use it.
fn read_key(jwk: &Jwk) -> Option<Key> {
    // Not for signatures, or not for verifying them.
    if jwk.usage.as_deref().is_some_and(|u| u != "sig") {
        return None;
    }
    if jwk
        .key_ops
        .as_ref()
        .is_some_and(|ops| !ops.iter().any(|op| op == "verify"))
    {
        return None;
    }
    // A key that says it is for an algorithm this does not allow is not used for another.
    let declared = match jwk.alg.as_deref() {
        None => None,
        Some(name) => Some(algorithm(name)?),
    };
    let (kind, decoding) = match jwk.kty.as_str() {
        "RSA" => {
            let (n, e) = (jwk.n.as_deref()?, jwk.e.as_deref()?);
            if b64_len(n)? < MIN_RSA_MODULUS_BYTES || b64_len(e)? == 0 {
                return None;
            }
            (Kind::Rsa, DecodingKey::from_rsa_components(n, e).ok()?)
        }
        "EC" => {
            if jwk.crv.as_deref() != Some("P-256") {
                return None;
            }
            let (x, y) = (jwk.x.as_deref()?, jwk.y.as_deref()?);
            // A P-256 coordinate is 32 bytes; its leading zeros may have been cut or kept.
            if b64_len(x)? > 32 || b64_len(y)? > 32 {
                return None;
            }
            (Kind::EcP256, DecodingKey::from_ec_components(x, y).ok()?)
        }
        "OKP" => {
            if jwk.crv.as_deref() != Some("Ed25519") {
                return None;
            }
            let x = jwk.x.as_deref()?;
            if b64_len(x)? != 32 {
                return None;
            }
            (Kind::Ed25519, DecodingKey::from_ed_components(x).ok()?)
        }
        _ => return None,
    };
    if let Some(declared) = declared
        && kind_of(declared) != Some(kind)
    {
        return None;
    }
    Some(Key {
        kid: jwk.kid.clone(),
        declared,
        kind,
        decoding,
    })
}

impl KeySet {
    /// The keys of a JWKS document (`{"keys": [...]}`). A key that cannot be read, or that this
    /// cannot use, is skipped; a document that is not a JWKS is `None`.
    pub(crate) fn parse(document: &Value) -> Option<Parsed> {
        let listed = document.get("keys")?.as_array()?;
        let mut keys = Vec::new();
        let mut skipped = 0;
        for item in listed {
            match serde_json::from_value::<Jwk>(item.clone())
                .ok()
                .and_then(|jwk| read_key(&jwk))
            {
                Some(key) => keys.push(key),
                None => skipped += 1,
            }
        }
        Some(Parsed {
            set: KeySet { keys },
            skipped,
        })
    }

    /// Whether the set has a key to verify with.
    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The key that may verify a token with this `alg` and `kid`: a key of the algorithm's kind,
    /// whose declared algorithm (if any) is `alg`, and whose `kid` is the token's. A token with
    /// no `kid` takes the one key of that kind, when there is exactly one.
    pub(crate) fn select(&self, alg: Algorithm, kid: Option<&str>) -> Option<&Key> {
        let kind = kind_of(alg)?;
        let mut fits = self
            .keys
            .iter()
            .filter(|k| k.kind == kind && k.declared.is_none_or(|d| d == alg))
            .filter(|k| kid.is_none_or(|kid| k.kid.as_deref() == Some(kid)));
        let first = fits.next()?;
        match kid {
            Some(_) => Some(first),
            None => fits.next().is_none().then_some(first),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rsa(kid: &str, extra: Value) -> Value {
        let n = URL_SAFE_NO_PAD.encode([0xC3u8; 256]);
        let mut jwk = json!({"kty": "RSA", "kid": kid, "n": n, "e": "AQAB"});
        jwk.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        jwk
    }

    fn ec(kid: &str, crv: &str) -> Value {
        let c = URL_SAFE_NO_PAD.encode([7u8; 32]);
        json!({"kty": "EC", "crv": crv, "kid": kid, "x": c, "y": c})
    }

    fn okp(kid: &str, crv: &str, len: usize) -> Value {
        json!({"kty": "OKP", "crv": crv, "kid": kid, "x": URL_SAFE_NO_PAD.encode(vec![9u8; len])})
    }

    fn parse(keys: Vec<Value>) -> Parsed {
        KeySet::parse(&json!({ "keys": keys })).unwrap()
    }

    #[test]
    fn only_the_keys_the_allowed_algorithms_can_use_are_kept() {
        let parsed = parse(vec![
            rsa("rsa", json!({})),
            ec("p256", "P-256"),
            okp("ed", "Ed25519", 32),
            // not usable:
            ec("p384", "P-384"),
            okp("x448", "X25519", 32),
            okp("short", "Ed25519", 31),
            json!({"kty": "oct", "kid": "hmac", "k": "c2VjcmV0"}),
            rsa("enc", json!({"use": "enc"})),
            rsa("rs512", json!({"alg": "RS512"})),
            rsa("sign-only", json!({"key_ops": ["sign"]})),
            json!({"kty": "RSA", "kid": "small", "n": URL_SAFE_NO_PAD.encode([1u8; 128]), "e": "AQAB"}),
            json!({"kty": "EC"}),
            json!("not an object"),
        ]);
        assert_eq!(parsed.set.keys.len(), 3);
        assert_eq!(parsed.skipped, 10);
    }

    #[test]
    fn a_document_that_is_not_a_jwks_is_nothing() {
        assert!(KeySet::parse(&json!({"issuer": "x"})).is_none());
        assert!(KeySet::parse(&json!({"keys": "no"})).is_none());
        assert!(KeySet::parse(&json!([])).is_none());
        assert!(KeySet::parse(&json!({"keys": []})).unwrap().set.is_empty());
    }

    #[test]
    fn a_token_takes_the_key_of_its_kid_and_its_algorithms_kind() {
        let set = parse(vec![
            rsa("a", json!({})),
            rsa("b", json!({})),
            ec("c", "P-256"),
            okp("d", "Ed25519", 32),
        ])
        .set;
        assert_eq!(
            set.select(Algorithm::RS256, Some("a"))
                .unwrap()
                .kid
                .as_deref(),
            Some("a")
        );
        assert_eq!(
            set.select(Algorithm::RS384, Some("b"))
                .unwrap()
                .kid
                .as_deref(),
            Some("b")
        );
        // The right kid, the wrong kind of key for the algorithm: no key.
        assert!(set.select(Algorithm::ES256, Some("a")).is_none());
        assert!(set.select(Algorithm::RS256, Some("c")).is_none());
        assert!(set.select(Algorithm::EdDSA, Some("c")).is_none());
        assert!(set.select(Algorithm::RS256, Some("nope")).is_none());
        // An algorithm that is not allowed has no kind, so no key.
        assert!(set.select(Algorithm::HS256, Some("a")).is_none());
        assert!(set.select(Algorithm::PS256, Some("a")).is_none());
    }

    #[test]
    fn a_token_without_a_kid_takes_the_only_key_of_its_kind() {
        let one = parse(vec![rsa("a", json!({})), ec("c", "P-256")]).set;
        assert!(one.select(Algorithm::RS256, None).is_some());
        assert!(one.select(Algorithm::ES256, None).is_some());
        assert!(one.select(Algorithm::EdDSA, None).is_none());
        let two = parse(vec![rsa("a", json!({})), rsa("b", json!({}))]).set;
        assert!(two.select(Algorithm::RS256, None).is_none());
    }

    #[test]
    fn a_key_that_declares_an_algorithm_verifies_only_that_one() {
        let set = parse(vec![rsa("a", json!({"alg": "RS256"}))]).set;
        assert!(set.select(Algorithm::RS256, Some("a")).is_some());
        assert!(set.select(Algorithm::RS384, Some("a")).is_none());
        // A declared algorithm of another kind makes the key unusable.
        assert!(
            parse(vec![rsa("a", json!({"alg": "ES256"}))])
                .set
                .is_empty()
        );
    }
}
