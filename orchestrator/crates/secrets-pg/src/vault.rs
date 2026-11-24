//! `PgVault`: the [`VaultStore`] of the orchestrator's own database.
//!
//! A person's named secrets are rows of `vault_secrets`, **sealed** with the same AES-256-GCM
//! keyring as the tool servers' credentials ([`crate::PgSecretStore`], ADR 0046), one row per
//! `(owner, name, version)`. The associated data of the seal is `vault ‖ owner ‖ name ‖ version ‖
//! kid` (see [`crate::envelope`]'s docs), so a row moved to another owner, name or version — or
//! into `user_secrets` — does not open.
//!
//! A write takes the **name's** advisory lock (a domain of its own, so a tool server's secret and
//! a named secret never wait for each other), numbers the new version one past the greatest the
//! name has had, marks the version that was current as superseded, and inserts the new one, all in
//! one `READ COMMITTED` transaction whose wait for the lock is bounded
//! ([`crate::LOCK_TIMEOUT`]). The clock is the store's (`Clock`, the wall clock unless a test gives
//! another), so the 24 hours of [`SUPERSEDED_GRACE`] are the store's to keep and a test's to move.

use std::sync::Arc;

use jiff::Timestamp;
use orch_core::UserId;
use orch_ports::{
    Clock, SUPERSEDED_GRACE, SecretMaterial, SecretVersion, VaultError, VaultName, VaultRef,
    VaultStore,
};

use crate::envelope::{Envelope, NONCE_LEN, SealError};
use crate::keys::Keyring;
use crate::store::{PgSecretStore, VAULT_LOCK_DOMAIN, column, db_err, open_err};

/// The columns of a row of `vault_secrets`, in the order [`VaultRow`] reads them.
pub(crate) const VAULT_COLUMNS: &str = "version, v, kid, nonce, ciphertext, superseded";

/// What a row holds, as [`VAULT_COLUMNS`] lists it.
type VaultRow = (i64, i16, String, Vec<u8>, Vec<u8>, Option<Timestamp>);

/// [`VaultStore`] in Postgres: a person's named secrets, with their versions, sealed under the
/// keys of the crate's [`Keyring`].
///
/// Cheap to clone: clones share the pool, the keys and the clock.
#[derive(Clone)]
pub struct PgVault {
    store: PgSecretStore,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PgVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgVault")
            .field("store", &self.store)
            .field("now", &self.clock.now())
            .finish()
    }
}

impl PgSecretStore {
    /// The vault over this store's pool and keys, its 24-hour grace on the wall clock.
    ///
    /// The migrations of this crate ([`migrate`](Self::migrate)) create the table, so a deployment
    /// that already runs the secret store runs the vault with no further boot step.
    pub fn vault(&self) -> PgVault {
        PgVault {
            store: self.clone(),
            clock: Arc::new(orch_ports::SystemClock),
        }
    }
}

/// The key of the advisory lock that serializes the writers of one secret of a person's vault:
/// every [`put`](VaultStore::put) of it takes
/// `pg_advisory_xact_lock(vault_advisory_lock_key(owner, name))`. Anything else that writes
/// `vault_secrets` by hand (a repair, a migration) takes the same lock to be ordered with them.
///
/// It is a domain of its own: a named secret and a tool server's credential of the same text never
/// wait for each other ([`advisory_lock_key`](crate::advisory_lock_key)).
pub fn vault_advisory_lock_key(owner: &UserId, name: &VaultName) -> i64 {
    crate::store::vault_lock_key(owner.as_str(), name.as_str())
}

impl PgVault {
    /// A vault over `store`'s pool and keys, whose grace runs on `clock` (a test's, or the wall
    /// clock).
    pub fn new(store: PgSecretStore, clock: Arc<dyn Clock>) -> Self {
        PgVault { store, clock }
    }

    /// The keys of this vault.
    pub fn keyring(&self) -> &Keyring {
        self.store.keyring()
    }

    /// The clock the grace runs on.
    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Seals `plaintext` as the secret of `name` at `version`.
    fn seal(
        &self,
        owner: &str,
        name: &str,
        version: u64,
        plaintext: &[u8],
    ) -> Result<Envelope, VaultError> {
        self.store
            .keyring()
            .seal_in_vault(owner, name, version, plaintext)
            .map_err(|e| match e {
                SealError::NoRandomness => {
                    VaultError::unavailable("the operating system gave no random bytes")
                }
                SealError::Cipher => VaultError::unavailable("the secret could not be sealed"),
            })
    }

    /// Opens what a row holds.
    fn open(
        &self,
        owner: &str,
        name: &str,
        entry: &VaultEntry,
    ) -> Result<SecretMaterial, VaultError> {
        self.store
            .keyring()
            .open_in_vault(owner, name, entry.version, &entry.envelope)
            .map_err(|e| VaultError::from(open_err(e)))
    }

    /// Why a `get` found no row: the name is not there, or only that version is not.
    async fn missing(&self, owner: &str, name: &str) -> Result<VaultError, VaultError> {
        let conn = self.store.conn().await?;
        let select = conn
            .prepare_cached("SELECT 1 FROM vault_secrets WHERE owner = $1 AND name = $2 LIMIT 1")
            .await
            .map_err(db_err)?;
        let known = conn
            .query_opt(&select, &[&owner, &name])
            .await
            .map_err(db_err)?
            .is_some();
        Ok(if known {
            VaultError::UnknownVersion
        } else {
            VaultError::UnknownName
        })
    }
}

/// A row of `vault_secrets`, decoded.
struct VaultEntry {
    version: u64,
    envelope: Envelope,
    /// When a later write superseded it; `None` while it is the current version.
    superseded: Option<Timestamp>,
}

impl VaultEntry {
    /// # Errors
    /// [`VaultError::Corrupt`] for a row that is not shaped as this crate writes (the table's
    /// checks make that a row somebody wrote by hand).
    fn decode(
        (version, v, kid, nonce, ciphertext, superseded): VaultRow,
    ) -> Result<Self, VaultError> {
        let version = u64::try_from(version)
            .map_err(|_| VaultError::corrupt("a stored version is not a positive number"))?;
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| VaultError::corrupt("a stored nonce is not 96 bits"))?;
        Ok(VaultEntry {
            version,
            envelope: Envelope {
                v,
                kid,
                nonce,
                ciphertext,
            },
            superseded,
        })
    }
}

impl VaultStore for PgVault {
    async fn put(
        &self,
        owner: &UserId,
        name: &VaultName,
        material: &SecretMaterial,
    ) -> Result<VaultRef, VaultError> {
        let now = self.clock.now();
        let (owner, name_text) = (owner.as_str(), name.as_str());
        let mut conn = self.store.conn().await?;
        let tx = self
            .store
            .locked_in(&mut conn, VAULT_LOCK_DOMAIN, owner, name_text)
            .await?;
        let greatest = tx
            .prepare_cached(
                "SELECT coalesce(max(version), 0) FROM vault_secrets WHERE owner = $1 AND name = $2",
            )
            .await
            .map_err(db_err)?;
        let row = tx
            .query_one(&greatest, &[&owner, &name_text])
            .await
            .map_err(db_err)?;
        let greatest: i64 = column(&row, 0)?;
        let version = u64::try_from(greatest)
            .map_err(|_| VaultError::corrupt("a stored version is not a positive number"))?
            .checked_add(1)
            .ok_or_else(|| VaultError::unavailable("the versions of that name ran out"))?;
        let sealed = self.seal(owner, name_text, version, material.expose())?;
        let supersede = tx
            .prepare_cached(
                "UPDATE vault_secrets SET superseded = $3 \
                 WHERE owner = $1 AND name = $2 AND superseded IS NULL",
            )
            .await
            .map_err(db_err)?;
        tx.execute(&supersede, &[&owner, &name_text, &now])
            .await
            .map_err(db_err)?;
        let insert = tx
            .prepare_cached(
                "INSERT INTO vault_secrets \
                   (owner, name, version, v, kid, nonce, ciphertext, created) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .await
            .map_err(db_err)?;
        tx.execute(
            &insert,
            &[
                &owner,
                &name_text,
                &to_i64(version)?,
                &sealed.v,
                &sealed.kid,
                &sealed.nonce.as_slice(),
                &sealed.ciphertext,
                &now,
            ],
        )
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(VaultRef::new(name.clone(), SecretVersion::new(version)))
    }

    async fn get(
        &self,
        owner: &UserId,
        reference: &VaultRef,
    ) -> Result<SecretMaterial, VaultError> {
        let now = self.clock.now();
        let (owner, name) = (owner.as_str(), reference.name().as_str());
        // a version past `bigint` was never written: the answer is the same as for any other
        let Ok(version) = i64::try_from(reference.version().get()) else {
            return Err(VaultError::UnknownVersion);
        };
        let conn = self.store.conn().await?;
        let select = conn
            .prepare_cached(&format!(
                "SELECT {VAULT_COLUMNS} FROM vault_secrets \
                 WHERE owner = $1 AND name = $2 AND version = $3"
            ))
            .await
            .map_err(db_err)?;
        let row = conn
            .query_opt(&select, &[&owner, &name, &version])
            .await
            .map_err(db_err)?;
        drop(conn);
        let Some(row) = row else {
            return Err(self.missing(owner, name).await?);
        };
        let entry = VaultEntry::decode((
            column(&row, 0)?,
            column(&row, 1)?,
            column(&row, 2)?,
            column(&row, 3)?,
            column(&row, 4)?,
            column(&row, 5)?,
        ))?;
        // past its grace the value is refused, not returned stale, and whether the row is still
        // here is not the caller's business
        if entry
            .superseded
            .is_some_and(|at| now.duration_since(at) >= SUPERSEDED_GRACE)
        {
            return Err(VaultError::UnknownVersion);
        }
        self.open(owner, name, &entry)
    }

    async fn purge(&self) -> Result<u64, VaultError> {
        let now = self.clock.now();
        let cutoff = now
            .checked_sub(SUPERSEDED_GRACE)
            .map_err(|_| VaultError::unavailable("the clock is outside the range of an instant"))?;
        let conn = self.store.conn().await?;
        let delete = conn
            .prepare_cached(
                "DELETE FROM vault_secrets WHERE superseded IS NOT NULL AND superseded <= $1",
            )
            .await
            .map_err(db_err)?;
        let removed = conn.execute(&delete, &[&cutoff]).await.map_err(db_err)?;
        Ok(removed)
    }
}

fn to_i64(version: u64) -> Result<i64, VaultError> {
    i64::try_from(version).map_err(|_| VaultError::corrupt("a version does not fit a stored row"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_decoded_or_a_corrupt_one_is_refused() {
        let entry =
            VaultEntry::decode((3, 1, "k1".to_owned(), vec![0; NONCE_LEN], vec![0; 16], None))
                .unwrap();
        assert_eq!(entry.version, 3);
        assert!(entry.superseded.is_none());

        let bad_rows: Vec<VaultRow> = vec![
            (
                -1,
                1,
                "k1".to_owned(),
                vec![0; NONCE_LEN],
                vec![0; 16],
                None,
            ),
            (
                1,
                1,
                "k1".to_owned(),
                vec![0; NONCE_LEN - 1],
                vec![0; 16],
                None,
            ),
        ];
        for bad in bad_rows {
            assert!(matches!(
                VaultEntry::decode(bad),
                Err(VaultError::Corrupt(_))
            ));
        }
    }
}
