//! The envelope: what a secret is at rest, and how it is sealed and opened.
//!
//! `{v, kid, nonce, ciphertext + tag}` with AES-256-GCM (RustCrypto's `aes-gcm`): `v` is the format
//! (1), `kid` the id of the key that sealed it, `nonce` 96 random bits drawn from the operating
//! system for every seal, and the ciphertext ends with the 128-bit tag. The **associated data**,
//! which the tag authenticates and which is not stored, is built from where the secret lives:
//!
//! ```text
//! aad = v (1 byte) ‖ len(owner) ‖ owner ‖ len(server) ‖ server ‖ version ‖ len(kid) ‖ kid
//! ```
//!
//! where every `len` is eight bytes, big-endian, and `version` is eight bytes, big-endian. A length
//! in front of each text makes the encoding unambiguous (owner `ab` and server `c…` can never be
//! read as owner `a` and server `bc…`), and binds the format as well as the place. So a row moved to
//! another owner, another server, or another version (or whose kid or format was edited) does not
//! open.

use orch_ports::SecretMaterial;
use zeroize::Zeroizing;

use crate::keys::Keyring;

/// The format of the envelope this build writes and reads.
pub(crate) const FORMAT: i16 = 1;

/// Bytes of a nonce: 96 bits, what GCM is specified for.
pub(crate) const NONCE_LEN: usize = 12;

/// Bytes of the tag at the end of the ciphertext.
pub(crate) const TAG_LEN: usize = 16;

/// A sealed secret as it is kept: the four columns of the table.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Envelope {
    pub(crate) v: i16,
    pub(crate) kid: String,
    pub(crate) nonce: [u8; NONCE_LEN],
    /// The ciphertext followed by the tag.
    pub(crate) ciphertext: Vec<u8>,
}

impl std::fmt::Debug for Envelope {
    /// The format and the key id; the rest is bytes that mean nothing to a reader of a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Envelope(v{}, kid {}, {} bytes)",
            self.v,
            self.kid,
            self.ciphertext.len()
        )
    }
}

/// Why an envelope did not open. The texts are fixed: nothing of the bytes is in them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenError {
    /// Sealed under a key id the ring does not hold.
    KeyUnknown,
    /// Not an envelope this build wrote, or not the one that belongs to this owner, server and
    /// version.
    Corrupt(&'static str),
}

/// Why a seal did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SealError {
    /// The operating system gave no random bytes.
    NoRandomness,
    /// The cipher refused (a plaintext beyond what GCM allows; unreachable at 16 KiB).
    Cipher,
}

fn push_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&u64::try_from(text.len()).unwrap_or(u64::MAX).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

/// The associated data of a seal.
pub(crate) fn aad(format: i16, owner: &str, server: &str, version: u64, kid: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 8 * 4 + owner.len() + server.len() + kid.len());
    out.push(u8::try_from(format).unwrap_or(u8::MAX));
    push_text(&mut out, owner);
    push_text(&mut out, server);
    out.extend_from_slice(&version.to_be_bytes());
    push_text(&mut out, kid);
    out
}

impl Keyring {
    /// Seals `plaintext` for `(owner, server)` at `version`, under the current key, with a nonce
    /// of its own.
    pub(crate) fn seal(
        &self,
        owner: &str,
        server: &str,
        version: u64,
        plaintext: &[u8],
    ) -> Result<Envelope, SealError> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| SealError::NoRandomness)?;
        self.seal_with_nonce(nonce, owner, server, version, plaintext)
    }

    /// [`seal`](Self::seal) with the nonce given, for the known-answer test.
    pub(crate) fn seal_with_nonce(
        &self,
        nonce: [u8; NONCE_LEN],
        owner: &str,
        server: &str,
        version: u64,
        plaintext: &[u8],
    ) -> Result<Envelope, SealError> {
        use aes_gcm::AeadInOut as _;
        let key = self.sealing_key();
        let aad = aad(FORMAT, owner, server, version, key.id.as_str());
        // The room for the tag is made first, so that the buffer never moves and leaves a copy of
        // the plaintext behind; it holds the plaintext until it is the ciphertext, so it is wiped.
        let mut buffer = Zeroizing::new(Vec::with_capacity(plaintext.len() + TAG_LEN));
        buffer.extend_from_slice(plaintext);
        key.cipher
            .encrypt_in_place(&nonce.into(), &aad, &mut *buffer)
            .map_err(|_| SealError::Cipher)?;
        Ok(Envelope {
            v: FORMAT,
            kid: key.id.as_str().to_owned(),
            nonce,
            ciphertext: std::mem::take(&mut *buffer),
        })
    }

    /// Opens `envelope` as the secret of `(owner, server)` at `version`.
    pub(crate) fn open(
        &self,
        owner: &str,
        server: &str,
        version: u64,
        envelope: &Envelope,
    ) -> Result<SecretMaterial, OpenError> {
        use aes_gcm::AeadInOut as _;
        if envelope.v != FORMAT {
            return Err(OpenError::Corrupt(
                "the envelope is of a format this build does not read",
            ));
        }
        let Some(key) = self.key(&envelope.kid) else {
            return Err(OpenError::KeyUnknown);
        };
        if envelope.ciphertext.len() < TAG_LEN {
            return Err(OpenError::Corrupt(
                "the sealed secret is shorter than its tag",
            ));
        }
        let aad = aad(envelope.v, owner, server, version, &envelope.kid);
        // opened in place in a buffer that is wiped, however it ends
        let mut buffer = Zeroizing::new(envelope.ciphertext.clone());
        key.cipher
            .decrypt_in_place(&envelope.nonce.into(), &aad, &mut *buffer)
            .map_err(|_| {
                OpenError::Corrupt(
                    "the sealed secret does not open: it was altered, or it belongs to another \
                     owner, server or version",
                )
            })?;
        SecretMaterial::new(std::mem::take(&mut *buffer))
            .map_err(|_| OpenError::Corrupt("the sealed secret is larger than a secret may be"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keys::{KEY_LEN, KeyBytes, KeyId};

    const OWNER: &str = "alice@example.com";
    const SERVER: &str = "u-abcdefghij";
    const SENTINEL: &[u8] = b"sentinel-3f9a-this-must-not-be-in-the-row";

    fn id(text: &str) -> KeyId {
        KeyId::parse(text).unwrap()
    }

    fn ring(current: &str, keys: &[(&str, u8)]) -> Keyring {
        Keyring::new(
            id(current),
            keys.iter()
                .map(|(name, byte)| (id(name), KeyBytes::from_bytes([*byte; KEY_LEN]))),
        )
        .unwrap()
    }

    fn one() -> Keyring {
        ring("k1", &[("k1", 1)])
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The vectors below were made with OpenSSL 3.0.13 through Python `cryptography` 49.0.0
    /// (`AESGCM(key).encrypt(nonce, plaintext, aad)`), by an encoder of the associated data
    /// written separately from this one.
    fn kat_ring() -> Keyring {
        let key = KeyBytes::from_bytes(std::array::from_fn(|i| u8::try_from(i).unwrap()));
        Keyring::new(id("k1"), [(id("k1"), key)]).unwrap()
    }

    fn kat_nonce() -> [u8; NONCE_LEN] {
        std::array::from_fn(|i| 100 + u8::try_from(i).unwrap())
    }

    #[test]
    fn the_associated_data_is_pinned_byte_for_byte() {
        // changing this encoding makes every stored secret unreadable: it is a migration
        let expected = "01\
            0000000000000011616c696365406578616d706c652e636f6d\
            000000000000000c752d6162636465666768696a\
            0000000000000007\
            00000000000000026b31";
        assert_eq!(hex(&aad(FORMAT, OWNER, SERVER, 7, "k1")), expected);
    }

    #[test]
    fn the_associated_data_cannot_be_read_two_ways() {
        // the same letters, split between the owner and the server in two ways
        assert_ne!(aad(1, "ab", "cd", 1, "k"), aad(1, "a", "bcd", 1, "k"));
        assert_ne!(aad(1, "ab", "cd", 1, "k"), aad(1, "abc", "d", 1, "k"));
        // and between the server and the key id
        assert_ne!(aad(1, "o", "ab", 1, "c"), aad(1, "o", "a", 1, "bc"));
        // every part counts
        let base = aad(1, "o", "s", 1, "k");
        for other in [
            aad(2, "o", "s", 1, "k"),
            aad(1, "p", "s", 1, "k"),
            aad(1, "o", "t", 1, "k"),
            aad(1, "o", "s", 2, "k"),
            aad(1, "o", "s", 1, "l"),
        ] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn a_seal_is_what_openssl_makes_of_the_same_key_nonce_and_associated_data() {
        let envelope = kat_ring()
            .seal_with_nonce(kat_nonce(), OWNER, SERVER, 7, b"hello, sealed world")
            .unwrap();
        assert_eq!(envelope.v, 1);
        assert_eq!(envelope.kid, "k1");
        assert_eq!(envelope.nonce, kat_nonce());
        assert_eq!(
            hex(&envelope.ciphertext),
            "207eb20a16c576ed5b03338dbe451d9230ae62611040df099d8994f266d137f2dcabfd"
        );
        let empty = kat_ring()
            .seal_with_nonce(kat_nonce(), OWNER, SERVER, 7, b"")
            .unwrap();
        assert_eq!(hex(&empty.ciphertext), "af91c99d77f9a6e359664d47118cbb5e");
    }

    #[test]
    fn what_openssl_sealed_opens() {
        let envelope = Envelope {
            v: 1,
            kid: "k1".to_owned(),
            nonce: kat_nonce(),
            ciphertext: (0..35)
                .map(|i| {
                    let text =
                        "207eb20a16c576ed5b03338dbe451d9230ae62611040df099d8994f266d137f2dcabfd";
                    u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap()
                })
                .collect(),
        };
        let material = kat_ring().open(OWNER, SERVER, 7, &envelope).unwrap();
        assert_eq!(material.expose(), b"hello, sealed world");
    }

    #[test]
    fn material_of_every_size_comes_back_whole() {
        let ring = one();
        for len in [0, 1, 15, 16, 17, 255, 4096, 16 * 1024] {
            let plaintext: Vec<u8> = (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect();
            let envelope = ring.seal(OWNER, SERVER, 9, &plaintext).unwrap();
            assert_eq!(envelope.ciphertext.len(), len + TAG_LEN);
            let back = ring.open(OWNER, SERVER, 9, &envelope).unwrap();
            assert_eq!(back.expose(), plaintext.as_slice(), "{len} bytes");
        }
    }

    #[test]
    fn two_seals_of_the_same_material_differ() {
        let ring = one();
        let a = ring.seal(OWNER, SERVER, 1, b"same").unwrap();
        let b = ring.seal(OWNER, SERVER, 1, b"same").unwrap();
        assert_ne!(a.nonce, b.nonce, "a fresh nonce for every seal");
        assert_ne!(a.ciphertext, b.ciphertext);
        // a few hundred nonces from the system are all different (a constant would show at once)
        let nonces: std::collections::HashSet<_> = (0..300)
            .map(|_| ring.seal(OWNER, SERVER, 1, b"x").unwrap().nonce)
            .collect();
        assert_eq!(nonces.len(), 300);
    }

    #[test]
    fn the_plaintext_is_not_in_the_envelope() {
        let envelope = one().seal(OWNER, SERVER, 1, SENTINEL).unwrap();
        let all = [
            envelope.ciphertext.clone(),
            envelope.nonce.to_vec(),
            envelope.kid.clone().into_bytes(),
        ]
        .concat();
        assert!(!all.windows(SENTINEL.len()).any(|w| w == SENTINEL));
        assert!(!format!("{envelope:?}").contains("sentinel"));
    }

    #[test]
    fn a_row_moved_to_another_owner_server_or_version_does_not_open() {
        let ring = one();
        let envelope = ring.seal(OWNER, SERVER, 7, b"secret").unwrap();
        assert!(ring.open(OWNER, SERVER, 7, &envelope).is_ok());
        for (owner, server, version) in [
            ("bob@example.com", SERVER, 7),
            (OWNER, "u-abcdefghik", 7),
            (OWNER, "u-abcdefghij#flow", 7),
            (OWNER, SERVER, 8),
            (OWNER, SERVER, 6),
            // the same letters, split another way
            ("alice@example.comu-", "abcdefghij", 7),
        ] {
            assert!(
                matches!(
                    ring.open(owner, server, version, &envelope),
                    Err(OpenError::Corrupt(_))
                ),
                "{owner} {server} {version}"
            );
        }
    }

    #[test]
    fn a_changed_key_id_or_format_does_not_open() {
        // k2 holds the very bytes of k1: only the authenticated key id tells the two seals apart
        let ring = ring("k1", &[("k1", 1), ("k2", 1)]);
        let envelope = ring.seal(OWNER, SERVER, 7, b"secret").unwrap();
        let moved = Envelope {
            kid: "k2".to_owned(),
            ..envelope.clone()
        };
        assert!(matches!(
            ring.open(OWNER, SERVER, 7, &moved),
            Err(OpenError::Corrupt(_))
        ));
        let newer = Envelope { v: 2, ..envelope };
        assert!(matches!(
            ring.open(OWNER, SERVER, 7, &newer),
            Err(OpenError::Corrupt(_))
        ));
    }

    #[test]
    fn any_changed_byte_of_the_nonce_or_the_ciphertext_or_the_tag_does_not_open() {
        let ring = one();
        let envelope = ring
            .seal(OWNER, SERVER, 7, b"a secret of some length")
            .unwrap();
        for at in 0..envelope.ciphertext.len() {
            let mut bad = envelope.clone();
            bad.ciphertext[at] ^= 0x01;
            assert!(
                matches!(
                    ring.open(OWNER, SERVER, 7, &bad),
                    Err(OpenError::Corrupt(_))
                ),
                "byte {at}"
            );
        }
        for at in 0..NONCE_LEN {
            let mut bad = envelope.clone();
            bad.nonce[at] ^= 0x80;
            assert!(
                matches!(
                    ring.open(OWNER, SERVER, 7, &bad),
                    Err(OpenError::Corrupt(_))
                ),
                "nonce {at}"
            );
        }
        // shorter than a tag, and cut by one byte
        for keep in [0, 1, TAG_LEN - 1, envelope.ciphertext.len() - 1] {
            let mut bad = envelope.clone();
            bad.ciphertext.truncate(keep);
            assert!(
                matches!(
                    ring.open(OWNER, SERVER, 7, &bad),
                    Err(OpenError::Corrupt(_))
                ),
                "kept {keep}"
            );
        }
    }

    #[test]
    fn a_key_that_is_not_held_is_unknown_and_not_corrupt() {
        let sealed_by = ring("old", &[("old", 1)]);
        let envelope = sealed_by.seal(OWNER, SERVER, 7, b"secret").unwrap();
        let without_it = ring("new", &[("new", 2)]);
        assert_eq!(
            without_it.open(OWNER, SERVER, 7, &envelope).unwrap_err(),
            OpenError::KeyUnknown
        );
        // a held key id with other bytes is a seal that does not verify
        let other_bytes = ring("old", &[("old", 3)]);
        assert!(matches!(
            other_bytes.open(OWNER, SERVER, 7, &envelope),
            Err(OpenError::Corrupt(_))
        ));
    }

    #[test]
    fn a_ring_opens_what_any_of_its_keys_sealed_and_seals_with_the_current_one() {
        let first = ring("k1", &[("k1", 1)]);
        let old = first.seal(OWNER, SERVER, 1, b"old").unwrap();
        let both = ring("k2", &[("k1", 1), ("k2", 2)]);
        assert_eq!(both.open(OWNER, SERVER, 1, &old).unwrap().expose(), b"old");
        let new = both.seal(OWNER, SERVER, 2, b"new").unwrap();
        assert_eq!(new.kid, "k2");
        assert_eq!(both.open(OWNER, SERVER, 2, &new).unwrap().expose(), b"new");
        // the first ring cannot open what the second sealed
        assert_eq!(
            first.open(OWNER, SERVER, 2, &new).unwrap_err(),
            OpenError::KeyUnknown
        );
    }
}
