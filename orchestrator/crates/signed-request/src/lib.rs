//! Signed requests to a tool server ([`docs/api/signed-tool-requests-v1.md`](../../../docs/api/signed-tool-requests-v1.md),
//! [ADR 0080](../../../docs/decisions/0080-requests-to-a-tool-server-are-signed-for-the-person.md)).
//!
//! A [`Signer`] turns one HTTP request to a tool server into the value of the header [`HEADER`]: a tagged `COSE_Sign1`
//! (Ed25519) in the *claim object* profile of `cratestack-cose` 0.17.2, which `cratestack_cose::open_claims` verifies. The
//! protected header carries the CWT claims `aud`, `exp`, `iat` and `cti`, and the payload binds the request: its method, its
//! path, the SHA-256 of its exact body bytes, the thread and the thread's owner.
//!
//! The object is written by the real `cratestack_cose::sign_claims` and, on the receiving side, opened by the real
//! `cratestack_cose::open_claims`; this crate owns the payload (the request binding), the limits of the contract, the key and
//! the checks around those calls. The bytes are pinned by the known-answer test (`tests/vectors/v1.json`).
//!
//! This crate is **pure**: no I/O and no clock (every function that needs the time takes it). Signing and verifying are
//! `async` because the library's functions are; with the key held here and the pinned keys in memory they wait for nothing
//! and need no particular runtime.
//!
//! ```
//! use orch_signed_request::{Method, Request, Signer, SigningKey, verify::{Incoming, Verifier}};
//! use uuid::Uuid;
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
//! // a throwaway key; never use a key that is written in a file
//! let key = SigningKey::from_seed(b"throwaway-ed25519-seed-do-not-us").unwrap();
//! let public = key.public_key();
//! let signer = Signer::new(key, "https://tools.example/mcp", 30).unwrap();
//!
//! let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
//! let thread: Uuid = "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c33".parse().unwrap();
//! let header = signer
//!     .sign(
//!         &Request { method: Method::Post, path: "/mcp", body, thread, owner: "amina@example.com" },
//!         1_790_000_000,
//!     )
//!     .await
//!     .unwrap();
//!
//! // the other side: the pinned public key, its own name, and the request it received
//! let verifier = Verifier::new("https://tools.example/mcp", 30).pin(public).unwrap();
//! let seen = verifier
//!     .verify(
//!         &[&header],
//!         &Incoming { method: "POST", path: "/mcp", query: None, body, content_encoding: None },
//!         1_790_000_010,
//!     )
//!     .await
//!     .unwrap();
//! assert_eq!(seen.owner, "amina@example.com");
//! assert_eq!(seen.thread, thread);
//! # });
//! ```

mod key;
mod sign;
#[cfg(feature = "verify")]
pub mod verify;

pub use key::{KID_LEN, KeyError, SigningKey, kid_of};
pub use sign::{
    CTI_LEN, DEFAULT_TTL_SECS, HEADER, MAX_AUDIENCE_BYTES, MAX_HEADER_BYTES, MAX_OWNER_BYTES,
    MAX_PATH_BYTES, MAX_TTL_SECS, Method, Request, SignError, Signer, TYP,
};
