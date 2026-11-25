//! `PendingAuthorizations` over SQLite: the authorizations of people's own MCP servers that have
//! begun and whose OAuth callback has not come (ADR 0046, slice 4c), the SQLite form of
//! `store-postgres/src/pending_authorizations.rs`.
//!
//! The table is `user_tool_oauth_pending` (migration `0006`), **beside the log**: no event records
//! an authorization, a fork does not inherit it and deleting a thread erases nothing of it. It
//! holds no secret and **no state**: a row has the SHA-256 of the state ([`StateDigest`]).
//!
//! What each call does, and why it is the Postgres adapter's semantics:
//!
//! - **[`take`](PendingAuthorizations::take) is one `DELETE … RETURNING`** (SQLite 3.35 and later;
//!   the bundled build is 3.54.1), in one `BEGIN IMMEDIATE` transaction: of any number of callers
//!   for one state exactly one gets the row, and an ended state's row is removed by the take that
//!   finds it, which gives back nothing.
//! - **[`put`](PendingAuthorizations::put) is one `BEGIN IMMEDIATE` transaction**: it deletes the
//!   rows that ended at or before the `now` it is given and an earlier put of the same state, then
//!   writes the row with `INSERT … ON CONFLICT (owner, server) DO UPDATE`, so a server has one
//!   pending authorization and the table has at most one row per server of each person.
//! - **Times are microseconds**, rounded on write exactly as the Postgres adapter's `ts` rounds
//!   them, so a take that compares the caller's `now` with the stored `expires_at` answers the same
//!   in both stores. `created_at` is the database's own clock, as Postgres's `clock_timestamp()`;
//!   the port never reads it.
//! - **One writer, one transaction.** Postgres opens each statement's transaction `READ COMMITTED`
//!   with a two-second `lock_timeout`; here the store's one connection and its one `BEGIN IMMEDIATE`
//!   are the serialisation, and a lock that outlives the file's five-second busy timeout is
//!   [`PendingAuthorizationError::Unavailable`] like everything else, which is the only class this
//!   port has.
//! - **Nothing of an error names a state, an owner or a server**: the driver's failure is the source
//!   and SQLite's messages hold no bound parameter.
//! - **A server id that does not parse is nothing.** A row the table's checks let through is a
//!   server id; one that is not (a repair by hand with the check dropped) is a state nobody can use,
//!   and is not copied into a message, as in Postgres.

use jiff::Timestamp;
use orch_core::UserId;
use orch_ports::{
    OAuthState, PendingAuthorization, PendingAuthorizationError, PendingAuthorizations,
    StateDigest, StoreError, UserToolServerId,
};
use rusqlite::{OptionalExtension, params};

use crate::SqliteStore;
use crate::codec::{from_db, to_db};
use crate::error::store_err;
use crate::store::write;

/// The database's clock, in microseconds: what `created_at` holds, as Postgres's
/// `clock_timestamp()`.
const NOW_IN_DB: &str = "CAST(unixepoch('subsec') * 1000000 AS INTEGER)";

impl PendingAuthorizations for SqliteStore {
    async fn put(
        &self,
        state: &OAuthState,
        owner: &UserId,
        server: &UserToolServerId,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> Result<(), PendingAuthorizationError> {
        let digest = StateDigest::of(state);
        let (owner, server) = (owner.as_str().to_owned(), server.as_str().to_owned());
        self.call(move |conn| {
            write(conn, |tx| {
                // what has ended, and an earlier put of this very state (a state is made once, from
                // random bytes, so the second is not a case a caller meets; it must still not fail)
                tx.execute(
                    "DELETE FROM user_tool_oauth_pending WHERE expires_at <= ?1 OR state_hash = ?2",
                    params![to_db(now)?, digest.as_bytes().as_slice()],
                )
                .map_err(store_err)?;
                tx.execute(
                    &format!(
                        "INSERT INTO user_tool_oauth_pending \
                           (state_hash, owner, server, expires_at, created_at) \
                         VALUES (?1, ?2, ?3, ?4, {NOW_IN_DB}) \
                         ON CONFLICT (owner, server) DO UPDATE SET \
                           state_hash = excluded.state_hash, \
                           expires_at = excluded.expires_at, \
                           created_at = {NOW_IN_DB}"
                    ),
                    params![
                        digest.as_bytes().as_slice(),
                        owner,
                        server,
                        to_db(expires_at)?
                    ],
                )
                .map_err(store_err)?;
                Ok(())
            })
        })
        .await
        .map_err(port_err)
    }

    async fn take(
        &self,
        state: &OAuthState,
        now: Timestamp,
    ) -> Result<Option<PendingAuthorization>, PendingAuthorizationError> {
        let digest = StateDigest::of(state);
        self.call(move |conn| {
            write(conn, |tx| {
                let row: Option<(String, String, i64)> = tx
                    .query_row(
                        "DELETE FROM user_tool_oauth_pending WHERE state_hash = ?1 \
                         RETURNING owner, server, expires_at",
                        params![digest.as_bytes().as_slice()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(store_err)?;
                let Some((owner, server, expires_at)) = row else {
                    return Ok(None);
                };
                // ended: the row is gone (it was deleted above) and gives back nothing
                if now >= from_db(expires_at)? {
                    return Ok(None);
                }
                // a row the table's checks let through is a server id; one that is not (a repair by
                // hand with the check dropped) is a state nobody can use, and is not copied into a
                // message
                let Ok(server) = UserToolServerId::parse(&server) else {
                    tracing::warn!(
                        "a pending authorization names a server that is not a server id"
                    );
                    return Ok(None);
                };
                Ok(Some(PendingAuthorization::new(UserId::new(&owner), server)))
            })
        })
        .await
        .map_err(port_err)
    }
}

/// A store failure as the port says it. Everything is `Unavailable` (a store that cannot do it now,
/// and a statement it refuses, which for this table is a bug and not the caller's), with a sentence
/// that names nothing. The source is the failure as this crate keeps it; it holds no parameter of
/// the statement, so no state, no owner and no server.
fn port_err(err: StoreError) -> PendingAuthorizationError {
    match err {
        StoreError::Unavailable { .. } => PendingAuthorizationError::unavailable(
            "the pending authorizations are locked by another writer; try again",
        )
        .with_source(err),
        StoreError::Corrupt { .. } => PendingAuthorizationError::unavailable(
            "a row of the pending authorizations does not decode",
        )
        .with_source(err),
        _ => PendingAuthorizationError::unavailable("the database refused the request")
            .with_source(err),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use jiff::SignedDuration;

    use orch_core::UserId;
    use orch_ports::{OAuthState, StateDigest};

    use super::*;
    use crate::SqliteStore;
    use crate::error::store_err;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_second(1_790_000_000)
            .unwrap()
            .checked_add(SignedDuration::from_secs(secs))
            .unwrap()
    }

    fn state() -> OAuthState {
        OAuthState::new(&uuid::Uuid::now_v7().simple().to_string()).unwrap()
    }

    fn server(suffix: &str) -> UserToolServerId {
        UserToolServerId::parse(&format!("u-{suffix}")).unwrap()
    }

    /// A migrated store on a file of its own under the temporary directory.
    async fn fresh(name: &str) -> (SqliteStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "orch-store-sqlite-pending-{name}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStore::open(dir.join("orch.sqlite")).unwrap();
        store.migrate().await.unwrap();
        (store, dir)
    }

    /// How many rows the table has, of every owner.
    async fn rows(store: &SqliteStore) -> i64 {
        store
            .call(|conn| {
                conn.query_row("SELECT count(*) FROM user_tool_oauth_pending", [], |r| {
                    r.get(0)
                })
                .map_err(store_err)
            })
            .await
            .unwrap()
    }

    /// A row keeps the SHA-256 of the state and nothing of the state: the port leaves this to each
    /// adapter's own tests, and the conformance suite only sees the port.
    #[tokio::test]
    async fn a_row_holds_the_digest_of_the_state_and_never_the_state() {
        let (store, dir) = fresh("hash").await;
        let planted = OAuthState::new("SENTINEL-state-0123456789abcdef").unwrap();
        let owner = UserId::new("alice@example.test");
        let x = server("aaaaaaaaaa");
        store
            .put(&planted, &owner, &x, at(600), at(0))
            .await
            .unwrap();

        let (digest, whole): (Vec<u8>, String) = store
            .call(|conn| {
                conn.query_row(
                    "SELECT state_hash, \
                       hex(state_hash) || '|' || owner || '|' || server || '|' || expires_at \
                         || '|' || created_at \
                     FROM user_tool_oauth_pending",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(store_err)
            })
            .await
            .unwrap();
        assert_eq!(
            digest,
            StateDigest::of(&planted).as_bytes().to_vec(),
            "the column is the digest of the state"
        );
        assert!(
            !whole.contains(planted.as_str()),
            "the state is in the row: {whole}"
        );
        assert!(!whole.contains("SENTINEL"), "nor a piece of it: {whole}");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A server has one row however often it authorizes, and a put sweeps the rows of the flows that
    /// have ended: the table is bounded by the servers of the people, not by the flows they began.
    #[tokio::test]
    async fn a_server_has_one_row_and_the_next_put_sweeps_what_has_ended() {
        let (store, dir) = fresh("rows").await;
        let (alice, bob) = (
            UserId::new("alice@example.test"),
            UserId::new("bob@example.test"),
        );
        let (x, y) = (server("aaaaaaaaaa"), server("bbbbbbbbbb"));
        for _ in 0..3 {
            store
                .put(&state(), &alice, &x, at(600), at(0))
                .await
                .unwrap();
        }
        store
            .put(&state(), &alice, &y, at(600), at(0))
            .await
            .unwrap();
        assert_eq!(rows(&store).await, 2, "a server replaces its own row");
        // ten minutes later someone begins a flow: the two rows ended at 600
        store
            .put(&state(), &bob, &x, at(1_300), at(700))
            .await
            .unwrap();
        assert_eq!(rows(&store).await, 1, "the ended rows are swept");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
