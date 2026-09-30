//! HMAC-SHA-256 signatures of the two webhook schemes (ADR 0017).
//!
//! Both schemes put `sha256=` and the lowercase hex of an HMAC-SHA-256 in a header. They differ in
//! what is signed: GitHub signs the raw body; the generic scheme signs `"<timestamp>.<body>"`, the
//! timestamp being the exact text of its header, so a captured request cannot be replayed with a
//! fresher timestamp.
//!
//! A signature is checked with [`Mac::verify_slice`], which compares the tag in constant time
//! (`subtle`'s `ct_eq`; *verified 2026-09-30*, `digest` 0.10.7 `src/mac.rs`, `verify_slice`), and
//! against every secret, without stopping at the first hit, so a rotation does not show in the
//! time a check takes.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::Secrets;

type HmacSha256 = Hmac<Sha256>;

/// The prefix of a signature header's value.
const PREFIX: &str = "sha256=";
/// Bytes of an HMAC-SHA-256 tag.
const TAG_LEN: usize = 32;

/// The HMAC of `parts`, one after the other, under `secret`. `None` cannot happen (HMAC takes a
/// key of any length) and is treated as "no signature".
fn tag(secret: &[u8], parts: &[&[u8]]) -> Option<[u8; TAG_LEN]> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).ok()?;
    for part in parts {
        mac.update(part);
    }
    Some(mac.finalize().into_bytes().into())
}

/// `sha256=` and the lowercase hex of the HMAC-SHA-256 of `parts` under `secret`: what a sender
/// puts in its signature header. `None` cannot happen (see above).
pub(crate) fn sign(secret: &str, parts: &[&[u8]]) -> Option<String> {
    let tag = tag(secret.as_bytes(), parts)?;
    let mut out = String::with_capacity(PREFIX.len() + 2 * TAG_LEN);
    out.push_str(PREFIX);
    for byte in tag {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Some(out)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The generic scheme's signature: HMAC-SHA-256 of `"<timestamp>.<body>"`.
pub fn sign_generic(secret: &str, timestamp: &str, body: &[u8]) -> Option<String> {
    sign(secret, &[timestamp.as_bytes(), b".", body])
}

/// The 32 bytes a `sha256=<64 hex digits>` header value stands for. Upper-case digits are read
/// as well; anything else (no prefix, wrong length, a non-hex digit) is `None`.
fn decode(header: &str) -> Option<[u8; TAG_LEN]> {
    let hex = header.strip_prefix(PREFIX)?.as_bytes();
    if hex.len() != 2 * TAG_LEN {
        return None;
    }
    let nibble = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mut out = [0_u8; TAG_LEN];
    for (byte, pair) in out.iter_mut().zip(hex.chunks_exact(2)) {
        *byte = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(out)
}

/// Whether `header` is the signature of `parts` under any of `secrets`.
pub(crate) fn verify(secrets: &Secrets, header: &str, parts: &[&[u8]]) -> bool {
    let Some(claimed) = decode(header) else {
        return false;
    };
    // No early exit: every secret is tried, whichever matches.
    secrets.iter().fold(false, |ok, secret| {
        let good = <HmacSha256 as Mac>::new_from_slice(secret).is_ok_and(|mut mac| {
            for part in parts {
                mac.update(part);
            }
            mac.verify_slice(&claimed).is_ok()
        });
        ok | good
    })
}

/// GitHub's signature: HMAC-SHA-256 of the raw body.
pub fn sign_github(secret: &str, body: &[u8]) -> Option<String> {
    sign(secret, &[body])
}

/// Whether `header` is GitHub's signature of `body` under any of `secrets`.
pub(crate) fn verify_github(secrets: &Secrets, body: &[u8], header: &str) -> bool {
    verify(secrets, header, &[body])
}

/// Whether `header` is the generic signature of `body` at `timestamp` under any of `secrets`.
pub(crate) fn verify_generic(
    secrets: &Secrets,
    timestamp: &str,
    body: &[u8],
    header: &str,
) -> bool {
    verify(secrets, header, &[timestamp.as_bytes(), b".", body])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const S3: &str = "s3cret-s3cret-s3cret-s3cret-s3cret";
    const NEW: &str = "new-new-new-new-new-new-new-new-new";
    const OLD: &str = "old-old-old-old-old-old-old-old-old";
    const STRANGER: &str = "other-other-other-other-other-other";

    fn secrets(raw: &str) -> Secrets {
        Secrets::parse(raw).unwrap()
    }

    /// GitHub's own example secret is shorter than a secret this crate accepts in service; the
    /// primitives take any key.
    fn secrets_unchecked(raw: &str) -> Secrets {
        Secrets::unchecked(raw)
    }

    /// RFC 4231, test case 2 (HMAC-SHA-256, key "Jefe"): a vector the implementation, not this
    /// crate, is answerable for.
    #[test]
    fn rfc_4231_test_case_2() {
        assert_eq!(
            sign("Jefe", &[b"what do ya want ", b"for nothing?"]).unwrap(),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// The vector of `docs/api/webhooks.md`, computed there with OpenSSL.
    #[test]
    fn the_documented_vector() {
        const BODY: &str = r#"{"version":1,"repository":"https://github.com/acme/widgets","sha":"0123456789abcdef0123456789abcdef01234567","branch":"agent/fix-flaky-test","name":"ci/build","conclusion":"success","url":"https://ci.example.com/runs/42","summary":"212 tests passed"}"#;
        assert_eq!(
            sign_generic(
                "dev-webhook-secret-0123456789abcdef0123",
                "1790800000",
                BODY.as_bytes()
            )
            .unwrap(),
            "sha256=e7ff72c4411e69debb1f634a339e7c884641e4ada663a42369deead91e617944"
        );
    }

    /// The example of GitHub's "Validating webhook deliveries" (*verified 2026-09-30*,
    /// <https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>), and
    /// the vector of `docs/api/webhooks.md` for the same body as the generic one.
    #[test]
    fn github_signs_the_raw_body() {
        assert_eq!(
            sign_github("It's a Secret to Everybody", b"Hello, World!").unwrap(),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        const BODY: &str = r#"{"version":1,"repository":"https://github.com/acme/widgets","sha":"0123456789abcdef0123456789abcdef01234567","branch":"agent/fix-flaky-test","name":"ci/build","conclusion":"success","url":"https://ci.example.com/runs/42","summary":"212 tests passed"}"#;
        assert_eq!(
            sign_github("dev-webhook-secret-0123456789abcdef0123", BODY.as_bytes()).unwrap(),
            "sha256=3f7810292c8978124166e4004b825bbb80dbd5b64b72ddecd8099f832ba345d0"
        );
        let s = secrets_unchecked("It's a Secret to Everybody");
        let good = sign_github("It's a Secret to Everybody", b"Hello, World!").unwrap();
        assert!(verify_github(&s, b"Hello, World!", &good));
        assert!(!verify_github(&s, b"Hello, World?", &good));
        // The two schemes do not stand for each other.
        assert!(!verify_generic(&s, "1", b"Hello, World!", &good));
        let generic = sign_generic("It's a Secret to Everybody", "1", b"x").unwrap();
        assert!(!verify_github(&s, b"x", &generic));
    }

    #[test]
    fn the_timestamp_and_every_byte_of_the_body_are_signed() {
        let s = secrets(S3);
        let sig = sign_generic(S3, "1790800000", b"{}").unwrap();
        assert!(verify_generic(&s, "1790800000", b"{}", &sig));
        assert!(!verify_generic(&s, "1790800001", b"{}", &sig));
        assert!(
            !verify_generic(&s, "01790800000", b"{}", &sig),
            "the timestamp is text"
        );
        assert!(!verify_generic(&s, "1790800000", b"{} ", &sig));
        assert!(!verify_generic(
            &s,
            "1790800000",
            b"{}",
            &sig.replace("sha256=", "sha256=0")
        ));
    }

    #[test]
    fn either_of_two_secrets_verifies_and_a_third_does_not() {
        let both = secrets(&format!("{NEW},{OLD}"));
        for signer in [NEW, OLD] {
            let sig = sign_generic(signer, "1", b"body").unwrap();
            assert!(verify_generic(&both, "1", b"body", &sig), "{signer}");
        }
        let stranger = sign_generic(STRANGER, "1", b"body").unwrap();
        assert!(!verify_generic(&both, "1", b"body", &stranger));
    }

    #[test]
    fn a_malformed_header_never_verifies() {
        let s = secrets(S3);
        let good = sign_generic(S3, "1", b"x").unwrap();
        let hex = good.strip_prefix(PREFIX).unwrap();
        let upper = format!("{PREFIX}{}", hex.to_uppercase());
        assert!(
            verify_generic(&s, "1", b"x", &upper),
            "hex digits are case-insensitive"
        );
        for bad in [
            String::new(),
            "sha256=".to_owned(),
            hex.to_owned(),          // no prefix
            format!("SHA256={hex}"), // the prefix is exact
            format!("sha1={hex}"),
            format!("{PREFIX}{}", &hex[..62]),  // short
            format!("{PREFIX}{hex}00"),         // long
            format!("{PREFIX}{}g", &hex[..63]), // not hex
            format!("{PREFIX} {}", &hex[1..]),  // a blank
        ] {
            assert!(!verify_generic(&s, "1", b"x", &bad), "{bad:?}");
        }
    }
}
