//! `PendingAuthorizations` over Postgres: the authorizations of people's own MCP servers that have
//! begun and whose OAuth callback has not come (ADR 0046, slice 4c).

use std::time::Duration;

use jiff::Timestamp;
use orch_core::UserId;
use orch_ports::{
    OAuthState, PendingAuthorization, PendingAuthorizationError, PendingAuthorizations,
    StateDigest, UserToolServerId,
};
use sqlx::{PgPool, Postgres, Transaction};

use crate::codec::to_db;

/// How long a statement waits for a lock before it gives up as
/// [`PendingAuthorizationError::Unavailable`]: two seconds, as the definitions' writes
/// ([`LOCK_TIMEOUT`](crate::LOCK_TIMEOUT)). A statement holds a pooled connection from the moment it
/// waits, and the thread store shares the pool; a lock is held for milliseconds, so a wait that long
/// is somebody's hand-made transaction.
pub const PENDING_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// SQLSTATE `55P03`, `lock_not_available`: what a statement fails with when `lock_timeout` ends its
/// wait for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// [`PendingAuthorizations`] in the orchestrator's own Postgres: one row per server of each person
/// in `user_tool_oauth_pending`, created by [`PgStore::migrate`](crate::PgStore::migrate) (migration
/// `0022`, one history for the whole schema).
///
/// Cheap to clone: clones share the pool. It holds no secret, and **no state**: a row has the
/// SHA-256 of the state ([`StateDigest`]).
///
/// - **[`take`](PendingAuthorizations::take) is one `DELETE … WHERE state_hash = $1 RETURNING …`**:
///   of any number of callbacks for one state exactly one gets the row, in this process or another,
///   and an ended state's row is removed by the take that finds it, which gives back nothing.
/// - **[`put`](PendingAuthorizations::put) is one transaction**: it deletes the rows that ended at or
///   before the `now` it is given and an earlier put of the same state, then writes the row with
///   `INSERT … ON CONFLICT (owner, server) DO UPDATE`, so a server has one pending authorization and
///   the table has at most one row per server of each person.
/// - **Every statement is its own `READ COMMITTED` transaction** with [`PENDING_LOCK_TIMEOUT`], as
///   the definitions' writes are.
/// - **A driver's error that carries a row is not kept as the source**: Postgres puts the failing
///   row in the `DETAIL` of a constraint violation and a driver's `Debug` prints it, so for an error
///   the server answered the source is its message and its SQLSTATE, which hold no value.
#[derive(Clone, Debug)]
pub struct PgPendingAuthorizations {
    pool: PgPool,
}

impl PgPendingAuthorizations {
    /// The store over `pool` (the orchestrator's own, shared with the thread store, or one that pins a
    /// `search_path` in a test). The table is created by [`PgStore::migrate`](crate::PgStore::migrate);
    /// this does not migrate.
    pub fn from_pool(pool: PgPool) -> Self {
        PgPendingAuthorizations { pool }
    }

    /// The underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Begins a statement's transaction: `READ COMMITTED`, with the wait for a lock bounded.
    async fn begin(&self) -> Result<Transaction<'static, Postgres>, PendingAuthorizationError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await
            .map_err(db_err)?;
        let bounded = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET LOCAL lock_timeout = '{}ms'",
            PENDING_LOCK_TIMEOUT.as_millis()
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
}

impl PendingAuthorizations for PgPendingAuthorizations {
    async fn put(
        &self,
        state: &OAuthState,
        owner: &UserId,
        server: &UserToolServerId,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> Result<(), PendingAuthorizationError> {
        let digest = StateDigest::of(state);
        let mut tx = self.begin().await?;
        // what has ended, and an earlier put of this very state (a state is made once, from random
        // bytes, so the second is not a case a caller meets; it must still not fail)
        let swept = sqlx::query(
            "DELETE FROM user_tool_oauth_pending WHERE expires_at <= $1 OR state_hash = $2",
        )
        .bind(to_db(now))
        .bind(digest.as_bytes().as_slice())
        .execute(&mut *tx)
        .await;
        if let Err(err) = swept {
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        let written = sqlx::query(
            "INSERT INTO user_tool_oauth_pending (state_hash, owner, server, expires_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (owner, server) DO UPDATE SET \
               state_hash = EXCLUDED.state_hash, expires_at = EXCLUDED.expires_at, \
               created_at = clock_timestamp()",
        )
        .bind(digest.as_bytes().as_slice())
        .bind(owner.as_str())
        .bind(server.as_str())
        .bind(to_db(expires_at))
        .execute(&mut *tx)
        .await;
        if let Err(err) = written {
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        tx.commit().await.map_err(db_err)
    }

    async fn take(
        &self,
        state: &OAuthState,
        now: Timestamp,
    ) -> Result<Option<PendingAuthorization>, PendingAuthorizationError> {
        let digest = StateDigest::of(state);
        let mut tx = self.begin().await?;
        let taken: Result<Option<(String, String, jiff_sqlx::Timestamp)>, sqlx::Error> =
            sqlx::query_as(
                "DELETE FROM user_tool_oauth_pending WHERE state_hash = $1 \
                 RETURNING owner, server, expires_at",
            )
            .bind(digest.as_bytes().as_slice())
            .fetch_optional(&mut *tx)
            .await;
        let row = match taken {
            Ok(row) => row,
            Err(err) => {
                let _ = tx.rollback().await;
                return Err(db_err(err));
            }
        };
        tx.commit().await.map_err(db_err)?;
        let Some((owner, server, expires_at)) = row else {
            return Ok(None);
        };
        // ended: the row is gone (it was deleted above) and gives back nothing
        if now >= expires_at.to_jiff() {
            return Ok(None);
        }
        // a row the table's checks let through is a server id; one that is not (a repair by hand with
        // the check dropped) is a state nobody can use, and is not copied into a message
        let Ok(server) = UserToolServerId::parse(&server) else {
            tracing::warn!("a pending authorization names a server that is not a server id");
            return Ok(None);
        };
        Ok(Some(PendingAuthorization::new(UserId::new(&owner), server)))
    }
}

/// A driver error as the port says it. Everything is `Unavailable` (a store that cannot do it now,
/// and a statement it refuses, which for this table is a bug and not the caller's), with a sentence
/// that names nothing. The source of an error the server answered is its message and SQLSTATE and
/// not the driver's error, whose `Debug` carries the failing row.
fn db_err(err: sqlx::Error) -> PendingAuthorizationError {
    if let sqlx::Error::Database(db) = &err {
        let code = db.code().map(|c| c.into_owned());
        let source = std::io::Error::other(format!(
            "{} (SQLSTATE {})",
            db.message(),
            code.as_deref().unwrap_or("unknown")
        ));
        let detail = if code.as_deref() == Some(LOCK_NOT_AVAILABLE) {
            "the pending authorizations are locked by another writer for longer than a statement waits; try again"
        } else {
            "the database refused the request"
        };
        return PendingAuthorizationError::unavailable(detail).with_source(source);
    }
    PendingAuthorizationError::unavailable("the database cannot be reached").with_source(err)
}
