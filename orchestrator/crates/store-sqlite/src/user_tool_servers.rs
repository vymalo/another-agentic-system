//! `UserToolServerStore` over SQLite: the definitions of people's own MCP servers (ADR 0046,
//! migration `0005`), the SQLite form of `store-postgres/src/user_tool_servers.rs`.
//!
//! The table is `user_tool_servers`, **beside the log**: no event records a definition, a fork does
//! not inherit it and deleting a thread erases nothing of it. A definition holds no secret; the
//! credential column is only the broker's opaque reference.
//!
//! - **One writer, one transaction.** Postgres serialises the inserts of one owner with an advisory
//!   lock before counting; here every write is a `BEGIN IMMEDIATE` transaction on the one connection,
//!   so the id, the count and the insert happen with nobody between them and the limit of eight holds
//!   under racing inserts. The refusals come in the port's order: `Exists` first, then `TooMany`.
//! - **A revision is a number of one counter** (`user_tool_server_counter`), drawn by the statement
//!   that writes it and never given twice, a delete and a new insert of the same id included.
//! - **A compare and swap is one transaction**: the stored revision is read, compared with the one
//!   passed, and the row is written under it, so a writer whose read is stale writes nothing
//!   (`Conflict`), and a definition that is gone is `NotFound`, never brought back.
//! - **A row is read back through [`UserToolServer::from_stored`]**: whatever is wrong is `Corrupt`,
//!   with a message that holds no owner and no URL. A row that does not read back is warned of once,
//!   by its id, so an operator can find it.
//! - **The url may carry a secret in its path**: it is never logged and no error holds it.

use orch_core::{Timestamp, UserId};
use orch_ports::{
    CredentialRef, MAX_USER_TOOL_SERVERS, ServerRevision, StoreError, StoredServer, UserAuth,
    UserToolServer, UserToolServerId, UserToolServerState, UserToolServerStore,
    UserToolServerStoreError,
};
use rusqlite::{OptionalExtension, params};

use crate::SqliteStore;
use crate::codec::to_db;
use crate::error::{store_err, unique_on};
use crate::store::write;

/// A row, in the order of [`COLUMNS`].
type Row = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
);

/// The columns of a row, in the order of [`Row`].
const COLUMNS: &str = "id, name, url, auth, state, credential, last4, revision";

impl UserToolServerStore for SqliteStore {
    async fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        let written = to_db(Timestamp::now()).map_err(port_err)?;
        let (owner, fields) = (owner.as_str().to_owned(), Fields::of(server)?);
        let revision = self
            .call(move |conn| {
                write(conn, |tx| {
                    // The id first, so that at the limit a definition that is there already is
                    // "exists", and not "too many".
                    let exists: bool = tx
                        .query_row(
                            "SELECT EXISTS (SELECT 1 FROM user_tool_servers \
                             WHERE owner = ?1 AND id = ?2)",
                            params![&owner, &fields.id],
                            |r| r.get(0),
                        )
                        .map_err(store_err)?;
                    if exists {
                        return Ok(Err(UserToolServerStoreError::Exists));
                    }
                    let held: i64 = tx
                        .query_row(
                            "SELECT count(*) FROM user_tool_servers WHERE owner = ?1",
                            [&owner],
                            |r| r.get(0),
                        )
                        .map_err(store_err)?;
                    if !usize::try_from(held).is_ok_and(|held| held < MAX_USER_TOOL_SERVERS) {
                        return Ok(Err(UserToolServerStoreError::TooMany {
                            limit: MAX_USER_TOOL_SERVERS,
                        }));
                    }
                    let revision = next_revision(tx)?;
                    let inserted = tx.execute(
                        "INSERT INTO user_tool_servers \
                           (owner, id, name, url, auth, state, credential, last4, revision, \
                            created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?10, ?8, ?9, ?9)",
                        params![
                            &owner,
                            &fields.id,
                            fields.name,
                            fields.url,
                            fields.auth,
                            fields.state,
                            fields.credential,
                            revision,
                            written,
                            fields.last4
                        ],
                    );
                    match inserted {
                        Ok(_) => Ok(Ok(revision)),
                        // the row was written by something that did not read the count first
                        Err(e) if unique_on(&e, "user_tool_servers.owner") => {
                            Ok(Err(UserToolServerStoreError::Exists))
                        }
                        Err(e) => Err(store_err(e)),
                    }
                })
            })
            .await
            .map_err(port_err)??;
        revision_of(revision)
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        let (owner, id) = (owner.as_str().to_owned(), id.as_str().to_owned());
        let row: Option<Row> = self
            .call(move |conn| {
                conn.query_row(
                    &format!(
                        "SELECT {COLUMNS} FROM user_tool_servers WHERE owner = ?1 AND id = ?2"
                    ),
                    params![owner, id],
                    decode_row,
                )
                .optional()
                .map_err(store_err)
            })
            .await
            .map_err(port_err)?;
        row.map(stored).transpose()
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        let owner = owner.as_str().to_owned();
        let rows: Vec<Row> = self
            .call(move |conn| {
                crate::store::rows(
                    conn,
                    &format!(
                        "SELECT {COLUMNS} FROM user_tool_servers WHERE owner = ?1 ORDER BY id"
                    ),
                    &[&owner],
                    |r| decode_row(r).map_err(store_err),
                )
            })
            .await
            .map_err(port_err)?;
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
        let written = to_db(Timestamp::now()).map_err(port_err)?;
        let (owner, fields) = (owner.as_str().to_owned(), Fields::of(server)?);
        let expected = stored_revision(expected);
        let outcome = self
            .call(move |conn| {
                write(conn, |tx| {
                    if let Some(refusal) = compare(tx, &owner, &fields.id, expected)? {
                        return Ok(Err(refusal));
                    }
                    let revision = next_revision(tx)?;
                    let updated = tx
                        .execute(
                            "UPDATE user_tool_servers SET name = ?3, url = ?4, auth = ?5, \
                               state = ?6, credential = ?7, last4 = ?11, revision = ?8, updated_at = ?9 \
                             WHERE owner = ?1 AND id = ?2 AND revision = ?10",
                            params![
                                owner,
                                fields.id,
                                fields.name,
                                fields.url,
                                fields.auth,
                                fields.state,
                                fields.credential,
                                revision,
                                written,
                                expected,
                                fields.last4
                            ],
                        )
                        .map_err(store_err)?;
                    // Unreachable under the one-writer model: `compare` read this row at this very
                    // revision, and no other writer can be in the file between the two statements.
                    // The check stays because a row that did vanish must spend no revision: this
                    // `Err` leaves `write` before its commit, which rolls the counter's draw back.
                    if updated == 0 {
                        return Err(StoreError::VersionConflict);
                    }
                    Ok(Ok(revision))
                })
            })
            .await;
        let revision = match outcome {
            Err(StoreError::VersionConflict) => return Err(UserToolServerStoreError::Conflict),
            other => other.map_err(port_err)??,
        };
        revision_of(revision)
    }

    async fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        let written = to_db(Timestamp::now()).map_err(port_err)?;
        let (owner, id) = (owner.as_str().to_owned(), id.as_str().to_owned());
        let expected = stored_revision(expected);
        let outcome = self
            .call(move |conn| {
                write(conn, |tx| {
                    if let Some(refusal) = compare(tx, &owner, &id, expected)? {
                        return Ok(Err(refusal));
                    }
                    let revision = next_revision(tx)?;
                    let updated = tx
                        .execute(
                            "UPDATE user_tool_servers SET state = ?3, revision = ?4, updated_at = ?5 \
                             WHERE owner = ?1 AND id = ?2 AND revision = ?6",
                            params![owner, id, state.as_str(), revision, written, expected],
                        )
                        .map_err(store_err)?;
                    // Unreachable under the one-writer model: `compare` read this row at this very
                    // revision, and no other writer can be in the file between the two statements.
                    // The check stays because a row that did vanish must spend no revision: this
                    // `Err` leaves `write` before its commit, which rolls the counter's draw back.
                    if updated == 0 {
                        return Err(StoreError::VersionConflict);
                    }
                    Ok(Ok(revision))
                })
            })
            .await;
        let revision = match outcome {
            Err(StoreError::VersionConflict) => return Err(UserToolServerStoreError::Conflict),
            other => other.map_err(port_err)??,
        };
        revision_of(revision)
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        let (owner, id) = (owner.as_str().to_owned(), id.as_str().to_owned());
        self.call(move |conn| {
            write(conn, |tx| {
                tx.execute(
                    "DELETE FROM user_tool_servers WHERE owner = ?1 AND id = ?2",
                    params![owner, id],
                )
                .map_err(store_err)?;
                Ok(())
            })
        })
        .await
        .map_err(port_err)
    }
}

/// The columns a write sends, rendered once outside the blocking task.
struct Fields {
    id: String,
    name: String,
    url: String,
    auth: String,
    state: &'static str,
    credential: Option<String>,
    last4: Option<String>,
}

impl Fields {
    fn of(server: &UserToolServer) -> Result<Self, UserToolServerStoreError> {
        Ok(Self {
            id: server.id().as_str().to_owned(),
            name: server.name().to_owned(),
            url: server.url().to_owned(),
            auth: serde_json::to_string(server.auth()).map_err(|_| {
                UserToolServerStoreError::unavailable("the kind of authentication cannot be stored")
            })?,
            state: server.state().as_str(),
            credential: server.credential().map(|c| c.as_str().to_owned()),
            last4: server.last4().map(str::to_owned),
        })
    }
}

/// The compare of a compare-and-swap, in the writing transaction: `None` when the stored revision is
/// `expected` and the write may go ahead; otherwise the refusal the port names (`NotFound` when the
/// definition is not there, `Conflict` when it is at another revision). Nothing is written.
fn compare(
    tx: &rusqlite::Transaction<'_>,
    owner: &str,
    id: &str,
    expected: i64,
) -> Result<Option<UserToolServerStoreError>, StoreError> {
    let current: Option<i64> = tx
        .query_row(
            "SELECT revision FROM user_tool_servers WHERE owner = ?1 AND id = ?2",
            params![owner, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(store_err)?;
    Ok(match current {
        None => Some(UserToolServerStoreError::NotFound),
        Some(held) if held != expected => Some(UserToolServerStoreError::Conflict),
        Some(_) => None,
    })
}

/// Draws the next revision from the counter, in the writing transaction.
fn next_revision(tx: &rusqlite::Transaction<'_>) -> Result<i64, StoreError> {
    tx.query_row(
        "UPDATE user_tool_server_counter SET n = n + 1 WHERE one = 1 RETURNING n",
        [],
        |r| r.get(0),
    )
    .map_err(store_err)
}

/// A revision as the column holds it. One that does not fit is not any revision the store gave:
/// `-1`, which the column's check forbids, so the write matches nothing and is a `Conflict`.
fn stored_revision(revision: ServerRevision) -> i64 {
    i64::try_from(revision.get()).unwrap_or(-1)
}

fn revision_of(revision: i64) -> Result<ServerRevision, UserToolServerStoreError> {
    u64::try_from(revision)
        .map(ServerRevision::new)
        .map_err(|_| {
            UserToolServerStoreError::unavailable("the revision counter gave a number out of range")
        })
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

/// A row, read back through [`UserToolServer::from_stored`]: whatever is wrong is `Corrupt`, with a
/// sentence that holds no owner, URL or reference. Each such row is also **warned of once, by its
/// id**: the port's error names no id and the person is told to ask the operator, who finds the id
/// here.
fn stored(row: Row) -> Result<StoredServer, UserToolServerStoreError> {
    let id = row.0.clone();
    read_back(row).inspect_err(|error| warn_unreadable(&id, error))
}

/// The warning for a row that does not read back. It names the id and the sentence of the error,
/// which holds no owner, URL or reference: never the owner, the name or the URL. An id that is not
/// one is not copied: nothing says what it holds.
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
    (id, name, url, auth, state, credential, last4, revision): Row,
) -> Result<StoredServer, UserToolServerStoreError> {
    let corrupt = |what: &str| {
        UserToolServerStoreError::Corrupt(format!("a stored definition does not read back: {what}"))
    };
    let id = UserToolServerId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let auth: UserAuth = serde_json::from_str(&auth)
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
    if last4.as_deref().is_some_and(|l| l.chars().count() != 4) {
        return Err(corrupt("the last four characters are not four"));
    }
    let server = UserToolServer::from_stored(id, &name, &url, auth, state, credential)
        .map_err(|e| corrupt(&e.to_string()))?
        .with_last4(last4);
    Ok(StoredServer::new(server, revision))
}

/// A store failure as the port says it. The driver's error is the source; it holds no parameter of
/// the statement, so no URL and no owner.
fn port_err(err: StoreError) -> UserToolServerStoreError {
    match err {
        StoreError::Unavailable { source } => {
            UserToolServerStoreError::unavailable("the database is busy; try again")
                .with_source(source)
        }
        StoreError::Corrupt { .. } => {
            UserToolServerStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        StoreError::Internal { source } => {
            UserToolServerStoreError::unavailable("the database refused the request")
                .with_source(source)
        }
        _ => UserToolServerStoreError::unavailable("the database refused the request"),
    }
}
