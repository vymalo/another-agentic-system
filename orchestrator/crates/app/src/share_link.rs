//! The token of a share link (ADR 0040, section 2): a capability, built from the nonce on the
//! thread's row and a MAC under the deployment's `sharing.secret`.
//!
//! ```text
//! mac   = HMAC-SHA256(secret, "share/v1" || thread_id || nonce)[0..16]
//! token = base64url_nopad(nonce || mac)          # 32 bytes, 43 characters
//! ```
//!
//! `thread_id` is the 16 bytes of the thread's UUID, `nonce` the 16 random bytes. The nonce is the
//! lookup key (a unique index); the MAC is why a copy of the database alone cannot build a working
//! link. The link is **recomputed** from the row whenever the owner asks for it, so no secret token
//! is stored and the owner can always copy it again.
//!
//! [`ShareKeys`] holds the current secret, which makes tokens and verifies them, and the previous
//! one, which only verifies (a rotation: links made under either still open). A token that does not
//! parse, or whose MAC fails against both, is the same `None`: the caller answers the same 404 for
//! it as for a token nobody has.
//!
//! Pure: no I/O, no clock, no randomness (the nonce is drawn through a port).

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use orch_core::{NONCE_LEN, ShareNonce, ThreadId};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use subtle::Choice;

type HmacSha256 = Hmac<Sha256>;

/// The domain separator of the MAC.
const CONTEXT: &[u8] = b"share/v1";
/// Bytes of the MAC that are kept.
pub const MAC_LEN: usize = 16;
/// Bytes of a token before base64: the nonce and the MAC.
pub const TOKEN_BYTES: usize = NONCE_LEN + MAC_LEN;
/// Characters of a token: 32 bytes, unpadded base64url.
pub const TOKEN_CHARS: usize = 43;
/// The shortest `sharing.secret`, in bytes: what `openssl rand -hex 16` gives, and what HMAC-SHA256
/// asks for a key as large as its output (the same floor as `threadTools.secret`).
pub const MIN_SECRET_BYTES: usize = 32;

/// Why a set of keys cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ShareKeyError {
    /// A secret shorter than [`MIN_SECRET_BYTES`].
    #[error("the {which} secret is shorter than {MIN_SECRET_BYTES} bytes")]
    TooShort {
        /// `current` or `previous`.
        which: &'static str,
    },
    /// The previous secret is the current one: the rotation is a mistake.
    #[error("the previous secret is the same as the current one")]
    Same,
}

/// The secrets a link's MAC is made and checked with. `Debug` shows nothing of them.
#[derive(Clone)]
pub struct ShareKeys {
    current: SecretString,
    previous: Option<SecretString>,
}

impl fmt::Debug for ShareKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShareKeys")
            .field("previous", &self.previous.is_some())
            .finish_non_exhaustive()
    }
}

/// A token read apart, not yet checked: the nonce to look the thread up by, and the MAC to check
/// against the thread it finds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OpenedToken {
    nonce: [u8; NONCE_LEN],
    mac: [u8; MAC_LEN],
}

impl fmt::Debug for OpenedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpenedToken(..)")
    }
}

impl OpenedToken {
    /// The nonce, the key the thread is found by.
    pub const fn nonce(&self) -> &[u8; NONCE_LEN] {
        &self.nonce
    }
}

impl ShareKeys {
    /// The keys: each at least [`MIN_SECRET_BYTES`] bytes, the previous one not the current one.
    ///
    /// # Errors
    /// [`ShareKeyError`].
    pub fn new(
        current: SecretString,
        previous: Option<SecretString>,
    ) -> Result<ShareKeys, ShareKeyError> {
        if current.expose_secret().len() < MIN_SECRET_BYTES {
            return Err(ShareKeyError::TooShort { which: "current" });
        }
        if let Some(previous) = &previous {
            if previous.expose_secret().len() < MIN_SECRET_BYTES {
                return Err(ShareKeyError::TooShort { which: "previous" });
            }
            if previous.expose_secret() == current.expose_secret() {
                return Err(ShareKeyError::Same);
            }
        }
        Ok(ShareKeys { current, previous })
    }

    /// The token of `thread`'s link with this `nonce`, under the **current** secret: what the
    /// owner is shown, whatever secret the link was first made under.
    pub fn token(&self, thread: ThreadId, nonce: &ShareNonce) -> String {
        let mac = mac_of(&self.current, thread, nonce.as_bytes());
        let mut bytes = [0_u8; TOKEN_BYTES];
        bytes[..NONCE_LEN].copy_from_slice(nonce.as_bytes());
        bytes[NONCE_LEN..].copy_from_slice(&mac);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Reads `token` apart. `None` for anything that is not exactly 43 characters of the unpadded
    /// base64url alphabet: another length, padding, another alphabet and non-canonical trailing
    /// bits are all refused, so no two spellings stand for one token.
    pub fn open(token: &str) -> Option<OpenedToken> {
        if token.len() != TOKEN_CHARS {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
        let bytes: [u8; TOKEN_BYTES] = bytes.try_into().ok()?;
        let mut nonce = [0_u8; NONCE_LEN];
        let mut mac = [0_u8; MAC_LEN];
        nonce.copy_from_slice(&bytes[..NONCE_LEN]);
        mac.copy_from_slice(&bytes[NONCE_LEN..]);
        Some(OpenedToken { nonce, mac })
    }

    /// Whether `opened` is a token this deployment made for `thread`: its MAC is the one the
    /// current secret gives, or the one the previous secret gives. Both are always computed and
    /// compared in constant time, so the time says neither which secret matched nor how many bytes
    /// of the MAC were right.
    pub fn verify(&self, thread: ThreadId, opened: &OpenedToken) -> bool {
        let under = |secret: &SecretString| {
            let expected = mac_of(secret, thread, &opened.nonce);
            constant_time_eq(&expected, &opened.mac)
        };
        let current = under(&self.current);
        // With no previous secret this is a comparison that cannot succeed, but still happens.
        let previous = match &self.previous {
            Some(previous) => under(previous),
            None => Choice::from(0),
        };
        bool::from(current | previous)
    }
}

/// The MAC a link carries.
fn mac_of(secret: &SecretString, thread: ThreadId, nonce: &[u8; NONCE_LEN]) -> [u8; MAC_LEN] {
    // HMAC takes a key of any length, so this cannot fail; a key it somehow refused would make a
    // MAC no token carries, which fails closed.
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(secret.expose_secret().as_bytes()) else {
        return [0xff; MAC_LEN];
    };
    mac.update(CONTEXT);
    mac.update(thread.0.as_bytes());
    mac.update(nonce);
    let full = mac.finalize().into_bytes();
    let mut out = [0_u8; MAC_LEN];
    out.copy_from_slice(&full[..MAC_LEN]);
    out
}

/// Equality of two MACs that takes the same time whatever they hold.
fn constant_time_eq(a: &[u8; MAC_LEN], b: &[u8; MAC_LEN]) -> Choice {
    use subtle::ConstantTimeEq as _;
    a.ct_eq(b)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use uuid::Uuid;

    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";
    const PREVIOUS: &str = "fedcba9876543210fedcba9876543210";

    fn keys(current: &str, previous: Option<&str>) -> ShareKeys {
        ShareKeys::new(
            SecretString::from(current.to_owned()),
            previous.map(|p| SecretString::from(p.to_owned())),
        )
        .unwrap()
    }

    fn thread(n: u128) -> ThreadId {
        ThreadId(Uuid::from_u128(
            0x0000_0000_0000_7000_8000_0000_0000_0000 + n,
        ))
    }

    fn nonce() -> ShareNonce {
        ShareNonce::new(std::array::from_fn(|i| u8::try_from(i).unwrap()))
    }

    // Computed by an independent implementation (Python's hmac and base64): the construction of
    // the ADR, byte for byte.
    const TOKEN: &str = "AAECAwQFBgcICQoLDA0OD0SyaxPz5VtDPNnsAPfYfko";
    const TOKEN_UNDER_PREVIOUS: &str = "AAECAwQFBgcICQoLDA0OD-6Nq8LnAi060EGu9x3CnF8";

    #[test]
    fn a_token_is_the_nonce_and_the_mac_as_the_adr_builds_it() {
        let keys = keys(SECRET, None);
        let token = keys.token(thread(1), &nonce());
        assert_eq!(token, TOKEN);
        assert_eq!(token.len(), TOKEN_CHARS);
        // and a second secret gives a second token for the same thread and nonce
        let previous = self::keys(PREVIOUS, None);
        assert_eq!(previous.token(thread(1), &nonce()), TOKEN_UNDER_PREVIOUS);
    }

    #[test]
    fn a_token_made_here_opens_and_verifies_for_its_thread_only() {
        let keys = keys(SECRET, None);
        let token = keys.token(thread(1), &nonce());
        let opened = ShareKeys::open(&token).unwrap();
        assert_eq!(opened.nonce(), nonce().as_bytes());
        assert!(keys.verify(thread(1), &opened));
        // another thread's id: the nonce of one thread cannot open another
        assert!(!keys.verify(thread(2), &opened));
    }

    #[test]
    fn a_token_made_under_another_secret_does_not_verify() {
        let other = keys(PREVIOUS, None);
        let opened = ShareKeys::open(&other.token(thread(1), &nonce())).unwrap();
        assert!(!keys(SECRET, None).verify(thread(1), &opened));
    }

    #[test]
    fn the_previous_secret_still_verifies_and_the_owner_copy_carries_the_current_one() {
        let rotated = keys(SECRET, Some(PREVIOUS));
        // a link made under the previous secret still opens...
        let old = ShareKeys::open(TOKEN_UNDER_PREVIOUS).unwrap();
        assert!(rotated.verify(thread(1), &old));
        // ...a link under the current one opens too...
        assert!(rotated.verify(thread(1), &ShareKeys::open(TOKEN).unwrap()));
        // ...what the server shows the owner is made under the current secret...
        assert_eq!(rotated.token(thread(1), &nonce()), TOKEN);
        // ...and once the previous secret is dropped the old link is dead (the nonce is intact).
        let dropped = keys(SECRET, None);
        assert!(!dropped.verify(thread(1), &old));
    }

    #[test]
    fn a_tampered_mac_or_nonce_fails() {
        let keys = keys(SECRET, None);
        let token = keys.token(thread(1), &nonce());
        // every one of the 43 characters, changed, gives a token that either does not parse or
        // does not verify (the last character carries two bits that are not data)
        for at in 0..token.len() {
            let mut bytes = token.clone().into_bytes();
            bytes[at] = if bytes[at] == b'A' { b'B' } else { b'A' };
            let tampered = String::from_utf8(bytes).unwrap();
            let verified = ShareKeys::open(&tampered).is_some_and(|o| keys.verify(thread(1), &o));
            assert!(!verified, "character {at}");
        }
    }

    #[test]
    fn only_the_exact_spelling_opens() {
        for bad in [
            "",
            "A",
            &TOKEN[..42],
            &format!("{TOKEN}A"),
            &format!("{TOKEN}="),
            // the standard alphabet is not the URL one
            &TOKEN_UNDER_PREVIOUS.replace('-', "+"),
            &format!(" {}", &TOKEN[..42]),
            "é".repeat(22).as_str(),
        ] {
            assert!(
                ShareKeys::open(bad).is_none(),
                "{bad:?} is not a token of this construction"
            );
        }
        assert!(ShareKeys::open(TOKEN).is_some());
    }

    #[test]
    fn keys_are_checked() {
        let short = ShareKeys::new(SecretString::from("short".to_owned()), None);
        assert_eq!(
            short.unwrap_err(),
            ShareKeyError::TooShort { which: "current" }
        );
        let short_previous = ShareKeys::new(
            SecretString::from(SECRET.to_owned()),
            Some(SecretString::from("short".to_owned())),
        );
        assert_eq!(
            short_previous.unwrap_err(),
            ShareKeyError::TooShort { which: "previous" }
        );
        let same = ShareKeys::new(
            SecretString::from(SECRET.to_owned()),
            Some(SecretString::from(SECRET.to_owned())),
        );
        assert_eq!(same.unwrap_err(), ShareKeyError::Same);
    }

    #[test]
    fn nothing_prints_a_secret_or_a_token() {
        let keys = keys(SECRET, Some(PREVIOUS));
        let printed = format!("{keys:?} {:?}", ShareKeys::open(TOKEN).unwrap());
        assert!(!printed.contains(SECRET) && !printed.contains(PREVIOUS));
        assert!(!printed.contains(TOKEN));
    }

    #[test]
    fn the_comparison_is_over_the_whole_mac() {
        let a = [7_u8; MAC_LEN];
        let mut b = a;
        assert!(bool::from(constant_time_eq(&a, &b)));
        for i in 0..MAC_LEN {
            b[i] ^= 1;
            assert!(!bool::from(constant_time_eq(&a, &b)), "byte {i}");
            b[i] ^= 1;
        }
    }
}
