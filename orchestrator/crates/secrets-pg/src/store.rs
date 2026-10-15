//! `PgSecretStore`: the [`SecretStore`] of the orchestrator's own database.

use std::sync::Arc;
use std::time::Duration;

use orch_core::UserId;
use orch_ports::{SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion};
use sha2::{Digest as _, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};

use crate::envelope::{Envelope, NONCE_LEN, OpenError, SealError};
use crate::keys::Keyring;

/// The name of this crate's own history of migrations. `orch-store-postgres` records its own in
/// sqlx's default `_sqlx_migrations`; two crates sharing a history table would each see the
/// other's migrations as ones it does not know (`VersionMissing`) and refuse to start.
pub const MIGRATIONS_TABLE: &str = "_orch_secrets_pg_migrations";

/// The pool size of [`PgSecretStore::connect`].
const DEFAULT_MAX_CONNECTIONS: u32 = 4;

/// How long a call of a store made by [`PgSecretStore::connect`] waits for a connection, and so how
/// long `connect` itself tries a database that is down: ten seconds, not sqlx's thirty. The broker
/// that calls the store gives up on a request after ten.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a write waits for a lock, the lock of its key first, before it gives up as
/// [`SecretError::Unavailable`]: two seconds.
///
/// Every write of a key holds a pooled connection from the moment it asks for the key's lock, so
/// without a bound one lock that is held for long (a replica that is wedged or paused, an
/// operator's repair that has not committed) would keep every writer of that key on a connection
/// after its caller has given up, until the pool is empty and a `get` of an unrelated key fails
/// for want of one. A write holds the lock for milliseconds; two seconds is long enough to ride out
/// a slow commit and short enough that a stuck key costs a connection for two seconds at a time.
/// It is `SET LOCAL lock_timeout` in the write's transaction, so it also bounds a wait for a row
/// lock that somebody's hand-made transaction holds.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// What a row holds, in the order of [`COLUMNS`].
pub(crate) type Row = (i64, i16, String, Vec<u8>, Vec<u8>);

/// The columns of a row, in the order of [`Row`].
pub(crate) const COLUMNS: &str = "version, v, kid, nonce, ciphertext";

/// [`SecretStore`] in Postgres: one row per `(owner, server)` in `user_secrets`, the material
/// sealed with AES-256-GCM under a key of the [`Keyring`] (see the crate's docs).
///
/// Cheap to clone: clones share the pool and the keys.
#[derive(Clone, Debug)]
pub struct PgSecretStore {
    pub(crate) pool: PgPool,
    pub(crate) keys: Arc<Keyring>,
}

impl PgSecretStore {
    /// Connects a pool of four connections, and waits at most ten seconds for a connection (here
    /// and in every call). Does not migrate; call [`migrate`](Self::migrate) at boot.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] if the database cannot be reached.
    pub async fn connect(url: &str, keys: Keyring) -> Result<Self, SecretError> {
        Self::connect_with(url, keys, DEFAULT_MAX_CONNECTIONS).await
    }

    /// Like [`connect`](Self::connect) with an explicit pool size.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] if the database cannot be reached.
    pub async fn connect_with(
        url: &str,
        keys: Keyring,
        max_connections: u32,
    ) -> Result<Self, SecretError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect(url)
            .await
            .map_err(db_err)?;
        Ok(Self::from_pool(pool, keys))
    }

    /// Wraps an existing pool (the orchestrator's own, or one that pins a `search_path` in a test).
    pub fn from_pool(pool: PgPool, keys: Keyring) -> Self {
        PgSecretStore {
            pool,
            keys: Arc::new(keys),
        }
    }

    /// The keys of this store.
    pub fn keyring(&self) -> &Keyring {
        &self.keys
    }

    /// Applies this crate's migrations, recorded in [`MIGRATIONS_TABLE`]. Idempotent, and safe to
    /// call from every replica at boot: sqlx takes a database-wide advisory lock while it runs.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] if the database cannot be reached or rejects a statement;
    /// [`SecretError::Corrupt`] if the history does not match the migrations of this build.
    pub async fn migrate(&self) -> Result<(), SecretError> {
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.dangerous_set_table_name(MIGRATIONS_TABLE);
        migrator.run(&self.pool).await.map_err(migrate_err)
    }

    /// Closes the pool: every later call fails as [`SecretError::Unavailable`], as a database that
    /// cannot be reached does. For a clean shutdown, and for the conformance testkit's breaker.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Begins a transaction that holds the key's lock until it ends. Every write of one key runs
    /// in one of these, so the writers of a key are serialized and take their versions from the
    /// sequence in the order they land.
    ///
    /// The transaction is `READ COMMITTED` whatever the database or the role defaults to: the
    /// statements after the lock must see what the writer ahead of it committed, and under
    /// `REPEATABLE READ` the snapshot is taken at the lock statement, before the wait, so the
    /// writer that waited would fail with a serialization error (SQLSTATE `40001`). The wait for
    /// the lock is bounded by [`LOCK_TIMEOUT`]; a wait that ends is `Unavailable`, with the
    /// connection given back at once.
    pub(crate) async fn locked(
        &self,
        owner: &str,
        server: &str,
    ) -> Result<Transaction<'static, Postgres>, SecretError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await
            .map_err(db_err)?;
        let taken = async {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SET LOCAL lock_timeout = '{}ms'",
                LOCK_TIMEOUT.as_millis()
            )))
            .execute(&mut *tx)
            .await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(lock_key(owner, server))
                .execute(&mut *tx)
                .await
        }
        .await;
        if let Err(err) = taken {
            // do not leave the connection to the pool's drop path with an aborted transaction
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        Ok(tx)
    }

    /// Seals `material` for the key at `version` under the current key.
    pub(crate) fn seal(
        &self,
        owner: &str,
        server: &str,
        version: u64,
        plaintext: &[u8],
    ) -> Result<Envelope, SecretError> {
        self.keys
            .seal(owner, server, version, plaintext)
            .map_err(|e| match e {
                SealError::NoRandomness => {
                    SecretError::unavailable("the operating system gave no random bytes")
                }
                SealError::Cipher => SecretError::unavailable("the secret could not be sealed"),
            })
    }

    /// Opens what a row holds.
    pub(crate) fn open(
        &self,
        owner: &str,
        server: &str,
        stored: &Stored,
    ) -> Result<SecretMaterial, SecretError> {
        self.keys
            .open(owner, server, stored.version, &stored.envelope)
            .map_err(open_err)
    }
}

/// A row, decoded: the version and the envelope.
pub(crate) struct Stored {
    pub(crate) version: u64,
    pub(crate) envelope: Envelope,
}

impl Stored {
    /// Decodes a row.
    ///
    /// # Errors
    /// [`SecretError::Corrupt`] for a row that is not shaped as this crate writes (the table's
    /// checks make that a row somebody wrote by hand).
    pub(crate) fn decode((version, v, kid, nonce, ciphertext): Row) -> Result<Self, SecretError> {
        let version = u64::try_from(version)
            .map_err(|_| SecretError::corrupt("a stored version is not a positive number"))?;
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| SecretError::corrupt("a stored nonce is not 96 bits"))?;
        Ok(Stored {
            version,
            envelope: Envelope {
                v,
                kid,
                nonce,
                ciphertext,
            },
        })
    }
}

/// The next version of the store's sequence. Called with the key's lock held.
async fn next_version(tx: &mut Transaction<'static, Postgres>) -> Result<u64, SecretError> {
    let (version,): (i64,) = sqlx::query_as("SELECT nextval('user_secret_versions')")
        .fetch_one(&mut **tx)
        .await
        .map_err(db_err)?;
    u64::try_from(version)
        .map_err(|_| SecretError::unavailable("the version sequence gave a number out of range"))
}

impl SecretStore for PgSecretStore {
    async fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        let (owner, server) = (owner.as_str(), server.to_string());
        let mut tx = self.locked(owner, &server).await?;
        let version = next_version(&mut tx).await?;
        let sealed = self.seal(owner, &server, version, material.expose())?;
        sqlx::query(
            "INSERT INTO user_secrets (owner, server, version, v, kid, nonce, ciphertext) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (owner, server) DO UPDATE SET \
               version = EXCLUDED.version, v = EXCLUDED.v, kid = EXCLUDED.kid, \
               nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, \
               updated = clock_timestamp()",
        )
        .bind(owner)
        .bind(&server)
        .bind(to_i64(version)?)
        .bind(sealed.v)
        .bind(&sealed.kid)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(SecretVersion::new(version))
    }

    async fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        let (owner, server) = (owner.as_str(), server.to_string());
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM user_secrets WHERE owner = $1 AND server = $2"
        )))
        .bind(owner)
        .bind(&server)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else { return Ok(None) };
        let stored = Stored::decode(row)?;
        let material = self.open(owner, &server, &stored)?;
        Ok(Some((SecretVersion::new(stored.version), material)))
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        let (owner, server) = (owner.as_str(), server.to_string());
        let mut tx = self.locked(owner, &server).await?;
        sqlx::query("DELETE FROM user_secrets WHERE owner = $1 AND server = $2")
            .bind(owner)
            .bind(&server)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        // a version that cannot be stored is not the stored one
        let Ok(expected) = i64::try_from(expected.get()) else {
            return Err(SecretError::Conflict);
        };
        let (owner, server) = (owner.as_str(), server.to_string());
        let mut tx = self.locked(owner, &server).await?;
        let held: Option<(i64,)> =
            sqlx::query_as("SELECT version FROM user_secrets WHERE owner = $1 AND server = $2")
                .bind(owner)
                .bind(&server)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
        if held != Some((expected,)) {
            // nothing is written, and the transaction ends with the lock released
            return Err(SecretError::Conflict);
        }
        let version = next_version(&mut tx).await?;
        let sealed = self.seal(owner, &server, version, material.expose())?;
        let done = sqlx::query(
            "UPDATE user_secrets SET version = $3, v = $4, kid = $5, nonce = $6, ciphertext = $7, \
               updated = clock_timestamp() \
             WHERE owner = $1 AND server = $2 AND version = $8",
        )
        .bind(owner)
        .bind(&server)
        .bind(to_i64(version)?)
        .bind(sealed.v)
        .bind(&sealed.kid)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .bind(expected)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if done.rows_affected() != 1 {
            return Err(SecretError::Conflict);
        }
        tx.commit().await.map_err(db_err)?;
        Ok(SecretVersion::new(version))
    }
}

fn to_i64(version: u64) -> Result<i64, SecretError> {
    i64::try_from(version)
        .map_err(|_| SecretError::unavailable("the version sequence gave a number out of range"))
}

/// The key of the advisory lock that serializes the writers of `(owner, server)`: every write this
/// crate makes (`put`, `rotate`, `delete`, `rewrap`) runs in a transaction that has taken
/// `pg_advisory_xact_lock(advisory_lock_key(owner, server))` first. Anything else that writes
/// `user_secrets` by hand (a repair, a migration) takes the same lock to be ordered with them.
pub fn advisory_lock_key(owner: &UserId, server: &SecretServer) -> i64 {
    lock_key(owner.as_str(), &server.to_string())
}

/// The advisory lock of a key: 64 bits of the SHA-256 of a domain, the owner and the server, each
/// text with its length in front. Two keys that share a lock only wait for each other; the lock's
/// job is to serialize the writers of one key, and no other user of advisory locks in the database
/// is likely to hit one of 2^64.
pub(crate) fn lock_key(owner: &str, server: &str) -> i64 {
    let mut hash = Sha256::new();
    hash.update(b"orch-secrets-pg/lock/v1");
    for text in [owner, server] {
        hash.update(u64::try_from(text.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(text.as_bytes());
    }
    let digest = hash.finalize();
    i64::from_be_bytes(digest[..8].try_into().unwrap_or_default())
}

/// SQLSTATE `55P03`, `lock_not_available`: what a statement fails with when `lock_timeout` ends
/// its wait for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// Whether a SQLSTATE says "try again later" (the connection, resources, a shutdown).
fn transient_sqlstate(code: &str) -> bool {
    code.starts_with("08")
        || code.starts_with("53")
        || matches!(code, "40001" | "40P01" | "57P01" | "57P02" | "57P03")
}

/// Whether a driver error says the database is down or too busy to answer *now* (the network, TLS,
/// the pool, the connection worker, a lock that was not got in time, and the SQLSTATEs of
/// [`transient_sqlstate`]), as opposed to a statement it refused (a missing privilege, a syntax
/// error, a constraint): waiting cures the first and never the second.
fn transient(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::Protocol(_) => true,
        sqlx::Error::Database(db) => db
            .code()
            .is_some_and(|code| code == LOCK_NOT_AVAILABLE || transient_sqlstate(&code)),
        _ => false,
    }
}

/// A driver error, as the port says it: a row that does not decode is `Corrupt`; everything else
/// (the network, the pool, the server, a rejected statement) is `Unavailable`, with the driver's
/// error as the source. The port has no class for "a bug"; a statement that is rejected every
/// time shows as a store that is down, and the log has the cause. A driver error holds no
/// parameter of the statement, so no secret and no ciphertext. [`is_outage`] reads the source back
/// for the caller that must tell an outage from a refusal.
pub(crate) fn db_err(err: sqlx::Error) -> SecretError {
    if matches!(&err, sqlx::Error::Database(db) if db.code().as_deref() == Some(LOCK_NOT_AVAILABLE))
    {
        return SecretError::unavailable(
            "the secret is locked by another writer for longer than a write waits; try again",
        )
        .with_source(err);
    }
    if matches!(
        &err,
        sqlx::Error::Decode(_) | sqlx::Error::ColumnDecode { .. } | sqlx::Error::ColumnNotFound(_)
    ) {
        return SecretError::corrupt("a row of the secret store does not decode");
    }
    let detail = if transient(&err) {
        "the database cannot be reached"
    } else {
        "the database refused the request"
    };
    SecretError::unavailable(detail).with_source(err)
}

/// Whether `error` is the database being down or too busy to answer (the connection, the pool, a
/// restart, a lock that was held too long), and so a failure that waiting cures; `false` for
/// anything else: a statement the database refused (a missing privilege at `migrate`, a syntax
/// error), a migration history that is not this build's, a key this deployment does not hold, a
/// row that is not what was written.
///
/// [`SecretError::Unavailable`] does not tell them apart, because the port has no class for "a
/// bug" and a statement that is refused every time shows as a store that is down; its `Transient`
/// class is true of both. A caller that decides what to do from the class alone (retry, or exit
/// as for an outage) would wait for ever on the second, so it asks here, where the driver's error
/// the store kept as the source is understood. An `Unavailable` with no source says only that the
/// store could not be reached (an adapter that knows nothing more), and is an outage.
#[must_use]
pub fn is_outage(error: &SecretError) -> bool {
    let SecretError::Unavailable { source, .. } = error else {
        return false;
    };
    let Some(source) = source.as_deref() else {
        return true;
    };
    if let Some(driver) = source.downcast_ref::<sqlx::Error>() {
        return transient(driver);
    }
    // a migration that was not run for a reason of its own (the source, the driver's support) is no
    // outage; whatever else a source may be, the store could not be reached
    !source.is::<sqlx::migrate::MigrateError>()
}

fn migrate_err(err: sqlx::migrate::MigrateError) -> SecretError {
    use sqlx::migrate::MigrateError as M;
    match err {
        M::Execute(e) | M::ExecuteMigration(e, _) => db_err(e),
        e @ (M::VersionMissing(_)
        | M::VersionMismatch(_)
        | M::VersionNotPresent(_)
        | M::VersionTooOld(..)
        | M::VersionTooNew(..)
        | M::Dirty(_)) => SecretError::corrupt(
            "the applied migrations of the secret store do not match this build",
        )
        .with_source(e),
        other => {
            SecretError::unavailable("the migrations of the secret store failed").with_source(other)
        }
    }
}

pub(crate) fn open_err(err: OpenError) -> SecretError {
    match err {
        OpenError::KeyUnknown => SecretError::KeyUnknown,
        OpenError::Corrupt(detail) => SecretError::corrupt(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_of_a_key_depends_on_both_texts_and_where_one_ends() {
        assert_eq!(
            lock_key("a@b.c", "u-0123456789"),
            lock_key("a@b.c", "u-0123456789")
        );
        assert_ne!(
            lock_key("a@b.c", "u-0123456789"),
            lock_key("a@b.c", "u-0123456789#flow")
        );
        assert_ne!(
            lock_key("a@b.c", "u-0123456789"),
            lock_key("a@b.d", "u-0123456789")
        );
        assert_ne!(lock_key("ab", "c"), lock_key("a", "bc"));
    }

    #[test]
    fn driver_errors_are_unavailable_unless_a_row_does_not_decode() {
        use orch_core::{Classify as _, ErrorClass};
        let down = db_err(sqlx::Error::PoolClosed);
        assert!(matches!(down, SecretError::Unavailable { .. }));
        assert_eq!(down.class(), ErrorClass::Transient);
        let refused = db_err(sqlx::Error::RowNotFound);
        assert!(matches!(refused, SecretError::Unavailable { .. }));
        let bad = db_err(sqlx::Error::ColumnNotFound("v".to_owned()));
        assert!(matches!(bad, SecretError::Corrupt(_)));
    }

    /// A server error carrying only a SQLSTATE.
    #[derive(Debug)]
    struct Sqlstate(&'static str);

    impl std::fmt::Display for Sqlstate {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "server error {}", self.0)
        }
    }

    impl std::error::Error for Sqlstate {}

    impl sqlx::error::DatabaseError for Sqlstate {
        fn message(&self) -> &str {
            "server error"
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(std::borrow::Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    fn db(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(Sqlstate(code)))
    }

    /// The class of a failure is `Transient` for every `Unavailable`; what tells a database that is
    /// down from one that refused a statement is the driver's error kept as the source.
    #[test]
    fn only_a_database_that_cannot_answer_is_an_outage() {
        use orch_core::{Classify as _, ErrorClass};
        use sqlx::migrate::MigrateError as M;
        let boxed = |msg: &str| -> Box<dyn std::error::Error + Send + Sync> { msg.into() };
        let outages = [
            (
                "the network",
                db_err(sqlx::Error::Io(std::io::Error::other("reset"))),
            ),
            ("tls", db_err(sqlx::Error::Tls(boxed("handshake")))),
            ("the pool is empty", db_err(sqlx::Error::PoolTimedOut)),
            ("the pool is closed", db_err(sqlx::Error::PoolClosed)),
            ("the worker crashed", db_err(sqlx::Error::WorkerCrashed)),
            (
                "a bad frame",
                db_err(sqlx::Error::Protocol("bad frame".into())),
            ),
            ("connection exception", db_err(db("08006"))),
            ("too many connections", db_err(db("53300"))),
            ("serialization failure", db_err(db("40001"))),
            ("deadlock", db_err(db("40P01"))),
            ("admin shutdown", db_err(db("57P01"))),
            ("cannot connect now", db_err(db("57P03"))),
            ("a lock not got in time", db_err(db(LOCK_NOT_AVAILABLE))),
            (
                "a migration that could not reach it",
                migrate_err(M::Execute(sqlx::Error::PoolTimedOut)),
            ),
            (
                "a store that knows nothing more",
                SecretError::unavailable("down"),
            ),
        ];
        for (what, error) in outages {
            assert!(is_outage(&error), "{what}: {error:?}");
        }
        let refusals = [
            ("a missing privilege", db_err(db("42501"))),
            ("a syntax error", db_err(db("42601"))),
            ("a table that is not there", db_err(db("42P01"))),
            ("a read-only transaction", db_err(db("25006"))),
            ("a constraint", db_err(db("23514"))),
            ("a row that is not there", db_err(sqlx::Error::RowNotFound)),
            (
                "a bad configuration",
                db_err(sqlx::Error::Configuration(boxed("bad url"))),
            ),
            (
                "a missing privilege during migrate",
                migrate_err(M::ExecuteMigration(db("42501"), 1)),
            ),
            (
                "a migration source",
                migrate_err(M::Source(boxed("no such directory"))),
            ),
        ];
        for (what, error) in refusals {
            // the class is the port's and does not say it: both are `Unavailable`, both `Transient`
            assert!(
                matches!(error, SecretError::Unavailable { .. }),
                "{what}: {error:?}"
            );
            assert_eq!(error.class(), ErrorClass::Transient, "{what}");
            assert!(!is_outage(&error), "{what}: {error:?}");
        }
        // the other variants are never an outage
        for error in [
            SecretError::Conflict,
            SecretError::KeyUnknown,
            SecretError::corrupt("a row"),
            migrate_err(M::VersionMismatch(1)),
            migrate_err(M::Dirty(1)),
            db_err(sqlx::Error::ColumnNotFound("v".to_owned())),
        ] {
            assert!(!is_outage(&error), "{error:?}");
        }
    }
}
