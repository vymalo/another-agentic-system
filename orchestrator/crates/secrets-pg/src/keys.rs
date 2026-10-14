//! The keys that seal the secrets: an id and 32 random bytes each, a ring of them, and the id of
//! the one that seals new secrets.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use zeroize::Zeroizing;

/// The most characters of a key id.
pub const MAX_KEY_ID_LEN: usize = 64;

/// Bytes in a key: AES-256.
pub const KEY_LEN: usize = 32;

/// What is wrong with a key or a ring of keys. No message holds a key, a part of one or the text
/// a key was read from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeyError {
    /// An id that is empty, over [`MAX_KEY_ID_LEN`] characters, or has a character other than
    /// `A-Z a-z 0-9 . _ -`.
    #[error("a key id is 1 to 64 characters of A-Z, a-z, 0-9, '.', '_' and '-'")]
    BadId,
    /// A key that is not 64 hexadecimal characters (`openssl rand -hex 32`).
    #[error("a key is 64 hexadecimal characters (32 bytes, as `openssl rand -hex 32` makes)")]
    BadKey,
    /// A key of 32 zero bytes: a placeholder nobody replaced, not a key.
    #[error("a key of all zero bytes is no key: make one with `openssl rand -hex 32`")]
    ZeroKey,
    /// Two keys with one id.
    #[error("the key id {0} is listed twice")]
    Duplicate(KeyId),
    /// No key at all.
    #[error("a keyring holds at least one key")]
    NoKeys,
    /// The current key is not one of the keys.
    #[error("the current key id {0} names no key of the ring")]
    CurrentNotHeld(KeyId),
}

/// The id of a key: a short name the operator gives it (`k1`, `2026-10`). It is stored beside every
/// secret it sealed and is part of what the seal authenticates. Not a secret.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId(String);

impl KeyId {
    /// Reads an id.
    ///
    /// # Errors
    /// [`KeyError::BadId`].
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        let ok = !text.is_empty()
            && text.len() <= MAX_KEY_ID_LEN
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if ok {
            Ok(KeyId(text.to_owned()))
        } else {
            Err(KeyError::BadId)
        }
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for KeyId {
    /// Consistent with the derived `Eq`, `Ord` and `Hash` (which are those of the text).
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyId({})", self.0)
    }
}

/// The 32 bytes of a key, wiped when dropped. `Debug` shows `<redacted>`; there is no `Display`,
/// `Clone`, `PartialEq` or `Serialize`.
pub struct KeyBytes(Zeroizing<[u8; KEY_LEN]>);

impl KeyBytes {
    /// Wraps 32 bytes. The array the caller built them in is theirs to wipe.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        KeyBytes(Zeroizing::new(bytes))
    }

    /// Reads a key written as 64 hexadecimal characters, in either case, with the white space
    /// around it cut (a file's trailing newline).
    ///
    /// # Errors
    /// [`KeyError::BadKey`], whatever is wrong with the text; it is not quoted. [`KeyError::ZeroKey`]
    /// for 64 zeros, which is what a template or an empty buffer leaves behind and no generator
    /// makes.
    pub fn from_hex(text: &str) -> Result<Self, KeyError> {
        let digits = text.trim().as_bytes();
        if digits.len() != KEY_LEN * 2 {
            return Err(KeyError::BadKey);
        }
        let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            let (high, low) = (hex_digit(pair[0]), hex_digit(pair[1]));
            match (high, low) {
                (Some(high), Some(low)) => *byte = (high << 4) | low,
                _ => return Err(KeyError::BadKey),
            }
        }
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(KeyError::ZeroKey);
        }
        Ok(KeyBytes(bytes))
    }

    pub(crate) fn expose(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for KeyBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A key as the ring holds it: its id and the cipher built from it (the key schedule is wiped when
/// the cipher is dropped; the 32 bytes themselves are not kept).
pub(crate) struct Key {
    pub(crate) id: KeyId,
    pub(crate) cipher: aes_gcm::Aes256Gcm,
}

/// The keys of a deployment: any of them opens what it sealed, and the **current** one seals every
/// new secret and every secret that is re-sealed ([`rewrap`](crate::PgSecretStore::rewrap)).
///
/// Rotation is by id: add a key, make it current, run `rewrap`, remove the old one.
pub struct Keyring {
    current: Arc<Key>,
    keys: BTreeMap<KeyId, Arc<Key>>,
}

impl Keyring {
    /// A ring of `keys`, sealing with `current`.
    ///
    /// # Errors
    /// [`KeyError::NoKeys`], [`KeyError::Duplicate`], [`KeyError::CurrentNotHeld`].
    pub fn new(
        current: KeyId,
        keys: impl IntoIterator<Item = (KeyId, KeyBytes)>,
    ) -> Result<Self, KeyError> {
        use aes_gcm::KeyInit as _;
        let mut ring = BTreeMap::new();
        for (id, bytes) in keys {
            let cipher = aes_gcm::Aes256Gcm::new(bytes.expose().into());
            if ring.contains_key(&id) {
                return Err(KeyError::Duplicate(id));
            }
            ring.insert(id.clone(), Arc::new(Key { id, cipher }));
        }
        if ring.is_empty() {
            return Err(KeyError::NoKeys);
        }
        let Some(sealing) = ring.get(&current) else {
            return Err(KeyError::CurrentNotHeld(current));
        };
        Ok(Keyring {
            current: Arc::clone(sealing),
            keys: ring,
        })
    }

    /// The id of the key that seals.
    pub fn current(&self) -> &KeyId {
        &self.current.id
    }

    /// The ids of the keys held, in order.
    pub fn ids(&self) -> impl Iterator<Item = &KeyId> {
        self.keys.keys()
    }

    /// Whether a key of this id is held.
    pub fn holds(&self, id: &str) -> bool {
        self.keys.contains_key(id)
    }

    pub(crate) fn sealing_key(&self) -> &Key {
        &self.current
    }

    pub(crate) fn key(&self, id: &str) -> Option<&Key> {
        self.keys.get(id).map(AsRef::as_ref)
    }
}

impl fmt::Debug for Keyring {
    /// The ids only.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyring")
            .field("current", &self.current.id)
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn id(text: &str) -> KeyId {
        KeyId::parse(text).unwrap()
    }

    fn key(byte: u8) -> KeyBytes {
        KeyBytes::from_bytes([byte; KEY_LEN])
    }

    #[test]
    fn a_key_is_64_hex_characters_in_either_case_with_white_space_around_cut() {
        let lower = KeyBytes::from_hex(HEX).unwrap();
        assert_eq!(lower.expose()[..4], [0, 1, 2, 3]);
        assert_eq!(lower.expose()[31], 0x1f);
        let upper = KeyBytes::from_hex(&format!("  {}\r\n", HEX.to_uppercase())).unwrap();
        assert_eq!(upper.expose(), lower.expose());
    }

    #[test]
    fn a_key_of_another_length_or_with_another_character_is_refused_without_a_word_of_it() {
        let wrong = [
            String::new(),
            HEX[..62].to_owned(),
            format!("{HEX}00"),
            format!("{}g", &HEX[..63]),
            format!("{} ", &HEX[..63]),
            "ü".repeat(32),
            // an `0x` prefix is not hex
            format!("0x{}", &HEX[..62]),
        ];
        for text in wrong {
            let error = KeyBytes::from_hex(&text).expect_err(&text);
            assert_eq!(error, KeyError::BadKey);
            let said = format!("{error} {error:?}");
            assert!(!said.contains(&text[..text.len().min(8)]) || text.is_empty());
        }
    }

    #[test]
    fn a_key_of_all_zeros_is_refused_and_a_key_with_some_is_not() {
        for text in ["0".repeat(64), format!("  {}\r\n", "0".repeat(64))] {
            let error = KeyBytes::from_hex(&text).unwrap_err();
            assert_eq!(error, KeyError::ZeroKey, "{text:?}");
            assert!(
                !format!("{error} {error:?}").contains("00000000"),
                "the text of the key is not in the error"
            );
        }
        assert!(KeyBytes::from_hex(&format!("{}1", "0".repeat(63))).is_ok());
        assert!(KeyBytes::from_hex(&format!("1{}", "0".repeat(63))).is_ok());
        // a key that is zero and short is just a bad key
        assert_eq!(
            KeyBytes::from_hex(&"0".repeat(63)).unwrap_err(),
            KeyError::BadKey
        );
    }

    #[test]
    fn the_bytes_of_a_key_are_never_printed() {
        let key = KeyBytes::from_hex(HEX).unwrap();
        assert_eq!(format!("{key:?}"), "<redacted>");
        let ring = Keyring::new(id("k1"), [(id("k1"), key)]).unwrap();
        let said = format!("{ring:?}");
        assert!(said.contains("k1"));
        assert!(
            !said.contains("0102") && !said.contains("redacted"),
            "{said}"
        );
    }

    #[test]
    fn an_id_is_one_to_sixty_four_characters_of_a_small_alphabet() {
        for good in ["k1", "2026-10", "a.b_c-D", &"x".repeat(64)] {
            assert_eq!(KeyId::parse(good).unwrap().as_str(), good);
        }
        for bad in ["", " ", "k 1", "k/1", "k\n", "clé", &"x".repeat(65)] {
            assert_eq!(KeyId::parse(bad).unwrap_err(), KeyError::BadId, "{bad:?}");
        }
    }

    #[test]
    fn a_ring_names_its_current_key_and_holds_every_key() {
        let ring = Keyring::new(id("k2"), [(id("k1"), key(1)), (id("k2"), key(2))]).unwrap();
        assert_eq!(ring.current().as_str(), "k2");
        assert_eq!(
            ring.ids().map(KeyId::as_str).collect::<Vec<_>>(),
            ["k1", "k2"]
        );
        assert!(ring.holds("k1") && ring.holds("k2") && !ring.holds("k3"));
    }

    #[test]
    fn a_ring_refuses_no_keys_a_repeated_id_and_a_current_key_it_does_not_hold() {
        assert_eq!(Keyring::new(id("k1"), []).unwrap_err(), KeyError::NoKeys);
        assert_eq!(
            Keyring::new(id("k1"), [(id("k1"), key(1)), (id("k1"), key(2))]).unwrap_err(),
            KeyError::Duplicate(id("k1"))
        );
        assert_eq!(
            Keyring::new(id("k9"), [(id("k1"), key(1))]).unwrap_err(),
            KeyError::CurrentNotHeld(id("k9"))
        );
    }
}
