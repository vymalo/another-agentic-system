//! A [`PromptSigner`] for tests: an Ed25519 key and the certificate chain of its leaf, read from PEM files, never
//! renewed ([ADR 0087](../../../docs/decisions/0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md),
//! the note of 2026-11-25).
//!
//! It writes the statement as every signer must (`orch-prompt-statement`'s [`jws`](orch_prompt_statement::jws), the
//! `x5c` of the chain it was given), so the verifier's tests and the orchestrator's can run with a self-made chain. **It is
//! not for a deployment**: its key sits in a file and its certificate is never renewed or revoked; a deployment's signer
//! is certified by a PKI and renews itself.
//!
//! It checks what it can without a clock: the key is Ed25519, the chain holds at least the leaf, and the leaf certifies
//! this key. It does not judge the chain (that is the verifier's).

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use orch_ports::{AttestedPayload, PromptSigner, SignedStatement, SignerError, SignerState};
use orch_prompt_statement::jws;
use zeroize::Zeroizing;

mod der;

/// Why a key or a chain could not be read. None of the variants holds a byte of the key, or a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StaticSignerError {
    /// The key file or the chain file could not be read.
    #[error("the {0} file could not be read")]
    Unreadable(&'static str),
    /// Not exactly one `PRIVATE KEY` PEM block (text outside the block is allowed).
    #[error("the key is not one PKCS#8 PEM block (BEGIN PRIVATE KEY)")]
    NotOneKey,
    /// A block's body is not base64.
    #[error("a PEM block's body is not base64")]
    NotBase64,
    /// The key is not an Ed25519 private key.
    #[error("the key is not an Ed25519 private key")]
    NotEd25519,
    /// The chain holds no `CERTIFICATE` block.
    #[error("the chain holds no certificate")]
    NoCertificate,
    /// The first certificate is not a certificate with an Ed25519 key.
    #[error("the leaf is not a certificate of an Ed25519 key")]
    LeafNotEd25519,
    /// The leaf certifies another key than the one given.
    #[error("the leaf certifies another key")]
    KeyDoesNotMatchLeaf,
}

/// The static signer. `Debug` shows the leaf's serial and nothing of the key.
pub struct StaticPromptSigner {
    pair: Ed25519KeyPair,
    header: Vec<u8>,
    serial: String,
    unavailable: AtomicBool,
    signatures: AtomicU64,
}

impl StaticPromptSigner {
    /// The signer of a PKCS#8 Ed25519 key (`-----BEGIN PRIVATE KEY-----`) and a chain of `CERTIFICATE` blocks, the leaf
    /// first and the root left out. Text outside the blocks is ignored, so a file may say `TEST ONLY` above them.
    ///
    /// # Errors
    /// [`StaticSignerError`], which says what is wrong and never shows the text.
    pub fn from_pem(key_pem: &str, chain_pem: &str) -> Result<Self, StaticSignerError> {
        let keys = pem_blocks(key_pem, "PRIVATE KEY")?;
        let [der] = keys.as_slice() else {
            return Err(StaticSignerError::NotOneKey);
        };
        let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map_err(|_| StaticSignerError::NotEd25519)?;
        let chain = pem_blocks(chain_pem, "CERTIFICATE")?;
        let leaf = chain.first().ok_or(StaticSignerError::NoCertificate)?;
        let certified = der::ed25519_key(leaf).ok_or(StaticSignerError::LeafNotEd25519)?;
        if pair.public_key().as_ref() != certified {
            return Err(StaticSignerError::KeyDoesNotMatchLeaf);
        }
        let serial = der::serial_hex(leaf).ok_or(StaticSignerError::LeafNotEd25519)?;
        let chain: Vec<&[u8]> = chain.iter().map(|c| c.as_slice()).collect();
        Ok(StaticPromptSigner {
            pair,
            header: jws::protected_header(&chain),
            serial,
            unavailable: AtomicBool::new(false),
            signatures: AtomicU64::new(0),
        })
    }

    /// [`from_pem`](Self::from_pem) of two files.
    ///
    /// # Errors
    /// [`StaticSignerError::Unreadable`], or what [`from_pem`](Self::from_pem) refuses.
    pub fn from_files(key: &Path, chain: &Path) -> Result<Self, StaticSignerError> {
        let key = Zeroizing::new(
            std::fs::read_to_string(key).map_err(|_| StaticSignerError::Unreadable("key"))?,
        );
        let chain =
            std::fs::read_to_string(chain).map_err(|_| StaticSignerError::Unreadable("chain"))?;
        Self::from_pem(&key, &chain)
    }

    /// The leaf's serial number, lower-case hexadecimal without leading zeros: what a log line may name.
    pub fn leaf_serial(&self) -> &str {
        &self.serial
    }

    /// Makes the signer [`SignerState::Unavailable`] (`true`) or [`SignerState::Ready`] again (`false`): the stand-in,
    /// for tests, of a certificate that expired and was not renewed.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    /// How many statements it has signed: for a test that asserts nothing was.
    pub fn signatures(&self) -> u64 {
        self.signatures.load(Ordering::SeqCst)
    }
}

impl PromptSigner for StaticPromptSigner {
    async fn sign(&self, payload: &AttestedPayload) -> Result<SignedStatement, SignerError> {
        if self.state() == SignerState::Unavailable {
            return Err(SignerError::Unavailable {
                detail: "the static signer was switched off",
            });
        }
        let input = jws::signing_input(&self.header, payload);
        let signature = self.pair.sign(input.as_bytes());
        self.signatures.fetch_add(1, Ordering::SeqCst);
        Ok(SignedStatement::new(jws::compact(
            &input,
            signature.as_ref(),
        )))
    }

    fn state(&self) -> SignerState {
        if self.unavailable.load(Ordering::SeqCst) {
            SignerState::Unavailable
        } else {
            SignerState::Ready
        }
    }
}

impl fmt::Debug for StaticPromptSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticPromptSigner")
            .field("leaf_serial", &self.serial)
            .finish_non_exhaustive()
    }
}

/// The DER of each `-----BEGIN <label>-----` block, in order; lines outside the blocks are skipped. The base64 text is
/// wiped when it is dropped (a key's block is the key).
fn pem_blocks(pem: &str, label: &str) -> Result<Vec<Zeroizing<Vec<u8>>>, StaticSignerError> {
    let (begin, end) = (
        format!("-----BEGIN {label}-----"),
        format!("-----END {label}-----"),
    );
    let mut blocks = Vec::new();
    let mut body: Option<Zeroizing<String>> = None;
    for line in pem.lines().map(str::trim) {
        if line == begin {
            body = Some(Zeroizing::new(String::new()));
        } else if line == end {
            let text = body.take().ok_or(StaticSignerError::NotBase64)?;
            let der = STANDARD
                .decode(text.as_bytes())
                .map_err(|_| StaticSignerError::NotBase64)?;
            blocks.push(Zeroizing::new(der));
        } else if let Some(text) = body.as_mut() {
            text.push_str(line);
        }
    }
    if body.is_some() {
        // a block that never ends
        return Err(StaticSignerError::NotBase64);
    }
    Ok(blocks)
}
