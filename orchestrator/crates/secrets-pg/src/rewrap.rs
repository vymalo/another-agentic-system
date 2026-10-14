//! `rewrap`: re-sealing the rows that are not under the current key.

use orch_ports::SecretError;

use crate::store::{COLUMNS, PgSecretStore, Row, Stored, db_err};

/// What one run of [`PgSecretStore::rewrap`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RewrapReport {
    /// Rows that were under another key and are now sealed under the current one, at the same
    /// version.
    pub rewrapped: u64,
    /// Rows that were under the current key already.
    pub already_current: u64,
    /// Rows that were written or deleted while the run was under way and were left alone.
    pub changed: u64,
    /// Rows that could not be re-sealed.
    pub failed: u64,
    /// The first of the failed rows, at most [`MAX_LISTED_FAILURES`].
    pub failures: Vec<RewrapFailure>,
}

/// The most failures a [`RewrapReport`] lists (all are counted).
pub const MAX_LISTED_FAILURES: usize = 100;

/// A row that could not be re-sealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewrapFailure {
    /// The server's text form (`u-abcdefghij`, or `u-abcdefghij#flow`). The owner is not named.
    pub server: String,
    /// The id of the key the row is sealed under.
    pub kid: String,
    /// Why.
    pub kind: RewrapFailureKind,
}

/// Why a row could not be re-sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RewrapFailureKind {
    /// Sealed under a key id this deployment does not hold: add that key back and run again.
    KeyUnknown,
    /// Does not open under a key that is held: altered, or moved to another owner, server or
    /// version.
    Corrupt,
}

impl RewrapReport {
    /// Every row seen is under the current key now: none failed and none changed while the run
    /// was under way.
    pub fn is_complete(&self) -> bool {
        self.failed == 0 && self.changed == 0
    }
}

/// Rows read at a time.
const PAGE: i64 = 100;

/// What became of one row.
enum Outcome {
    Rewrapped,
    AlreadyCurrent,
    Changed,
    Failed(RewrapFailure),
}

impl PgSecretStore {
    /// Re-seals every secret that is not under the current key, **in place at the version it has**.
    ///
    /// The run reads the table a page at a time, in the order of `(owner, server)`. A row under the
    /// current key is counted and left alone. For any other the run takes the key's lock (the one
    /// every writer takes), reads the row again, and
    ///
    /// - if the row is gone, or has another version than the run saw, leaves it and counts it as
    ///   `changed`: a writer was there first, and sealed what it wrote under its own current key;
    /// - if it is under the current key now (another run did it), counts it as current;
    /// - opens it with the key it names (a key id this deployment does not hold is `KeyUnknown`,
    ///   a row that does not open is `Corrupt`; either is counted as `failed`, listed, and left as
    ///   it was) and seals the same material again, **with a new nonce under the current key, at
    ///   the same version**, then commits.
    ///
    /// It is **not** a [`rotate`](orch_ports::SecretStore::rotate): that would give the row a new
    /// version, and a refresh that claimed the grant at version 7 and is waiting for the
    /// authorization server would find 8 when it keeps the refresh token the server just rotated
    /// to, read that as another replica taking over, and lose the token. A caller compares versions
    /// for equality only (the port's contract), and the material behind version 7 is the same, so
    /// the version stands. `updated` is the time the material last changed and is not touched.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] if the database fails; the run can be repeated, and rows already
    /// done are counted as current the next time.
    pub async fn rewrap(&self) -> Result<RewrapReport, SecretError> {
        let current = self.keys.current().as_str().to_owned();
        let mut report = RewrapReport::default();
        let mut after = (String::new(), String::new());
        loop {
            let page: Vec<(String, String, i64, String)> = sqlx::query_as(
                "SELECT owner, server, version, kid FROM user_secrets \
                 WHERE (owner, server) > ($1, $2) ORDER BY owner, server LIMIT $3",
            )
            .bind(&after.0)
            .bind(&after.1)
            .bind(PAGE)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
            let Some((last_owner, last_server, _, _)) = page.last() else {
                break;
            };
            after = (last_owner.clone(), last_server.clone());
            for (owner, server, version, kid) in &page {
                if *kid == current {
                    report.already_current += 1;
                    continue;
                }
                match self.rewrap_one(owner, server, *version, &current).await? {
                    Outcome::Rewrapped => report.rewrapped += 1,
                    Outcome::AlreadyCurrent => report.already_current += 1,
                    Outcome::Changed => report.changed += 1,
                    Outcome::Failed(failure) => {
                        tracing::warn!(
                            server = %failure.server,
                            kid = %failure.kid,
                            kind = ?failure.kind,
                            "a secret could not be re-sealed under the current key"
                        );
                        report.failed += 1;
                        if report.failures.len() < MAX_LISTED_FAILURES {
                            report.failures.push(failure);
                        }
                    }
                }
            }
        }
        Ok(report)
    }

    /// One row, under its key's lock, in one transaction.
    async fn rewrap_one(
        &self,
        owner: &str,
        server: &str,
        seen_version: i64,
        current: &str,
    ) -> Result<Outcome, SecretError> {
        let mut tx = self.locked(owner, server).await?;
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM user_secrets WHERE owner = $1 AND server = $2"
        )))
        .bind(owner)
        .bind(server)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Ok(Outcome::Changed);
        };
        if row.0 != seen_version {
            return Ok(Outcome::Changed);
        }
        let kid = row.2.clone();
        let failed = |kind| {
            Ok(Outcome::Failed(RewrapFailure {
                server: server.to_owned(),
                kid: kid.clone(),
                kind,
            }))
        };
        let stored = match Stored::decode(row) {
            Ok(stored) => stored,
            Err(_) => return failed(RewrapFailureKind::Corrupt),
        };
        if stored.envelope.kid == current {
            return Ok(Outcome::AlreadyCurrent);
        }
        let material = match self.open(owner, server, &stored) {
            Ok(material) => material,
            Err(SecretError::KeyUnknown) => return failed(RewrapFailureKind::KeyUnknown),
            Err(SecretError::Corrupt(_)) => return failed(RewrapFailureKind::Corrupt),
            Err(other) => return Err(other),
        };
        let sealed = self.seal(owner, server, stored.version, material.expose())?;
        let done = sqlx::query(
            "UPDATE user_secrets SET v = $4, kid = $5, nonce = $6, ciphertext = $7 \
             WHERE owner = $1 AND server = $2 AND version = $3",
        )
        .bind(owner)
        .bind(server)
        .bind(seen_version)
        .bind(sealed.v)
        .bind(&sealed.kid)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if done.rows_affected() != 1 {
            // not reachable with the lock held, and not a reason to write anything
            return Ok(Outcome::Changed);
        }
        tx.commit().await.map_err(db_err)?;
        Ok(Outcome::Rewrapped)
    }
}
