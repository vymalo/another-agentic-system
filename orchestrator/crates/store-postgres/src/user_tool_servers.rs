//! `UserToolServerStore` over Postgres: the definitions of people's own MCP servers (ADR 0046).

use std::time::Duration;

use orch_core::{ErrorClass, UserId};
use orch_ports::{
    CredentialRef, MAX_USER_TOOL_SERVERS, ServerRevision, StoredServer, UserAuth, UserToolServer,
    UserToolServerId, UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};
use sqlx::{PgPool, Postgres, Transaction};

use crate::error::{is_unique_violation, sql_class};

/// How long a write waits for a lock, the owner's first and then the row's, before it gives up as
/// [`UserToolServerStoreError::Unavailable`]: two seconds.
///
/// A write holds a pooled connection from the moment it asks for a lock, so without a bound one lock
/// that is held for long (a replica that is paused, an operator's repair that has not committed)
/// would keep every writer of that owner on a connection after its caller has given up, until the
/// pool is empty and the thread store, which shares the pool, fails for want of one. A write holds a
/// lock for milliseconds. It is `SET LOCAL lock_timeout` in the write's own transaction, so it bounds
/// a wait for the owner's lock and for a row that somebody's hand-made transaction holds alike.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// The `classid` of the owner's advisory lock: the first of the two `int4` arguments of
/// `pg_advisory_xact_lock(int4, int4)`, the second being `hashtext(owner)`.
///
/// **A key space of its own.** Postgres keeps the one-`bigint` and the two-`int4` forms apart (the
/// lock tag differs in its last field, `objsubid` 1 and 2 in `pg_locks`), so this lock can never be
/// the same lock as one taken with a `bigint` key. Every other advisory lock the orchestrator or
/// adam-rs takes in this database is one-`bigint`: the watch keys of the thread store
/// (`hashtextextended('orch:watch:…', 0)`), the secret store's (`orch-secrets-pg`, 64 bits of a
/// SHA-256), the migrators' (sqlx, `pg_advisory_lock($1)`) and adam-rs's, which shares the database
/// under the feature `agent-local` and takes `pg_advisory_xact_lock(hashtext($1))` to serialise its
/// migrations (`crates/adam-store-postgres/src/lib.rs` at the revision the workspace pins: an `int4`
/// widened to `bigint`, so `objsubid` 1). Among the two-`int4` locks nothing else of either takes
/// one; this number, `0x7573_7276` (`usrv`), is the class of this store's, and a hash collision
/// between two owners only makes them take turns.
///
/// The hash is the server's (`hashtext`), not the process's: every replica of every build agrees on
/// the key for an owner, which a hasher of the standard library would not promise across Rust
/// versions during a rolling update.
pub const OWNER_LOCK_CLASS: i32 = 0x7573_7276;

/// What a row holds, in the order of [`COLUMNS`].
type Row = (
    String,
    String,
    String,
    serde_json::Value,
    String,
    Option<String>,
    i64,
);

/// The columns of a row, in the order of [`Row`].
const COLUMNS: &str = "id, name, url, auth, state, credential, revision";

/// [`UserToolServerStore`] in the orchestrator's own Postgres: one row per `(owner, id)` in
/// `user_tool_servers`, created by [`PgStore::migrate`](crate::PgStore::migrate) (migration `0021`,
/// one history for the whole schema).
///
/// Cheap to clone: clones share the pool. It holds no secret (a definition has none).
///
/// - **The limit of eight is kept under a lock of the owner**, taken before the count: every
///   [`insert`](UserToolServerStore::insert) runs in a transaction that takes
///   `pg_advisory_xact_lock(OWNER_LOCK_CLASS, hashtext(owner))` first, then looks whether the id is
///   taken, then counts, then inserts, so two writers racing for the last place take turns and the
///   second counts the first. Nothing else takes the lock: a delete only lowers the count and an
///   edit does not change it.
/// - **A revision is a number of one sequence** (`user_tool_server_revisions`), drawn by the
///   statement that writes it. [`replace`](UserToolServerStore::replace) and
///   [`set_state`](UserToolServerStore::set_state) are each one `UPDATE … WHERE owner = $1 AND id =
///   $2 AND revision = $expected`: the compare and the swap are one statement, and of writers that
///   hold the same revision one wins and the others find it changed (they are told `Conflict`, or
///   `NotFound` when the row is gone). A number is never given twice, a delete and an insert of the
///   same id included, and a failed write leaves a gap and never a repeat.
/// - **Every write is its own `READ COMMITTED` transaction**, whatever the database or the role
///   defaults to, with [`LOCK_TIMEOUT`]. Under `REPEATABLE READ` a write that waited for the row
///   another writer changed would fail with SQLSTATE `40001` instead of finding the revision moved.
/// - **A row is read back through [`UserToolServer::from_stored`]**: a row that is not a definition
///   (a URL that is not the normal form of one the guard reads, a kind that does not parse, a state
///   or a reference that is none) is [`Corrupt`](UserToolServerStoreError::Corrupt), and the message
///   holds neither the owner nor the URL. The deployment's lists are **not** applied: a person must
///   still be able to list a server that today's lists refuse, and delete it.
#[derive(Clone, Debug)]
pub struct PgUserToolServers {
    pool: PgPool,
}

impl PgUserToolServers {
    /// The store over `pool` (the orchestrator's own, shared with the thread store, or one that pins
    /// a `search_path` in a test). The table is created by [`PgStore::migrate`](crate::PgStore::migrate);
    /// this does not migrate.
    pub fn from_pool(pool: PgPool) -> Self {
        PgUserToolServers { pool }
    }

    /// The underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// How many definitions, of every owner, hold a credential (their `credential` is not null): one
    /// query, and the number only, so that nothing of an owner or an id leaves the database.
    ///
    /// For the composition root, which in a deployment that keeps no credentials (`userToolServers`
    /// enabled, no `secrets` section) says at startup how many servers cannot be used until the
    /// section is back. It is not a method of [`UserToolServerStore`]: no caller of the port needs it.
    ///
    /// # Errors
    /// [`UserToolServerStoreError::Unavailable`] when the database cannot be reached or refuses the
    /// query.
    pub async fn count_with_credential(&self) -> Result<u64, UserToolServerStoreError> {
        let held: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_tool_servers WHERE credential IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        // a count is never negative; a number that is not a `u64` is a database that is wrong
        u64::try_from(held).map_err(|_| {
            UserToolServerStoreError::unavailable("the database gave a count out of range")
        })
    }

    /// Begins a write: `READ COMMITTED`, with the wait for a lock bounded by [`LOCK_TIMEOUT`].
    async fn begin(&self) -> Result<Transaction<'static, Postgres>, UserToolServerStoreError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await
            .map_err(db_err)?;
        let bounded = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET LOCAL lock_timeout = '{}ms'",
            LOCK_TIMEOUT.as_millis()
        )))
        .execute(&mut *tx)
        .await;
        if let Err(err) = bounded {
            // do not leave the connection to the pool's drop path with an aborted transaction
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        Ok(tx)
    }

    /// Begins a write that holds the owner's lock until it ends.
    async fn begin_locked(
        &self,
        owner: &UserId,
    ) -> Result<Transaction<'static, Postgres>, UserToolServerStoreError> {
        let mut tx = self.begin().await?;
        let taken = sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(OWNER_LOCK_CLASS)
            .bind(owner.as_str())
            .execute(&mut *tx)
            .await;
        if let Err(err) = taken {
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        Ok(tx)
    }
}

impl UserToolServerStore for PgUserToolServers {
    async fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        let auth = auth_json(server.auth())?;
        let mut tx = self.begin_locked(owner).await?;
        // The id first, so that at the limit a definition that is there already is "exists", and
        // not "too many".
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_tool_servers WHERE owner = $1 AND id = $2)",
        )
        .bind(owner.as_str())
        .bind(server.id().as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if exists {
            abandon(tx).await;
            return Err(UserToolServerStoreError::Exists);
        }
        // The count is taken with the owner's lock held: no other insert of this owner is between
        // this count and this insert.
        let held: i64 =
            sqlx::query_scalar("SELECT count(*) FROM user_tool_servers WHERE owner = $1")
                .bind(owner.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
        if !usize::try_from(held).is_ok_and(|held| held < MAX_USER_TOOL_SERVERS) {
            abandon(tx).await;
            return Err(UserToolServerStoreError::TooMany {
                limit: MAX_USER_TOOL_SERVERS,
            });
        }
        let inserted: Result<i64, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, credential, revision) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, nextval('user_tool_server_revisions')) \
             RETURNING revision",
        )
        .bind(owner.as_str())
        .bind(server.id().as_str())
        .bind(server.name())
        .bind(server.url())
        .bind(auth)
        .bind(server.state().as_str())
        .bind(server.credential().map(CredentialRef::as_str))
        .fetch_one(&mut *tx)
        .await;
        let revision = match inserted {
            Ok(revision) => revision,
            // a writer that did not take the lock (a repair by hand) got there first
            Err(err) if is_unique_violation(&err, None) => {
                abandon(tx).await;
                return Err(UserToolServerStoreError::Exists);
            }
            Err(err) => return Err(db_err(err)),
        };
        tx.commit().await.map_err(db_err)?;
        revision_of(revision)
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM user_tool_servers WHERE owner = $1 AND id = $2"
        )))
        .bind(owner.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(stored).transpose()
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM user_tool_servers WHERE owner = $1 ORDER BY id"
        )))
        .bind(owner.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        // every row is read, so that every row that does not read back is warned of (the listing
        // fails as a whole for the first, and an operator looks for all of them in the log)
        let mut servers = Vec::with_capacity(rows.len());
        let mut unreadable = None;
        for row in rows {
            match stored(row) {
                Ok(server) => servers.push(server),
                Err(error) => {
                    unreadable.get_or_insert(error);
                }
            }
        }
        unreadable.map_or(Ok(servers), Err)
    }

    async fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        let auth = auth_json(server.auth())?;
        let mut tx = self.begin().await?;
        let written: Option<i64> = sqlx::query_scalar(
            "UPDATE user_tool_servers SET name = $3, url = $4, auth = $5, state = $6, \
               credential = $7, revision = nextval('user_tool_server_revisions'), \
               updated_at = clock_timestamp() \
             WHERE owner = $1 AND id = $2 AND revision = $8 \
             RETURNING revision",
        )
        .bind(owner.as_str())
        .bind(server.id().as_str())
        .bind(server.name())
        .bind(server.url())
        .bind(auth)
        .bind(server.state().as_str())
        .bind(server.credential().map(CredentialRef::as_str))
        .bind(stored_revision(expected))
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        finish(tx, written, owner, server.id()).await
    }

    async fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        let mut tx = self.begin().await?;
        let written: Option<i64> = sqlx::query_scalar(
            "UPDATE user_tool_servers SET state = $3, \
               revision = nextval('user_tool_server_revisions'), updated_at = clock_timestamp() \
             WHERE owner = $1 AND id = $2 AND revision = $4 \
             RETURNING revision",
        )
        .bind(owner.as_str())
        .bind(id.as_str())
        .bind(state.as_str())
        .bind(stored_revision(expected))
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        finish(tx, written, owner, id).await
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        let mut tx = self.begin().await?;
        sqlx::query("DELETE FROM user_tool_servers WHERE owner = $1 AND id = $2")
            .bind(owner.as_str())
            .bind(id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }
}

/// Ends a transaction that wrote nothing, which releases the locks it holds at once.
async fn abandon(tx: Transaction<'static, Postgres>) {
    // a failed rollback is a connection that is gone, and the server has released the locks then
    let _ = tx.rollback().await;
}

/// Ends a write that was one `UPDATE … WHERE … AND revision = $expected`: the new revision when a
/// row matched; otherwise the definition is either gone (`NotFound`: nothing is brought back) or at
/// another revision (`Conflict`), which one more read of the key, in the same transaction, tells
/// apart. Nothing was written when none matched.
async fn finish(
    mut tx: Transaction<'static, Postgres>,
    written: Option<i64>,
    owner: &UserId,
    id: &UserToolServerId,
) -> Result<ServerRevision, UserToolServerStoreError> {
    if let Some(revision) = written {
        tx.commit().await.map_err(db_err)?;
        return revision_of(revision);
    }
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_tool_servers WHERE owner = $1 AND id = $2)",
    )
    .bind(owner.as_str())
    .bind(id.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    // nothing was written: end the transaction, which releases whatever it took
    abandon(tx).await;
    Err(if present {
        UserToolServerStoreError::Conflict
    } else {
        UserToolServerStoreError::NotFound
    })
}

/// A revision as the column holds it. One that does not fit (the port's number is a `u64`, the
/// column a `bigint`) is not any revision the store gave: `-1`, which the column's check forbids, so
/// the write matches nothing and is a `Conflict`.
fn stored_revision(revision: ServerRevision) -> i64 {
    i64::try_from(revision.get()).unwrap_or(-1)
}

fn revision_of(revision: i64) -> Result<ServerRevision, UserToolServerStoreError> {
    u64::try_from(revision)
        .map(ServerRevision::new)
        .map_err(|_| {
            UserToolServerStoreError::unavailable(
                "the revision sequence gave a number out of range",
            )
        })
}

/// The JSON a kind is stored as: what its `Serialize` writes, so a variant added to the closed enum
/// is a variant the column can hold.
fn auth_json(auth: &UserAuth) -> Result<serde_json::Value, UserToolServerStoreError> {
    serde_json::to_value(auth).map_err(|_| {
        UserToolServerStoreError::unavailable("the kind of authentication cannot be stored")
    })
}

/// A row, read back through [`UserToolServer::from_stored`]: whatever is wrong is `Corrupt`, with a
/// sentence that holds no owner, URL or reference. Each such row is also **warned of once, by its
/// id**: the port's error names no id and the person is told to ask the operator, who finds the id
/// here.
fn stored(row: Row) -> Result<StoredServer, UserToolServerStoreError> {
    let id = row.0.clone();
    read_back(row).inspect_err(|error| warn_unreadable(&id, error))
}

/// The warning for a row that does not read back. It names the id (`u-` and ten characters of
/// `[a-z0-9]`, which the table checks and which holds nothing of the person) and the sentence of the
/// error, which holds no owner, URL or reference: never the owner, the name or the URL. An id that is
/// not one (the table's check dropped by a repair by hand) is not copied: nothing says what it holds.
fn warn_unreadable(id: &str, error: &UserToolServerStoreError) {
    match UserToolServerId::parse(id) {
        Ok(id) => tracing::warn!(
            %id,
            %error,
            "a definition of a person's own MCP server does not read back; its owner can still \
             delete it by this id"
        ),
        Err(_) => tracing::warn!(
            %error,
            "a definition of a person's own MCP server, whose id is not an id, does not read back"
        ),
    }
}

fn read_back(
    (id, name, url, auth, state, credential, revision): Row,
) -> Result<StoredServer, UserToolServerStoreError> {
    let corrupt = |what: &str| {
        UserToolServerStoreError::Corrupt(format!("a stored definition does not read back: {what}"))
    };
    let id = UserToolServerId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let auth: UserAuth = serde_json::from_value(auth)
        .map_err(|_| corrupt("the kind of authentication is not one"))?;
    let state = state
        .parse::<UserToolServerState>()
        .map_err(|e| corrupt(&e.to_string()))?;
    let credential = credential
        .as_deref()
        .map(CredentialRef::new)
        .transpose()
        .map_err(|e| corrupt(&e.to_string()))?;
    let revision = u64::try_from(revision)
        .map(ServerRevision::new)
        .map_err(|_| corrupt("the revision is not a positive number"))?;
    let server = UserToolServer::from_stored(id, &name, &url, auth, state, credential)
        .map_err(|e| corrupt(&e.to_string()))?;
    Ok(StoredServer::new(server, revision))
}

/// SQLSTATE `55P03`, `lock_not_available`: what a statement fails with when `lock_timeout` ends its
/// wait for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A driver error as the port says it: a row that does not decode is `Corrupt`; everything else (the
/// network, the pool, the server, a rejected statement, a lock that was not got in time) is
/// `Unavailable`, with the driver's error as the source. A driver error holds no parameter of the
/// statement, so no URL and no owner.
fn db_err(err: sqlx::Error) -> UserToolServerStoreError {
    if matches!(&err, sqlx::Error::Database(db) if db.code().as_deref() == Some(LOCK_NOT_AVAILABLE))
    {
        return UserToolServerStoreError::unavailable(
            "the definitions are locked by another writer for longer than a write waits; try again",
        )
        .with_source(err);
    }
    match sql_class(&err) {
        ErrorClass::Corrupt => {
            UserToolServerStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        ErrorClass::Transient => {
            UserToolServerStoreError::unavailable("the database cannot be reached").with_source(err)
        }
        _ => UserToolServerStoreError::unavailable("the database refused the request")
            .with_source(err),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// What the logs of the thread that runs a test say.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Runs `read` with every level of the log captured, and returns its result and the log.
    fn logged<T>(read: impl FnOnce() -> T) -> (T, String) {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        let result = tracing::subscriber::with_default(subscriber, read);
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        (result, log)
    }

    fn row(id: &str, url: &str) -> Row {
        (
            id.to_owned(),
            "Name-in-the-row".to_owned(),
            url.to_owned(),
            serde_json::json!({"kind": "none"}),
            "added".to_owned(),
            None,
            7,
        )
    }

    #[test]
    fn a_row_that_does_not_read_back_is_warned_of_once_by_its_id_and_by_nothing_else() {
        let secret = "s3cr3t-in-the-url";
        let url = format!("https://mcp.example.com/mcp?token={secret}");
        let (read, log) = logged(|| stored(row("u-abcdefghij", &url)));
        assert!(
            matches!(read, Err(UserToolServerStoreError::Corrupt(_))),
            "{read:?}"
        );
        assert_eq!(log.lines().count(), 1, "one warning: {log}");
        assert!(log.contains("WARN"), "{log}");
        assert!(
            log.contains("u-abcdefghij"),
            "the id is how it is found: {log}"
        );
        for leaked in [secret, "mcp.example.com", "Name-in-the-row", "token="] {
            assert!(!log.contains(leaked), "{leaked} is in the log: {log}");
        }
    }

    #[test]
    fn a_row_that_reads_back_logs_nothing() {
        let (read, log) = logged(|| stored(row("u-abcdefghij", "https://mcp.example.com/mcp")));
        assert!(read.is_ok(), "{read:?}");
        assert!(log.is_empty(), "{log}");
    }

    #[test]
    fn an_id_that_is_not_one_is_not_written_to_the_log() {
        // the table checks the shape of an id, so this is a row that was repaired by hand with the
        // check dropped: whatever it holds is not copied into the log
        let planted = "SENTINEL-not-an-id https://secret.example.com/path-token";
        let (read, log) = logged(|| stored(row(planted, "https://mcp.example.com/mcp")));
        assert!(
            matches!(read, Err(UserToolServerStoreError::Corrupt(_))),
            "{read:?}"
        );
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("WARN"), "{log}");
        for leaked in ["SENTINEL", "secret.example.com", "path-token"] {
            assert!(!log.contains(leaked), "{leaked} is in the log: {log}");
        }
    }
}
