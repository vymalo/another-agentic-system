//! The orchestrator's signing key: an Ed25519 key pair and the `kid` the tool server finds its public half by.

use std::fmt;

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::cbor;

/// The length of a `kid` on the wire: 8 bytes.
pub const KID_LEN: usize = 8;

const PEM_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PEM_END: &str = "-----END PRIVATE KEY-----";

/// Why a key could not be read. None of the variants holds a byte of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeyError {
    /// Not one PKCS#8 `PRIVATE KEY` PEM block (what `openssl genpkey -algorithm ed25519` writes).
    #[error("the signing key is not a PKCS#8 PEM block (BEGIN PRIVATE KEY)")]
    NotPem,
    /// The block's body is not base64.
    #[error("the signing key's PEM body is not base64")]
    NotBase64,
    /// The bytes are not an Ed25519 private key.
    #[error("the signing key is not an Ed25519 private key")]
    NotEd25519,
}

/// An Ed25519 private key and the `kid` of its public half: the first 8 bytes of the
/// [RFC 9679](https://www.rfc-editor.org/rfc/rfc9679) thumbprint (SHA-256 of the deterministic CBOR of the COSE key
/// `{1: 1, -1: 6, -2: x}`). `Debug` shows the `kid` and nothing else.
pub struct SigningKey {
    pair: Ed25519KeyPair,
    public: [u8; 32],
    kid: [u8; KID_LEN],
}

impl SigningKey {
    /// The key of the RFC 8032 secret `seed` (32 bytes).
    ///
    /// # Errors
    /// [`KeyError::NotEd25519`] when the library refuses the seed.
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self, KeyError> {
        let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| KeyError::NotEd25519)?;
        Self::of(pair)
    }

    /// The key of a PKCS#8 PEM block, version 1 (a bare seed) or 2 (with the public key), as `openssl genpkey -algorithm
    /// ed25519` and most tools write. Text around the block is refused: the configuration holds the key and nothing else.
    ///
    /// # Errors
    /// [`KeyError`], which says which step failed and never shows the text.
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self, KeyError> {
        let pem = pem.trim();
        let body = pem
            .strip_prefix(PEM_BEGIN)
            .and_then(|rest| rest.strip_suffix(PEM_END))
            .ok_or(KeyError::NotPem)?;
        let compact: Zeroizing<String> = Zeroizing::new(body.split_whitespace().collect());
        let der = Zeroizing::new(
            STANDARD
                .decode(compact.as_bytes())
                .map_err(|_| KeyError::NotBase64)?,
        );
        let pair =
            Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|_| KeyError::NotEd25519)?;
        Self::of(pair)
    }

    fn of(pair: Ed25519KeyPair) -> Result<Self, KeyError> {
        let public: [u8; 32] = pair
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| KeyError::NotEd25519)?;
        let kid = kid_of(&public);
        Ok(SigningKey { pair, public, kid })
    }

    /// The `kid` of the public half.
    pub fn kid(&self) -> [u8; KID_LEN] {
        self.kid
    }

    /// The public key, 32 bytes: what a tool server pins.
    pub fn public_key(&self) -> [u8; 32] {
        self.public
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.pair.sign(message).as_ref().to_vec()
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigningKey")
            .field("kid", &hex(&self.kid))
            .finish_non_exhaustive()
    }
}

/// The `kid` of an Ed25519 public key.
pub fn kid_of(public: &[u8; 32]) -> [u8; KID_LEN] {
    let mut cose_key = Vec::with_capacity(41);
    cbor::head(&mut cose_key, cbor::MAJOR_MAP, 3);
    // 1 (kty): 1 (OKP), -1 (crv): 6 (Ed25519), -2 (x): the key
    cose_key.extend_from_slice(&[0x01, 0x01, 0x20, 0x06, 0x21]);
    cbor::bstr(&mut cose_key, public);
    let thumbprint = Sha256::digest(&cose_key);
    let mut kid = [0_u8; KID_LEN];
    kid.copy_from_slice(&thumbprint[..KID_LEN]);
    kid
}

/// Lower-case hexadecimal.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}
