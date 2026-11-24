//! [`SecretStore`](orch_ports::SecretStore) in the orchestrator's own Postgres (ADR 0046, the
//! amendment of 2026-10-11, slice 3): the default adapter of the port, `orch-secrets-pg`.
//!
//! Each secret is one row of `user_secrets`, **sealed** with AES-256-GCM under a key of the
//! configuration ([`Keyring`]): the database holds ciphertext and nothing it can read. The
//! associated data of the seal is `owner ‖ server ‖ version ‖ kid` (length-prefixed, see
//! [the envelope](#the-envelope)), so a row moved to another person, another server or
//! another version does not open. Versions come from one Postgres sequence, taken inside a
//! per-key advisory lock, so the writers of a key are serialized and the version a key holds is
//! the greatest it was given. A write is a `READ COMMITTED` transaction whatever the database
//! defaults to, and its wait for the lock is bounded ([`LOCK_TIMEOUT`]): a key whose lock is held
//! for long makes its writers [`Unavailable`](orch_ports::SecretError::Unavailable) and does not
//! take the pool from the other keys. [`PgSecretStore::rewrap`] re-seals the rows that are under an older
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
//! # The vault
//!
//! [`PgVault`] is the [`VaultStore`](orch_ports::VaultStore) of the same database: a person's
//! **named** secrets, a new version for every write, which a configuration names as
//! `vault:<name>#v<n>` and never holds (ADR 0083, amended 2026-11-21). Its rows are in
//! `vault_secrets`, sealed under the same keyring and bound to `(owner, name, version)` with a
//! `vault` domain in front of the associated data, so a row moved to another owner, name or
//! version — or into `user_secrets` — does not open. A write marks the version that was current
//! as superseded and adds the next one in one transaction under the name's own advisory lock; a
//! superseded version reads for [`SUPERSEDED_GRACE`](orch_ports::SUPERSEDED_GRACE) (24 hours) and
//! is refused after that, on the store's [`Clock`](orch_ports::Clock). [`PgVault::purge`] is the
//! housekeeping that removes what is past it.
//!
//! # Migrations
//!
//! [`PgSecretStore::migrate`] applies this crate's own migrations (`migrations/`), recorded in
//! their own history table [`MIGRATIONS_TABLE`], so the crate does not depend on
//! `orch-store-postgres` and the two histories never see each other's versions. The vault's table
//! is one of them, so a deployment that migrates the secret store has the vault too.
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
mod vault;

pub use keys::{KEY_LEN, KeyBytes, KeyError, KeyId, Keyring, MAX_KEY_ID_LEN};
pub use rewrap::{MAX_LISTED_FAILURES, RewrapFailure, RewrapFailureKind, RewrapReport};
pub use store::{LOCK_TIMEOUT, MIGRATIONS_TABLE, PgSecretStore, advisory_lock_key, is_outage};
pub use vault::{PgVault, vault_advisory_lock_key};
