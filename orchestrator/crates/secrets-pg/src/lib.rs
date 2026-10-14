//! [`SecretStore`](orch_ports::SecretStore) in the orchestrator's own Postgres (ADR 0046, the
//! amendment of 2026-10-11, slice 3): the default adapter of the port, `orch-secrets-pg`.
//!
//! Each secret is one row of `user_secrets`, **sealed** with AES-256-GCM under a key of the
//! configuration ([`Keyring`]): the database holds ciphertext and nothing it can read. The
//! associated data of the seal is `owner ‖ server ‖ version ‖ kid` (length-prefixed, see
//! [the envelope](#the-envelope)), so a row moved to another person, another server or
//! another version does not open. Versions come from one Postgres sequence, taken inside a
//! per-key advisory lock, so the writers of a key are serialized and the version a key holds is
//! the greatest it was given. [`PgSecretStore::rewrap`] re-seals the rows that are under an older
//! key, **at the version they have**, so that a key is rotated by id without a refresh in flight
//! losing the token it was waiting for.
//!
//! # The envelope
//!
//! `{v, kid, nonce, ciphertext + tag}`, the columns `v`, `kid`, `nonce` and `ciphertext` of the
//! row. `v` is the format (1). `kid` is the id of the key that sealed it. `nonce` is 96 random
//! bits from the operating system, new for every seal. The ciphertext ends with the 128-bit tag.
//! The associated data, which the tag authenticates and which is not stored, is
//!
//! ```text
//! v (1 byte) ‖ len(owner) ‖ owner ‖ len(server) ‖ server ‖ version ‖ len(kid) ‖ kid
//! ```
//!
//! with every `len` and the `version` eight bytes, big-endian.
//!
//! # Migrations
//!
//! [`PgSecretStore::migrate`] applies this crate's own migrations (`migrations/`), recorded in
//! their own history table [`MIGRATIONS_TABLE`], so the crate does not depend on
//! `orch-store-postgres` and the two histories never see each other's versions.
//!
//! ```no_run
//! # async fn demo(url: &str) -> Result<(), Box<dyn std::error::Error>> {
//! use orch_secrets_pg::{KeyBytes, KeyId, Keyring, PgSecretStore};
//!
//! let key = KeyBytes::from_hex(&std::env::var("USER_SECRETS_KEY")?)?; // openssl rand -hex 32
//! let keys = Keyring::new(KeyId::parse("k1")?, [(KeyId::parse("k1")?, key)])?;
//! let store = PgSecretStore::connect(url, keys).await?;
//! store.migrate().await?;
//! # Ok(()) }
//! ```

mod envelope;
mod keys;
mod rewrap;
mod store;

pub use keys::{KEY_LEN, KeyBytes, KeyError, KeyId, Keyring, MAX_KEY_ID_LEN};
pub use rewrap::{MAX_LISTED_FAILURES, RewrapFailure, RewrapFailureKind, RewrapReport};
pub use store::{MIGRATIONS_TABLE, PgSecretStore, advisory_lock_key};
