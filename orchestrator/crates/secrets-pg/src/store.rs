//! `PgSecretStore`: the [`SecretStore`] of the orchestrator's own database.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deadpool_postgres::{HookError, Object, Pool, PoolError, Transaction};
use orch_core::UserId;
use orch_pg_connect::{ConnectError, PgOptions};
use orch_pg_migrate::{MigrateError, Migrator};
use orch_ports::{SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion};
use sha2::{Digest as _, Sha256};
use tokio_postgres::IsolationLevel;
use tokio_postgres::types::FromSql;

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

/// How long [`PgSecretStore::migrate_direct`] tries to connect: thirty seconds, the acquire timeout
/// of the pool of one that it made on sqlx.
const DIRECT_TIMEOUT: Duration = Duration::from_secs(30);

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
#[derive(Clone)]
pub struct PgSecretStore {
    pub(crate) pool: Pool,
    pub(crate) keys: Arc<Keyring>,
}

impl std::fmt::Debug for PgSecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgSecretStore")
            .field("pool", &self.pool.status())
            .field("keys", &self.keys)
            .finish()
    }
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
        let max = usize::try_from(max_connections.max(1)).unwrap_or(usize::MAX);
        let pool = parse_url(url)?
            .pool(max, ACQUIRE_TIMEOUT)
            .map_err(connect_err)?;
        let store = Self::from_pool(pool, keys);
        // One connection now, as sqlx's `connect_with` made one: a database that cannot be reached
        // fails here, not at the first call. It stays in the pool.
        drop(store.conn().await?);
        Ok(store)
    }

    /// Wraps an existing pool (the orchestrator's own, or one that pins a `search_path` in a test).
    pub fn from_pool(pool: Pool, keys: Keyring) -> Self {
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
    /// call from every replica at boot: the migrator takes a database-wide advisory lock while it
    /// runs (sqlx's, `orch-pg-migrate`).
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] if the database cannot be reached or rejects a statement;
    /// [`SecretError::Corrupt`] if the history does not match the migrations of this build.
    pub async fn migrate(&self) -> Result<(), SecretError> {
        let migrator = migrator()?;
        // The run's connection leaves the pool and is closed when this ends, on a success, an error
        // or a cancelled future alike: its session lock, and the settings the migrator gave the
        // session, never go back to the pool with it. The pool makes another when a call needs one.
        let mut client = Object::take(self.conn().await?);
        migrator.run(&mut client).await.map_err(migrate_err)
    }

    /// [`migrate`](Self::migrate) on one connection of its own to `url`, closed when it is done.
    ///
    /// For a deployment whose pool goes through a transaction-mode pooler (PgBouncer): the migrator's
    /// advisory lock is a session lock held across statements, which such a pooler cannot keep.
    /// `url` is the database itself. Same effect and errors as [`migrate`](Self::migrate); it touches
    /// no pool.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] if the database cannot be reached or rejects a statement, or `url`
    /// is not a connection string (the error never holds the text, it may carry a password);
    /// [`SecretError::Corrupt`] if the history does not match the migrations of this build.
    pub async fn migrate_direct(url: &str) -> Result<(), SecretError> {
        let options = parse_url(url)?;
        let migrator = migrator()?;
        let mut client = retrying(
            DIRECT_TIMEOUT,
            || options.connect(),
            |e| matches!(e, ConnectError::Database(e) if refused_while_starting(e)),
        )
        .await
        .map_err(|e| e.into_secret_error(connect_err))?;
        let outcome = migrator.run(&mut client).await.map_err(migrate_err);
        // The lock goes with the session whether or not this succeeded.
        drop(client);
        outcome
    }

    /// Closes the pool: every later call fails as [`SecretError::Unavailable`], as a database that
    /// cannot be reached does. For a clean shutdown, and for the conformance testkit's breaker.
    pub async fn close(&self) {
        self.pool.close();
    }

    /// A connection of the pool. A database that refuses connections because it is starting (the
    /// connection refused, SQLSTATE `53300` or `57P03`) is tried again until the pool's wait is
    /// over, as sqlx's pool did.
    pub(crate) async fn conn(&self) -> Result<Object, SecretError> {
        let wait = self.pool.timeouts().wait.unwrap_or(ACQUIRE_TIMEOUT);
        retrying(
            wait,
            || self.pool.get(),
            |e| matches!(e, PoolError::Backend(e) if refused_while_starting(e)),
        )
        .await
        .map_err(|e| e.into_secret_error(pool_err))
    }

    /// Begins a transaction on `conn` that holds the key's lock until it ends. Every write of one
    /// key runs in one of these, so the writers of a key are serialized and take their versions
    /// from the sequence in the order they land. The transaction borrows the connection, so one
    /// that is dropped is rolled back before the connection serves anybody else.
    ///
    /// The transaction is `READ COMMITTED` whatever the database or the role defaults to: the
    /// statements after the lock must see what the writer ahead of it committed, and under
    /// `REPEATABLE READ` the snapshot is taken at the lock statement, before the wait, so the
    /// writer that waited would fail with a serialization error (SQLSTATE `40001`). The wait for
    /// the lock is bounded by [`LOCK_TIMEOUT`]; a wait that ends is `Unavailable`, with the
    /// connection given back at once.
    pub(crate) async fn locked<'a>(
        &self,
        conn: &'a mut Object,
        owner: &str,
        server: &str,
    ) -> Result<Transaction<'a>, SecretError> {
        let tx = conn
            .build_transaction()
            .isolation_level(IsolationLevel::ReadCommitted)
            .start()
            .await
            .map_err(db_err)?;
        let taken = async {
            tx.batch_execute(&format!(
                "SET LOCAL lock_timeout = '{}ms'",
                LOCK_TIMEOUT.as_millis()
            ))
            .await?;
            let lock = tx
                .prepare_cached("SELECT pg_advisory_xact_lock($1)")
                .await?;
            tx.execute(&lock, &[&lock_key(owner, server)]).await
        }
        .await;
        if let Err(err) = taken {
            // do not leave the connection to the pool with an aborted transaction
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

/// The column `index` of a row, as `T`.
///
/// # Errors
/// [`SecretError::Corrupt`] when it is not there or not a `T`.
pub(crate) fn column<'a, T: FromSql<'a>>(
    row: &'a tokio_postgres::Row,
    index: usize,
) -> Result<T, SecretError> {
    row.try_get(index)
        .map_err(|_| SecretError::corrupt("a row of the secret store does not decode"))
}

/// The [`COLUMNS`] of a row, starting at column `at`.
pub(crate) fn row_at(row: &tokio_postgres::Row, at: usize) -> Result<Row, SecretError> {
    Ok((
        column(row, at)?,
        column(row, at + 1)?,
        column(row, at + 2)?,
        column(row, at + 3)?,
        column(row, at + 4)?,
    ))
}

/// The next version of the store's sequence. Called with the key's lock held.
async fn next_version(tx: &Transaction<'_>) -> Result<u64, SecretError> {
    let next = tx
        .prepare_cached("SELECT nextval('user_secret_versions')")
        .await
        .map_err(db_err)?;
    let row = tx.query_one(&next, &[]).await.map_err(db_err)?;
    let version: i64 = column(&row, 0)?;
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
        let mut conn = self.conn().await?;
        let tx = self.locked(&mut conn, owner, &server).await?;
        let version = next_version(&tx).await?;
        let sealed = self.seal(owner, &server, version, material.expose())?;
        let insert = tx
            .prepare_cached(
                "INSERT INTO user_secrets (owner, server, version, v, kid, nonce, ciphertext) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (owner, server) DO UPDATE SET \
                   version = EXCLUDED.version, v = EXCLUDED.v, kid = EXCLUDED.kid, \
                   nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, \
                   updated = clock_timestamp()",
            )
            .await
            .map_err(db_err)?;
        tx.execute(
            &insert,
            &[
                &owner,
                &server,
                &to_i64(version)?,
                &sealed.v,
                &sealed.kid,
                &sealed.nonce.as_slice(),
                &sealed.ciphertext,
            ],
        )
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
        let conn = self.conn().await?;
        let select = conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM user_secrets WHERE owner = $1 AND server = $2"
            ))
            .await
            .map_err(db_err)?;
        let row = conn
            .query_opt(&select, &[&owner, &server])
            .await
            .map_err(db_err)?;
        drop(conn);
        let Some(row) = row else { return Ok(None) };
        let stored = Stored::decode(row_at(&row, 0)?)?;
        let material = self.open(owner, &server, &stored)?;
        Ok(Some((SecretVersion::new(stored.version), material)))
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        let (owner, server) = (owner.as_str(), server.to_string());
        let mut conn = self.conn().await?;
        let tx = self.locked(&mut conn, owner, &server).await?;
        let delete = tx
            .prepare_cached("DELETE FROM user_secrets WHERE owner = $1 AND server = $2")
            .await
            .map_err(db_err)?;
        tx.execute(&delete, &[&owner, &server])
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
        let mut conn = self.conn().await?;
        let tx = self.locked(&mut conn, owner, &server).await?;
        let select = tx
            .prepare_cached("SELECT version FROM user_secrets WHERE owner = $1 AND server = $2")
            .await
            .map_err(db_err)?;
        let held = tx
            .query_opt(&select, &[&owner, &server])
            .await
            .map_err(db_err)?;
        let held: Option<i64> = held.map(|row| column(&row, 0)).transpose()?;
        if held != Some(expected) {
            // nothing is written, and the transaction ends with the lock released
            return Err(SecretError::Conflict);
        }
        let version = next_version(&tx).await?;
        let sealed = self.seal(owner, &server, version, material.expose())?;
        let update = tx
            .prepare_cached(
                "UPDATE user_secrets SET version = $3, v = $4, kid = $5, nonce = $6, ciphertext = $7, \
                   updated = clock_timestamp() \
                 WHERE owner = $1 AND server = $2 AND version = $8",
            )
            .await
            .map_err(db_err)?;
        let done = tx
            .execute(
                &update,
                &[
                    &owner,
                    &server,
                    &to_i64(version)?,
                    &sealed.v,
                    &sealed.kid,
                    &sealed.nonce.as_slice(),
                    &sealed.ciphertext,
                    &expected,
                ],
            )
            .await
            .map_err(db_err)?;
        if done != 1 {
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

/// This crate's migrations, in their own history table.
fn migrator() -> Result<Migrator, SecretError> {
    let migrations =
        orch_pg_migrate::migrations!("migrations": "0001_user_secrets.sql").map_err(migrate_err)?;
    Migrator::new(MIGRATIONS_TABLE, migrations).map_err(migrate_err)
}

/// A connection string as options, for every connection this crate opens by itself. The error never
/// holds the text: it may carry a password. `tokio-postgres` sends no `extra_float_digits`, which
/// PgBouncer refuses as a startup parameter.
fn parse_url(url: &str) -> Result<PgOptions, SecretError> {
    url.parse::<PgOptions>()
        .map_err(|_| SecretError::unavailable("the connection string cannot be parsed"))
}

/// How [`retrying`] ended without a value.
enum Retried<E> {
    /// The attempt failed in a way that is not tried again, or the time was up after it.
    Failed(E),
    /// An attempt was still under way when the time was up.
    TimedOut,
}

impl<E> Retried<E> {
    fn into_secret_error(self, err: impl FnOnce(E) -> SecretError) -> SecretError {
        match self {
            Retried::Failed(e) => err(e),
            Retried::TimedOut => failure(
                true,
                "no connection to the database was made in time".to_owned(),
            ),
        }
    }
}

/// `attempt` until it succeeds, fails in a way `again` does not try again, or `wait` is over:
/// sqlx's backoff while a database starts (10 ms, doubled, at most a fifth of the wait). A wait too
/// long to add to the clock (`Duration::MAX`) is no deadline: the backoff then stops at a second.
async fn retrying<T, E, F>(
    wait: Duration,
    mut attempt: impl FnMut() -> F,
    again: impl Fn(&E) -> bool,
) -> Result<T, Retried<E>>
where
    F: Future<Output = Result<T, E>>,
{
    let deadline = Instant::now().checked_add(wait);
    let mut backoff = Duration::from_millis(10);
    let max_backoff = match deadline {
        Some(_) => wait / 5,
        None => Duration::from_secs(1),
    };
    loop {
        let outcome = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(left, attempt()).await {
                    Ok(outcome) => outcome,
                    Err(_) => return Err(Retried::TimedOut),
                }
            }
            None => attempt().await,
        };
        let in_time = |deadline: Instant| {
            Instant::now()
                .checked_add(backoff)
                .is_some_and(|next| next < deadline)
        };
        match outcome {
            Ok(value) => return Ok(value),
            Err(e) if again(&e) && deadline.is_none_or(in_time) => {}
            Err(e) => return Err(Retried::Failed(e)),
        }
        tokio::time::sleep(backoff).await;
        backoff = backoff.saturating_mul(2).min(max_backoff);
    }
}

/// Whether a connection was refused in a way that a database that is starting refuses one, which
/// sqlx's pool tried again: the TCP connection refused, too many connections, cannot connect now.
fn refused_while_starting(err: &tokio_postgres::Error) -> bool {
    if let Some(db) = err.as_db_error() {
        return retried_sqlstate(db.code().code());
    }
    std::error::Error::source(err)
        .and_then(|s| s.downcast_ref::<std::io::Error>())
        .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// The SQLSTATEs at connection that sqlx's pool tried again: `53300` `too_many_connections` and
/// `57P03` `cannot_connect_now`.
fn retried_sqlstate(code: &str) -> bool {
    matches!(code, "53300" | "57P03")
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

/// What `tokio-postgres` says of a failure that is not the server's (its `Display`, which is its
/// kind and nothing else) when the database is down or cannot be understood: the network, TLS, a
/// connection that closed (its connection task ended), a frame that does not parse, a connect that
/// took too long. Every other kind (a parameter or a column of the wrong type, a row count, the
/// configuration, and the two of [`DRIVER_REFUSALS`]) is a request this crate or its configuration
/// got wrong, which waiting does not cure.
const DRIVER_OUTAGES: [&str; 7] = [
    "error communicating with the server",
    "error performing TLS handshake",
    "connection closed",
    "unexpected message from server",
    "error parsing response from server",
    "error connecting to server",
    "timeout waiting for server",
];

/// The kinds of `tokio-postgres` that carry an I/O error and are still no outage, read before the
/// I/O rule: a message that cannot be encoded (a user, database or `application_name` of the
/// connection string that holds a NUL byte, refused every time), and the client's own refusal of
/// an authentication (a SASL mechanism or a server signature it does not accept).
const DRIVER_REFUSALS: [&str; 2] = ["error encoding message to server", "authentication error"];

/// The failure of the database as this crate keeps it, the source of an `Unavailable`: whether it
/// is an outage, and a text for whoever reads the log. The text is the server's message (never its
/// `DETAIL`, which may quote a row) or the driver's kind, so it holds no parameter of a statement,
/// no secret and no connection string; and it has no source of its own.
#[derive(Debug)]
struct DbFailure {
    outage: bool,
    text: String,
}

impl std::fmt::Display for DbFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for DbFailure {}

/// An `Unavailable` whose source says whether it is an outage.
fn failure(outage: bool, text: String) -> SecretError {
    let detail = if outage {
        "the database cannot be reached"
    } else {
        "the database refused the request"
    };
    SecretError::unavailable(detail).with_source(DbFailure { outage, text })
}

/// A server's error, by its SQLSTATE: a lock not got in time and the SQLSTATEs of
/// [`transient_sqlstate`] are outages, every other one a refusal.
fn sqlstate_failure(code: &str, text: String) -> SecretError {
    if code == LOCK_NOT_AVAILABLE {
        return SecretError::unavailable(
            "the secret is locked by another writer for longer than a write waits; try again",
        )
        .with_source(DbFailure { outage: true, text });
    }
    failure(transient_sqlstate(code), text)
}

/// A failure that is not the server's, by what the driver says of it (`kind`, its `Display`),
/// whether its connection closed, and its I/O error if it has one.
fn driver_failure(closed: bool, io: Option<&std::io::Error>, kind: String) -> SecretError {
    let outage = !DRIVER_REFUSALS.contains(&kind.as_str())
        && (closed || io.is_some() || DRIVER_OUTAGES.contains(&kind.as_str()));
    let text = match io {
        Some(io) => format!("{kind}: {io}"),
        None => kind,
    };
    failure(outage, text)
}

/// A driver error, as the port says it: everything (the network, the server, a rejected
/// statement) is `Unavailable`, with a [`DbFailure`] as the source. A row that does not decode is
/// `Corrupt`, at [`column`]. The port has no class for "a bug"; a statement that is rejected every
/// time shows as a store that is down, and the log has the cause. [`is_outage`] reads the source
/// back for the caller that must tell an outage from a refusal.
pub(crate) fn db_err(err: tokio_postgres::Error) -> SecretError {
    if let Some(db) = err.as_db_error() {
        let code = db.code().code();
        let text = format!("{}: {} (SQLSTATE {code})", db.severity(), db.message());
        return sqlstate_failure(code, text);
    }
    let io = std::error::Error::source(&err).and_then(|s| s.downcast_ref::<std::io::Error>());
    driver_failure(err.is_closed(), io, err.to_string())
}

/// A pool's error: no connection in time and a closed pool are outages, the backend's error is
/// read as the driver's, and anything else (no runtime, a hook) is the pool's configuration.
fn pool_err(err: PoolError) -> SecretError {
    match err {
        PoolError::Backend(e) | PoolError::PostCreateHook(HookError::Backend(e)) => db_err(e),
        PoolError::Timeout(kind) => failure(
            true,
            format!("no connection of the pool in time ({kind:?})"),
        ),
        PoolError::Closed => failure(true, "the pool is closed".to_owned()),
        PoolError::NoRuntimeSpecified | PoolError::PostCreateHook(HookError::Message(_)) => {
            failure(false, "the pool cannot make a connection".to_owned())
        }
    }
}

/// An error of `orch-pg-connect`: the server's as the driver's; a root certificate that cannot be
/// read and a TLS configuration that rustls refuses as sqlx's TLS errors were (outages); a pool
/// that cannot be built is its configuration.
fn connect_err(err: ConnectError) -> SecretError {
    match err {
        ConnectError::Database(e) => db_err(e),
        ConnectError::RootCert => failure(true, "sslrootcert cannot be read".to_owned()),
        ConnectError::Tls(_) => failure(true, "the TLS configuration is refused".to_owned()),
        ConnectError::Pool(_) => failure(false, "the pool cannot be built".to_owned()),
        // `ConnectError` is non_exhaustive: a variant added later is read as a refusal until it is
        // named here, and its text is not kept (it might hold a driver's error)
        _ => failure(
            false,
            "the connection failed in a way this build does not know".to_owned(),
        ),
    }
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
/// as for an outage) would wait for ever on the second, so it asks here, where the failure the
/// store kept as the source is understood. An `Unavailable` with no source says only that the
/// store could not be reached (an adapter that knows nothing more), and is an outage.
#[must_use]
pub fn is_outage(error: &SecretError) -> bool {
    let SecretError::Unavailable { source, .. } = error else {
        return false;
    };
    let Some(source) = source.as_deref() else {
        return true;
    };
    if let Some(failure) = source.downcast_ref::<DbFailure>() {
        return failure.outage;
    }
    // a migration that was not run for a reason of its own (its files, its table's name) is no
    // outage; whatever else a source may be, the store could not be reached
    !source.is::<MigrateError>()
}

fn migrate_err(err: MigrateError) -> SecretError {
    match err {
        MigrateError::Execute { source, .. } | MigrateError::Database(source) => db_err(source),
        e @ (MigrateError::VersionMissing(_)
        | MigrateError::VersionMismatch(_)
        | MigrateError::Dirty(_)) => SecretError::corrupt(
            "the applied migrations of the secret store do not match this build",
        )
        .with_source(e),
        // a crate's own mistake: the error holds a name or a number, never a driver's error
        e @ (MigrateError::FileName(_)
        | MigrateError::TableName(_)
        | MigrateError::DuplicateVersion(_)) => {
            SecretError::unavailable("the migrations of the secret store failed").with_source(e)
        }
        // `MigrateError` is non_exhaustive: a variant added later is a refusal until it is named
        // here, and is not kept as the source (it might hold a driver's error, and a DETAIL)
        _ => failure(
            false,
            "the migrations of the secret store failed in a way this build does not know"
                .to_owned(),
        ),
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
    use deadpool_postgres::TimeoutType;

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
    #[allow(clippy::unwrap_used)]
    fn the_migrations_listed_are_the_files_of_the_directory() {
        let migrator = migrator().unwrap();
        migrator
            .check_directory(std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/migrations"
            )))
            .unwrap();
        let first = &migrator.migrations()[0];
        assert_eq!((first.version(), first.description()), (1, "user secrets"));
    }

    /// The driver's own words for a failure that is not the server's.
    fn driver(text: &str) -> SecretError {
        driver_failure(false, None, text.to_owned())
    }

    fn db(code: &str) -> SecretError {
        sqlstate_failure(code, format!("ERROR: server error (SQLSTATE {code})"))
    }

    #[test]
    fn driver_errors_are_unavailable_unless_a_row_does_not_decode() {
        use orch_core::{Classify as _, ErrorClass};
        let down = pool_err(PoolError::Closed);
        assert!(matches!(down, SecretError::Unavailable { .. }));
        assert_eq!(down.class(), ErrorClass::Transient);
        let refused = driver("query returned an unexpected number of rows");
        assert!(matches!(refused, SecretError::Unavailable { .. }));
        // a row that does not decode is `Corrupt` at `column`: `tests/adapter.rs` decodes one
    }

    #[test]
    fn what_sqlx_tried_again_at_connection_is_tried_again() {
        for code in ["53300", "57P03"] {
            assert!(retried_sqlstate(code), "{code}");
        }
        for code in ["28P01", "3D000", "08006", "57P01", "42501"] {
            assert!(!retried_sqlstate(code), "{code}");
        }
    }

    /// The class of a failure is `Transient` for every `Unavailable`; what tells a database that is
    /// down from one that refused a statement is the failure kept as the source.
    #[test]
    fn only_a_database_that_cannot_answer_is_an_outage() {
        use orch_core::{Classify as _, ErrorClass};
        let reset = std::io::Error::other("reset");
        let outages = [
            (
                "the network",
                driver_failure(
                    false,
                    Some(&reset),
                    "error communicating with the server".to_owned(),
                ),
            ),
            ("tls", driver("error performing TLS handshake")),
            (
                "the pool is empty",
                pool_err(PoolError::Timeout(TimeoutType::Wait)),
            ),
            (
                "no connection made in time",
                pool_err(PoolError::Timeout(TimeoutType::Create)),
            ),
            ("the pool is closed", pool_err(PoolError::Closed)),
            (
                "the connection task ended",
                driver_failure(true, None, "connection closed".to_owned()),
            ),
            ("a bad frame", driver("error parsing response from server")),
            (
                "an unexpected frame",
                driver("unexpected message from server"),
            ),
            ("a refused connection", driver("error connecting to server")),
            ("a connect timeout", driver("timeout waiting for server")),
            ("connection exception", db("08006")),
            ("too many connections", db("53300")),
            ("serialization failure", db("40001")),
            ("deadlock", db("40P01")),
            ("admin shutdown", db("57P01")),
            ("cannot connect now", db("57P03")),
            ("a lock not got in time", db(LOCK_NOT_AVAILABLE)),
            ("a root certificate", connect_err(ConnectError::RootCert)),
            (
                "a connection that was not made in time",
                Retried::<PoolError>::TimedOut.into_secret_error(pool_err),
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
            ("a missing privilege", db("42501")),
            ("a syntax error", db("42601")),
            ("a table that is not there", db("42P01")),
            ("a read-only transaction", db("25006")),
            ("a constraint", db("23514")),
            (
                "a row that is not there",
                driver("query returned an unexpected number of rows"),
            ),
            ("a bad configuration", driver("invalid configuration")),
            (
                "a parameter of the wrong type",
                driver("error serializing parameter 0"),
            ),
            ("a column that is not there", driver("invalid column `v`")),
            (
                "a message that cannot be encoded",
                driver_failure(
                    false,
                    Some(&std::io::Error::other("string contains embedded null")),
                    "error encoding message to server".to_owned(),
                ),
            ),
            (
                "an authentication the client refuses",
                driver_failure(
                    false,
                    Some(&std::io::Error::other("invalid server signature")),
                    "authentication error".to_owned(),
                ),
            ),
            (
                "a pool without a runtime",
                pool_err(PoolError::NoRuntimeSpecified),
            ),
            (
                "a pool that cannot be built",
                connect_err(ConnectError::Pool("max_size".to_owned())),
            ),
            (
                "a migration source",
                migrate_err(MigrateError::FileName("0001.sql".to_owned())),
            ),
            (
                "a migration table",
                migrate_err(MigrateError::TableName("x y".to_owned())),
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
            migrate_err(MigrateError::VersionMismatch(1)),
            migrate_err(MigrateError::VersionMissing(2)),
            migrate_err(MigrateError::Dirty(1)),
        ] {
            assert!(!is_outage(&error), "{error:?}");
        }
        for error in [
            migrate_err(MigrateError::VersionMismatch(1)),
            migrate_err(MigrateError::VersionMissing(2)),
            migrate_err(MigrateError::Dirty(1)),
        ] {
            assert!(matches!(error, SecretError::Corrupt(_)), "{error:?}");
        }
    }

    /// The table above is the driver's words; these are the driver's own errors, made without a
    /// database: a refused connection is an outage, a connection string it cannot read is not.
    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn the_drivers_own_errors_are_read_as_the_table_says() {
        let options: PgOptions = "postgres://u:hunter2-pw@127.0.0.1:1/db?sslmode=disable"
            .parse()
            .unwrap();
        let ConnectError::Database(refused) = options.connect().await.unwrap_err() else {
            panic!("not the driver's error");
        };
        assert!(refused_while_starting(&refused));
        let error = db_err(refused);
        assert!(is_outage(&error), "{error:?}");
        assert!(
            !format!("{error} {error:?}").contains("hunter2"),
            "{error:?}"
        );

        let unreadable = "host=h port=notaport"
            .parse::<tokio_postgres::Config>()
            .unwrap_err();
        assert!(!refused_while_starting(&unreadable));
        let error = db_err(unreadable);
        assert!(matches!(error, SecretError::Unavailable { .. }));
        assert!(!is_outage(&error), "{error:?}");
    }

    /// A server on a port of its own: it reads what the client sends first, answers `reply`, and
    /// holds the socket open, so the client reads the answer and not the end of the stream.
    #[allow(clippy::unwrap_used)]
    async fn fake_server(reply: &'static [u8]) -> u16 {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut first = [0_u8; 1024];
                    let _ = socket.read(&mut first).await;
                    let _ = socket.write_all(reply).await;
                    tokio::time::sleep(Duration::from_secs(10)).await;
                });
            }
        });
        port
    }

    /// What the driver makes of a connection to `url`.
    #[allow(clippy::unwrap_used)]
    async fn driver_error_of(url: &str) -> tokio_postgres::Error {
        let options: PgOptions = url.parse().unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), options.connect())
            .await
            .unwrap();
        match outcome {
            Err(ConnectError::Database(e)) => e,
            Ok(_) => panic!("{url}: connected"),
            Err(other) => panic!("{url}: not the driver's error: {other:?}"),
        }
    }

    /// `sslmode=require` against a server that answers `N` to the SSLRequest: the driver's TLS
    /// error, which carries no I/O error, so only its words make it an outage.
    #[tokio::test]
    async fn a_server_that_refuses_tls_under_require_is_an_outage() {
        let port = fake_server(b"N").await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=require")).await;
        assert_eq!(e.to_string(), "error performing TLS handshake");
        let error = db_err(e);
        assert!(is_outage(&error), "{error:?}");
    }

    /// A ReadyForQuery (`Z`, length 5, idle) before any authentication: the driver's unexpected
    /// message, with no I/O error either.
    #[tokio::test]
    async fn a_server_that_answers_out_of_turn_is_an_outage() {
        let port = fake_server(b"Z\0\0\0\x05I").await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=disable")).await;
        assert_eq!(e.to_string(), "unexpected message from server");
        let error = db_err(e);
        assert!(is_outage(&error), "{error:?}");
    }

    /// A NUL byte in the user: the startup message cannot be encoded, every time. The driver's
    /// error carries an I/O error, and is still no outage (sqlx refused such a URL).
    #[tokio::test]
    async fn a_user_that_cannot_be_encoded_is_a_refusal() {
        let port = fake_server(b"").await;
        let e = driver_error_of(&format!(
            "postgres://a%00b@127.0.0.1:{port}/db?sslmode=disable"
        ))
        .await;
        assert_eq!(e.to_string(), "error encoding message to server");
        assert!(!refused_while_starting(&e), "not tried again");
        let error = db_err(e);
        assert!(matches!(error, SecretError::Unavailable { .. }));
        assert!(!is_outage(&error), "{error:?}");
    }

    #[test]
    fn a_wait_too_long_for_the_clock_has_no_deadline() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build();
        let Ok(runtime) = runtime else {
            panic!("no runtime");
        };
        let mut tries = 0;
        let outcome: Result<u32, Retried<u32>> = runtime.block_on(retrying(
            Duration::MAX,
            || {
                tries += 1;
                let n = tries;
                async move { if n < 3 { Err(n) } else { Ok(n) } }
            },
            |_| true,
        ));
        assert!(matches!(outcome, Ok(3)));
    }
}
