//! The prompt statement of the cloud path ([ADR 0087](../../../docs/decisions/0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md),
//! the note of 2026-11-25).
//!
//! On the cloud path the orchestrator does not verify the person's signature. It checks that a signing app's prompt
//! carries one, passes it on byte for byte, and **attests** the prompt with a statement of its own, signed by a
//! certified key behind the `PromptSigner` port of `orch-ports`; the verifier behind the model
//! gateway checks both. This crate builds what is signed:
//!
//! - [`Statement`]: the members (`typ`, `aud`, `iat`, `exp`, `jti`, `src`, `sub`, `azp`, `thread`, `agent`, `a2a`,
//!   `content_sha256`, `user`), and [`UserClaims`], the `user` of a person's envelope (its hash and the members of the
//!   person's payload that only the chat request holds);
//! - the **integrity rule**: [`Statement::attest`] compares `thread`, `agent`, `msg`, `content_sha256` and
//!   `props_sha256` of `user` with the AG-UI request ([`Received`]) and refuses a difference with ADR 0087's
//!   `403 prompt_signature_invalid`; only then is there an [`AttestedPayload`], the one thing a signer takes;
//! - RFC 8785 ([`canonical`]) and the hashes of the contract ([`hash_of`], [`props_hash`], [`sha256_b64u`]);
//! - [`jws`]: the protected header with `x5c`, the signing input and the compact form, shared by the signer adapters.
//!
//! **Pure**: no I/O, no async, no clock (the caller gives `iat`), no randomness (the caller gives `jti`). It never
//! verifies a signature.
//!
//! ```
//! use orch_prompt_statement::{A2aIds, Received, Source, Statement, UserClaims, hash_of};
//! use serde_json::json;
//!
//! let content = json!("Run the tests again.");
//! let received = Received {
//!     thread: "thread-1", agent: "adam", msg: "msg-1", content: &content, forwarded_props: None,
//! };
//! let statement = Statement {
//!     aud: "https://models.example".into(), iat: 1_795_000_000, exp: 1_795_003_600, jti: [7; 16],
//!     src: Source::Chat, sub: "person-1".into(), azp: "app".into(), thread: "thread-1".into(),
//!     agent: "coder".into(),
//!     a2a: A2aIds { context_id: "ctx-1".into(), message_id: "m-1".into() },
//!     content_sha256: hash_of(&json!([{ "text": "Run the tests again." }])),
//!     user: Some(UserClaims::from_request("https://api.example", &received, "eyJh..c2ln")),
//! };
//! assert!(statement.attest(Some(&received)).is_ok());
//!
//! let other = json!("Delete the branch.");
//! let tampered = Received { content: &other, ..received };
//! assert!(statement.attest(Some(&tampered)).unwrap_err().is_prompt_signature_invalid());
//! ```

mod jcs;
pub mod jws;
mod statement;

pub use jcs::{canonical, hash_of, props_hash, sha256_b64u};
pub use statement::{
    A2aIds, AttestedPayload, Field, JTI_LEN, MAX_LIFETIME_SECS, PERSON_TYP, Received,
    STATEMENT_TYP, Source, Statement, StatementError, UserClaims, statement_exp,
};
