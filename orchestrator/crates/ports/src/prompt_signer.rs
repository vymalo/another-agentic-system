//! The prompt signer port: who signs the orchestrator's prompt statement on the cloud path
//! ([ADR 0087](../../../docs/decisions/0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md),
//! the note of 2026-11-25; ADR 0009).
//!
//! The orchestrator does not verify a person's signature there: it attests the prompt with a statement of its own,
//! which the verifier behind the model gateway checks beside the person's envelope. What is signed is built by the pure
//! crate `orch-prompt-statement`; a signer takes only an [`AttestedPayload`], which exists only for a statement whose
//! `user` passed the integrity rule, so a refused statement cannot reach a signer.
//!
//! | Call | Does |
//! |---|---|
//! | [`sign`](PromptSigner::sign) | The compact JWS of the payload (attached), its protected header `{"alg":"EdDSA","typ":"prompt-statement/v1","x5c":[leaf, intermediate]}` in RFC 8785 form, signed with the leaf's Ed25519 key. |
//! | [`state`](PromptSigner::state) | [`SignerState::Ready`], [`SignerState::Renewing`] or [`SignerState::Unavailable`]. |
//!
//! # What an implementation owes
//!
//! - **The statement as the contract writes it**: the payload attached byte for byte, the header above and nothing
//!   else in it, and a signature that verifies under the key of the first `x5c` certificate. The conformance testkit
//!   (`prompt_signer_conformance!`) checks each.
//! - **Its state, told truthfully**: `Unavailable` (no certificate it may sign with) refuses every `sign` with
//!   [`SignerError::Unavailable`]; `Ready` and `Renewing` sign.
//! - **No key, token, CSR or URL in an error or a `Debug`.** The leaf's serial may be shown; it is not a secret.

use std::fmt;
use std::future::Future;

use orch_core::{Classify, ErrorClass};
pub use orch_prompt_statement::AttestedPayload;

/// Whether a signer can sign now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignerState {
    /// A valid certificate with more than its renewal margin left.
    Ready,
    /// A valid certificate inside its renewal margin, while a new one is asked for: it still signs.
    Renewing,
    /// No certificate it may sign with (expired and not renewed, never obtained): it refuses.
    Unavailable,
}

/// A signed statement: a compact JWS with an attached payload. Not a secret, but the payload names the person, so
/// `Debug` shows the length only.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedStatement(String);

impl SignedStatement {
    /// Wraps the compact JWS an adapter wrote.
    pub fn new(compact: String) -> Self {
        SignedStatement(compact)
    }

    /// The compact JWS: the value the model calls of the run carry.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The compact JWS, owned.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for SignedStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedStatement")
            .field("len", &self.0.len())
            .finish()
    }
}

/// Why a statement was not signed. The `detail` is a fixed sentence of the adapter: no key, token, CSR or URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SignerError {
    /// The signer is [`SignerState::Unavailable`]: no certificate it may sign with. May succeed once one is obtained.
    #[error("the prompt signer is unavailable: {detail}")]
    Unavailable {
        /// What is missing.
        detail: &'static str,
    },
    /// The signature itself failed. Alert.
    #[error("the prompt signer failed: {detail}")]
    Failed {
        /// What failed.
        detail: &'static str,
    },
}

impl Classify for SignerError {
    fn class(&self) -> ErrorClass {
        match self {
            SignerError::Unavailable { .. } => ErrorClass::Transient,
            SignerError::Failed { .. } => ErrorClass::Internal,
        }
    }
}

/// Signs the orchestrator's prompt statements with a certified key. See the module's docs for what an implementation
/// owes.
pub trait PromptSigner: Send + Sync + 'static {
    /// The compact JWS of `payload`, with `x5c` in its protected header.
    ///
    /// # Errors
    /// [`SignerError::Unavailable`] while [`state`](Self::state) is `Unavailable`; [`SignerError::Failed`] otherwise.
    fn sign(
        &self,
        payload: &AttestedPayload,
    ) -> impl Future<Output = Result<SignedStatement, SignerError>> + Send;

    /// Whether it can sign now.
    fn state(&self) -> SignerState;
}
