//! Signed requests to a tool server ([`docs/api/signed-tool-requests-v1.md`](../../../docs/api/signed-tool-requests-v1.md),
//! [ADR 0067](../../../docs/decisions/0067-requests-to-a-tool-server-are-signed-for-the-person.md)).
//!
//! A [`Signer`] turns one HTTP request to a tool server into the value of the header [`HEADER`]: a tagged `COSE_Sign1`
//! (Ed25519) in the *claim object* profile of `cratestack-cose` 0.17.2, which `cratestack_cose::open_claims` verifies. The
//! protected header carries the CWT claims `aud`, `exp`, `iat` and `cti`, and the payload binds the request: its method, its
//! path, the SHA-256 of its exact body bytes, the thread and the thread's owner.
//!
//! This crate is **pure**: no I/O, no async, and no clock (every function that needs the time takes it). It does not use
//! `cratestack-cose`, which needs Rust 1.98 where the orchestrator is on 1.94; the bytes it writes are held to that crate's
//! by the known-answer test of this crate and by `interop/`, a crate with a toolchain of its own that runs the real
//! `sign_claims` and `open_claims` against them.
//!
//! ```
//! use orch_signed_request::{Method, Request, Signer, SigningKey, verify::{Incoming, Verifier}};
//! use uuid::Uuid;
//!
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
//!     .unwrap();
//! assert_eq!(seen.owner, "amina@example.com");
//! assert_eq!(seen.thread, thread);
//! ```

mod cbor;
mod key;
mod sign;
#[cfg(feature = "verify")]
pub mod verify;

pub use key::{KID_LEN, KeyError, SigningKey, kid_of};
pub use sign::{
    CTI_LEN, DEFAULT_TTL_SECS, HEADER, MAX_AUDIENCE_BYTES, MAX_HEADER_BYTES, MAX_OWNER_BYTES,
    MAX_PATH_BYTES, MAX_TTL_SECS, Method, Request, SignError, Signer, TYP,
};
