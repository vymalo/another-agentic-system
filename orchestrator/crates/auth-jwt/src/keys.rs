//! The issuer's keys: a JWKS read into the verification keys this authenticator can use, and the
//! choice of the one key that may verify a token.
//!
//! Only keys that can verify what the allowed algorithms sign are kept: an RSA key of at least
//! 2048 bits (RS256, RS384), a P-256 key (ES256), an Ed25519 key (EdDSA). Anything else in the
//! set (a symmetric key, a P-384 key, a key marked for encryption) is skipped, never an error
//! for the rest. A key never decides its own algorithm: the token's `alg` must be one of the
//! allowed ones **and** of the key's family, whatever the token or the key's `alg` says.
//!
//! The members of a JWK are read as the numbers they are, not as the text an issuer happens to write: the
//! library wants an RSA integer as its magnitude, a P-256 coordinate in the 32 octets of the curve and an
//! Ed25519 key in its 32 octets, and issuers write them cut, in full, with a sign byte, or padded with `=`.

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

/// The smallest RSA modulus, in bits, counted on its magnitude (the bits from its highest set bit): a sign byte
/// makes it no bigger, and 256 octets whose first is `0x01` are 2041 bits, which aws-lc refuses at verification.
const MIN_RSA_MODULUS_BITS: usize = 2048;

/// The size of a P-256 coordinate, in bytes.
const P256_COORDINATE_BYTES: usize = 32;

/// The size of an Ed25519 public key, in bytes.
const ED25519_KEY_BYTES: usize = 32;

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

/// The octets a JWK member writes in base64url. Padding, which RFC 7515 forbids and some issuers write, is
/// accepted.
fn octets(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

/// The octets of an unsigned big-endian integer without its leading zeros, which are a sign byte or padding
/// (RFC 7518 6.3.1 writes `n` and `e` without them; RFC 7518 6.2.1.2 writes a coordinate in the full length of
/// its curve; issuers do both, and some add a sign byte).
fn magnitude(bytes: &[u8]) -> &[u8] {
    let zeros = bytes.iter().take_while(|b| **b == 0).count();
    &bytes[zeros..]
}

/// The bits of an unsigned big-endian integer from its highest set bit: 0 for no octets (`magnitude` has taken
/// the zeros off, so the first octet, when there is one, is not zero).
fn bit_length(magnitude: &[u8]) -> usize {
    match magnitude.first() {
        None => 0,
        Some(top) => magnitude.len() * 8 - top.leading_zeros() as usize,
    }
}

fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A P-256 coordinate written by an issuer in any of its ways (cut, full, with a sign byte), as the 32 octets
/// the key is made of, in base64url. `None` for what cannot be one: an empty member, or more than 32 significant
/// octets. Zeros are a value: x = 0 is a valid abscissa of P-256.
fn p256_coordinate(text: &str) -> Option<String> {
    let bytes = octets(text)?;
    let value = magnitude(&bytes);
    if bytes.is_empty() || value.len() > P256_COORDINATE_BYTES {
        return None;
    }
    let mut padded = [0u8; P256_COORDINATE_BYTES];
    padded[P256_COORDINATE_BYTES - value.len()..].copy_from_slice(value);
    Some(base64url(&padded))
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
            let (n, e) = (octets(jwk.n.as_deref()?)?, octets(jwk.e.as_deref()?)?);
            // The library takes the integers as they are written: a sign byte in front would make a key that
            // is admitted and never verifies.
            let (n, e) = (magnitude(&n), magnitude(&e));
            if e.is_empty() || bit_length(n) < MIN_RSA_MODULUS_BITS {
                return None;
            }
            (
                Kind::Rsa,
                DecodingKey::from_rsa_components(&base64url(n), &base64url(e)).ok()?,
            )
        }
        "EC" => {
            if jwk.crv.as_deref() != Some("P-256") {
                return None;
            }
            // The library wants each coordinate in the 32 octets of the curve; an issuer may have cut its leading
            // zeros or covered them with a sign byte.
            let (x, y) = (
                p256_coordinate(jwk.x.as_deref()?)?,
                p256_coordinate(jwk.y.as_deref()?)?,
            );
            (Kind::EcP256, DecodingKey::from_ec_components(&x, &y).ok()?)
        }
        "OKP" => {
            if jwk.crv.as_deref() != Some("Ed25519") {
                return None;
            }
            // The key is its 32 octets, a zero in front included: nothing is cut and nothing is added.
            let x = octets(jwk.x.as_deref()?)?;
            if x.len() != ED25519_KEY_BYTES {
                return None;
            }
            (
                Kind::Ed25519,
                DecodingKey::from_ed_components(&base64url(&x)).ok()?,
            )
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

    /// The keys that may verify a token with this `alg` and `kid`: those of the algorithm's kind whose declared
    /// algorithm (if any) is `alg` and whose `kid` is the token's, **all of them** (a JWKS may hold two keys
    /// under one `kid`, a rotation that kept the name or two sets merged: the token is good if any of them
    /// signed it). A token with no `kid` takes the one key of that kind, when there is exactly one, and none
    /// otherwise.
    pub(crate) fn select(&self, alg: Algorithm, kid: Option<&str>) -> Vec<&Key> {
        let Some(kind) = kind_of(alg) else {
            return Vec::new();
        };
        let fits: Vec<&Key> = self
            .keys
            .iter()
            .filter(|k| k.kind == kind && k.declared.is_none_or(|d| d == alg))
            .filter(|k| kid.is_none_or(|kid| k.kid.as_deref() == Some(kid)))
            .collect();
        if kid.is_none() && fits.len() != 1 {
            return Vec::new();
        }
        fits
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
    fn an_ed25519_key_is_its_32_octets_and_a_leading_zero_is_a_part_of_it() {
        let x = |bytes: Vec<u8>| json!({"kty": "OKP", "crv": "Ed25519", "kid": "k", "x": URL_SAFE_NO_PAD.encode(bytes)});
        let mut zero_led = vec![9u8; 32];
        zero_led[0] = 0;
        assert_eq!(parse(vec![x(zero_led.clone())]).set.keys.len(), 1);
        assert_eq!(parse(vec![x(vec![9u8; 32])]).set.keys.len(), 1);
        // 31 octets: one too few. 33 with a zero in front: one too many, which is not a sign byte here.
        assert!(parse(vec![x(zero_led[1..].to_vec())]).set.is_empty());
        assert!(
            parse(vec![x([&[0u8][..], &[9u8; 32]].concat())])
                .set
                .is_empty()
        );
        assert!(parse(vec![x(Vec::new())]).set.is_empty());
    }

    #[test]
    fn rsa_integers_and_ec_coordinates_are_read_without_their_sign_bytes_and_zeros() {
        let enc = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
        let modulus = [0xC3u8; 256];
        let signed = [&[0u8][..], &modulus].concat();
        let rsa = |n: &[u8], e: &[u8]| json!({"kty": "RSA", "kid": "k", "n": enc(n), "e": enc(e)});
        for (n, e) in [
            (&modulus[..], &[1u8, 0, 1][..]),
            (&signed[..], &[1u8, 0, 1][..]),
            (&signed[..], &[0u8, 1, 0, 1][..]),
        ] {
            assert_eq!(parse(vec![rsa(n, e)]).set.keys.len(), 1);
        }
        // 255 significant octets are 2040 bits, however many zeros are in front; an exponent of zeros is none.
        let small = [&[0u8, 0][..], &[0xC3u8; 255]].concat();
        assert!(parse(vec![rsa(&small, &[1, 0, 1])]).set.is_empty());
        assert!(parse(vec![rsa(&modulus, &[0, 0])]).set.is_empty());
        assert!(parse(vec![rsa(&modulus, &[])]).set.is_empty());

        let mut x = [7u8; 32];
        x[0] = 0;
        let y = [7u8; 32];
        let ec = |x: &[u8], y: &[u8]| json!({"kty": "EC", "crv": "P-256", "kid": "k", "x": enc(x), "y": enc(y)});
        for written in [
            x.to_vec(),                        // in full
            x[1..].to_vec(),                   // the zero cut
            [&[0u8][..], &x].concat(),         // a sign byte in front
            [&[0u8, 0][..], &x[1..]].concat(), // padded with more zeros
        ] {
            assert_eq!(parse(vec![ec(&written, &y)]).set.keys.len(), 1);
        }
        // More than 32 significant octets, and an empty member, cannot be a coordinate.
        assert!(
            parse(vec![ec(&[&[1u8][..], &y].concat(), &y)])
                .set
                .is_empty()
        );
        assert!(parse(vec![ec(&[], &y)]).set.is_empty());
        assert!(parse(vec![ec(&x, &[])]).set.is_empty());
    }

    #[test]
    fn the_rsa_minimum_is_2048_bits_not_256_octets() {
        let rsa = |octets: usize, top: u8| {
            let mut n = vec![0xC3u8; octets];
            n[0] = top;
            json!({"kty": "RSA", "kid": "k", "n": URL_SAFE_NO_PAD.encode(n), "e": "AQAB"})
        };
        // 256 octets whose first is 0x01 are 2041 bits: admitted by a count of octets, and refused by aws-lc when a
        // token is verified. The same for 0x7F (2047 bits).
        assert!(parse(vec![rsa(256, 0x01)]).set.is_empty());
        assert!(parse(vec![rsa(256, 0x7F)]).set.is_empty());
        // Exactly 2048 bits, and more.
        assert_eq!(parse(vec![rsa(256, 0x80)]).set.keys.len(), 1);
        assert_eq!(parse(vec![rsa(256, 0xFF)]).set.keys.len(), 1);
        assert_eq!(parse(vec![rsa(257, 0x01)]).set.keys.len(), 1); // 2049 bits
        // A sign byte in front changes neither the count nor the answer.
        let signed = |n: &Value| {
            let octets = URL_SAFE_NO_PAD.decode(n["n"].as_str().unwrap()).unwrap();
            let mut jwk = n.clone();
            jwk["n"] = json!(URL_SAFE_NO_PAD.encode([&[0u8][..], &octets].concat()));
            jwk
        };
        assert!(parse(vec![signed(&rsa(256, 0x01))]).set.is_empty());
        assert_eq!(parse(vec![signed(&rsa(256, 0x80))]).set.keys.len(), 1);
    }

    #[test]
    fn a_p256_coordinate_of_zeros_is_a_value_and_only_an_empty_one_is_refused() {
        let enc = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
        let zeros = [0u8; 32];
        assert_eq!(p256_coordinate(&enc(&zeros)), Some(enc(&zeros)));
        assert_eq!(p256_coordinate(&enc(&[0u8])), Some(enc(&zeros)));
        assert_eq!(p256_coordinate(""), None);
        // x = 0 is a valid abscissa of P-256 (b is a square modulo p): this is y for it, so the point is on the curve.
        let y: [u8; 32] = [
            0x66, 0x48, 0x5c, 0x78, 0x0e, 0x2f, 0x83, 0xd7, 0x24, 0x33, 0xbd, 0x5d, 0x84, 0xa0,
            0x6b, 0xb6, 0x54, 0x1c, 0x2a, 0xf3, 0x1d, 0xae, 0x87, 0x17, 0x28, 0xbf, 0x85, 0x6a,
            0x17, 0x4f, 0x93, 0xf4,
        ];
        let ec = json!({"kty": "EC", "crv": "P-256", "kid": "k", "x": enc(&zeros), "y": enc(&y)});
        assert_eq!(parse(vec![ec]).set.keys.len(), 1);
    }

    #[test]
    fn a_member_written_with_base64_padding_is_read() {
        let padded = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE.encode(bytes);
        assert_eq!(padded(&[9u8; 32]).matches('=').count(), 1);
        let ed = json!({"kty": "OKP", "crv": "Ed25519", "kid": "k", "x": padded(&[9u8; 32])});
        let ec = json!({"kty": "EC", "crv": "P-256", "kid": "k", "x": padded(&[7u8; 32]), "y": padded(&[7u8; 32])});
        let rsa =
            json!({"kty": "RSA", "kid": "k", "n": padded(&[0xC3u8; 256]), "e": padded(&[1, 0, 1])});
        assert_eq!(parse(vec![ed, ec, rsa]).set.keys.len(), 3);
    }

    #[test]
    fn a_document_that_is_not_a_jwks_is_nothing() {
        assert!(KeySet::parse(&json!({"issuer": "x"})).is_none());
        assert!(KeySet::parse(&json!({"keys": "no"})).is_none());
        assert!(KeySet::parse(&json!([])).is_none());
        assert!(KeySet::parse(&json!({"keys": []})).unwrap().set.is_empty());
    }

    /// The `kid` of each key that may verify, in the order of the set.
    fn kids(set: &KeySet, alg: Algorithm, kid: Option<&str>) -> Vec<Option<String>> {
        set.select(alg, kid)
            .into_iter()
            .map(|key| key.kid.clone())
            .collect()
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
        let kids = |alg, kid| kids(&set, alg, Some(kid));
        assert_eq!(kids(Algorithm::RS256, "a"), [Some("a".to_owned())]);
        assert_eq!(kids(Algorithm::RS384, "b"), [Some("b".to_owned())]);
        // The right kid, the wrong kind of key for the algorithm: no key.
        assert!(kids(Algorithm::ES256, "a").is_empty());
        assert!(kids(Algorithm::RS256, "c").is_empty());
        assert!(kids(Algorithm::EdDSA, "c").is_empty());
        assert!(kids(Algorithm::RS256, "nope").is_empty());
        // An algorithm that is not allowed has no kind, so no key.
        assert!(kids(Algorithm::HS256, "a").is_empty());
        assert!(kids(Algorithm::PS256, "a").is_empty());
    }

    #[test]
    fn every_key_of_a_shared_kid_is_a_candidate() {
        let set = parse(vec![
            rsa("a", json!({})),
            rsa("a", json!({"alg": "RS384"})),
            rsa("b", json!({})),
            ec("a", "P-256"),
        ])
        .set;
        // Two keys of the kind named "a" fit RS384 (the second declares it), and only the first RS256.
        assert_eq!(set.select(Algorithm::RS384, Some("a")).len(), 2);
        assert_eq!(set.select(Algorithm::RS256, Some("a")).len(), 1);
        assert_eq!(set.select(Algorithm::ES256, Some("a")).len(), 1);
        // Without a kid it is the one key of the kind, and two of the kind are none.
        assert!(set.select(Algorithm::RS256, None).is_empty());
        assert_eq!(set.select(Algorithm::ES256, None).len(), 1);
    }

    #[test]
    fn a_token_without_a_kid_takes_the_only_key_of_its_kind() {
        let one = parse(vec![rsa("a", json!({})), ec("c", "P-256")]).set;
        assert_eq!(one.select(Algorithm::RS256, None).len(), 1);
        assert_eq!(one.select(Algorithm::ES256, None).len(), 1);
        assert!(one.select(Algorithm::EdDSA, None).is_empty());
        let two = parse(vec![rsa("a", json!({})), rsa("b", json!({}))]).set;
        assert!(two.select(Algorithm::RS256, None).is_empty());
    }

    #[test]
    fn a_key_that_declares_an_algorithm_verifies_only_that_one() {
        let set = parse(vec![rsa("a", json!({"alg": "RS256"}))]).set;
        assert_eq!(set.select(Algorithm::RS256, Some("a")).len(), 1);
        assert!(set.select(Algorithm::RS384, Some("a")).is_empty());
        // A declared algorithm of another kind makes the key unusable.
        assert!(
            parse(vec![rsa("a", json!({"alg": "ES256"}))])
                .set
                .is_empty()
        );
    }
}
