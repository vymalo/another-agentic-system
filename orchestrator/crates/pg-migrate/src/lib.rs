//! Applies a crate's Postgres migrations on `tokio-postgres`, and continues the history that sqlx
//! kept before it (ADR 0076, sys#382 E1): an existing database migrates as a no-op, and a process
//! of the build before, still on sqlx, and one of this build never migrate at the same time.
//!
//! What it keeps from sqlx 0.9's migrator, so that either can read what the other wrote:
//!
//! * **The history table.** `version BIGINT PRIMARY KEY, description TEXT, installed_on
//!   TIMESTAMPTZ, success BOOLEAN, checksum BYTEA, execution_time BIGINT`, under the name the crate
//!   gives ([`Migrator::new`]); created if missing.
//! * **A migration.** A file `<version>_<description>.sql`: the version is the integer before the
//!   first `_`, the description the rest without `.sql`, each `_` a space. Its checksum is the
//!   SHA-384 of its text. A file whose text starts with `-- no-transaction` runs outside a
//!   transaction; every other runs in one with its history row.
//! * **The lock.** `pg_advisory_lock(0x3d32ad9e * crc32(current_database()))`, CRC-32/ISO-HDLC,
//!   held for the whole run on the run's connection, and released at its end, an error included.
//! * **The refusals.** A row that did not succeed ([`MigrateError::Dirty`]) and a version the
//!   history has and this build does not ([`MigrateError::VersionMissing`]) stop the run before
//!   anything is applied; a version whose checksum differs ([`MigrateError::VersionMismatch`])
//!   stops it where that version comes, as sqlx does (a lower version not yet applied is applied
//!   first).
//! * **The session.** `TimeZone` is `UTC` and `DateStyle` is `ISO, MDY` for the run, as sqlx sets
//!   them on every connection, so a migration reads a literal as it did.
//!
//! A reversible migration (`.up.sql`, `.down.sql`) is refused: sqlx runs only the `up` half, and
//! none exists here. A session lock needs one session: run it on a connection to the database
//! itself, not through a transaction-mode pooler. A run that is cancelled (its future dropped)
//! keeps the lock until the connection closes: drop the client then.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::Instant;

use sha2::{Digest, Sha384};
use tokio_postgres::Client;

/// One migration: its version, its description and its SQL, as sqlx read them from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    file_name: String,
    version: i64,
    description: String,
    sql: &'static str,
    no_tx: bool,
}

impl Migration {
    /// The migration of the file `file_name` (`0001_user_secrets.sql`) whose text is `sql`, as
    /// sqlx names it: version `1`, description `user secrets`.
    ///
    /// # Errors
    ///
    /// [`MigrateError::FileName`] for a name that is not `<integer>_<description>.sql`.
    pub fn from_file(file_name: &str, sql: &'static str) -> Result<Self, MigrateError> {
        let bad = || MigrateError::FileName(file_name.to_owned());
        if file_name.ends_with(".up.sql") || file_name.ends_with(".down.sql") {
            return Err(bad());
        }
        let (version, rest) = file_name.split_once('_').ok_or_else(bad)?;
        let description = rest.strip_suffix(".sql").ok_or_else(bad)?;
        Ok(Self {
            file_name: file_name.to_owned(),
            version: version.parse().map_err(|_| bad())?,
            description: description.replace('_', " "),
            sql,
            no_tx: sql.starts_with("-- no-transaction"),
        })
    }

    /// The name of its file.
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Whether it runs outside a transaction (its text starts with `-- no-transaction`).
    pub fn no_transaction(&self) -> bool {
        self.no_tx
    }

    /// Its version.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Its description, as the history records it.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The SHA-384 of its text: what the history records, and compares on every run.
    pub fn checksum(&self) -> Vec<u8> {
        checksum(self.sql)
    }
}

/// The checksum sqlx records for a migration's text: its SHA-384.
pub fn checksum(sql: &str) -> Vec<u8> {
    Sha384::digest(sql.as_bytes()).to_vec()
}

/// The key of the advisory lock that sqlx's migrator takes for the database `database_name`, and
/// this one: `0x3d32ad9e * crc32(name)`, CRC-32/ISO-HDLC.
pub fn lock_id(database_name: &str) -> i64 {
    const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);
    0x3d32_ad9e * i64::from(CRC.checksum(database_name.as_bytes()))
}

/// The migrations of a crate in a history table of its own.
#[derive(Clone, Debug)]
pub struct Migrator {
    table: String,
    migrations: Vec<Migration>,
}

impl Migrator {
    /// The migrations `migrations`, recorded in the table `table`: a name of letters, digits and
    /// `_` only, since it is written into the statements.
    ///
    /// # Errors
    ///
    /// [`MigrateError::TableName`] for another name, [`MigrateError::DuplicateVersion`] for two
    /// migrations of one version.
    pub fn new(table: &str, mut migrations: Vec<Migration>) -> Result<Self, MigrateError> {
        let named =
            !table.is_empty() && table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !named {
            return Err(MigrateError::TableName(table.to_owned()));
        }
        migrations.sort_by_key(|m| m.version);
        if let Some(pair) = migrations.windows(2).find(|w| w[0].version == w[1].version) {
            return Err(MigrateError::DuplicateVersion(pair[0].version));
        }
        Ok(Self {
            table: table.to_owned(),
            migrations,
        })
    }

    /// The migrations, in the order of their versions.
    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    /// Whether the migrations are exactly the `.sql` files of `dir`: what a crate's test calls on
    /// its `migrations/`, since `migrations!` names its files one by one and a file added to the
    /// directory but not to the list would never run.
    ///
    /// # Errors
    ///
    /// The names that are in one and not the other, each prefixed `unlisted ` (in the directory
    /// only) or `missing ` (in the list only); or the directory's read error, as text.
    pub fn check_directory(&self, dir: &std::path::Path) -> Result<(), Vec<String>> {
        let entries =
            std::fs::read_dir(dir).map_err(|e| vec![format!("{}: {e}", dir.display())])?;
        let mut files = Vec::new();
        for entry in entries {
            let name = entry.map_err(|e| vec![e.to_string()])?.file_name();
            let name = name.to_string_lossy().into_owned();
            if name.ends_with(".sql") {
                files.push(name);
            }
        }
        let listed: Vec<&str> = self.migrations.iter().map(Migration::file_name).collect();
        let mut wrong: Vec<String> = files
            .iter()
            .filter(|f| !listed.contains(&f.as_str()))
            .map(|f| format!("unlisted {f}"))
            .collect();
        wrong.extend(
            listed
                .iter()
                .filter(|l| !files.iter().any(|f| f == *l))
                .map(|l| format!("missing {l}")),
        );
        if wrong.is_empty() { Ok(()) } else { Err(wrong) }
    }

    /// Applies every migration the history does not have, under the database's advisory lock, and
    /// releases the lock whatever the outcome. Idempotent, and safe to call from every replica at
    /// boot.
    ///
    /// # Errors
    ///
    /// [`MigrateError::Dirty`], [`MigrateError::VersionMissing`] or
    /// [`MigrateError::VersionMismatch`] when the history does not match this build (nothing is
    /// applied); [`MigrateError::Execute`] when a migration's SQL fails (its transaction is rolled
    /// back); [`MigrateError::Database`] for the connection or a bookkeeping statement.
    pub async fn run(&self, client: &mut Client) -> Result<(), MigrateError> {
        let database: String = client
            .query_one("SELECT current_database()", &[])
            .await?
            .try_get(0)?;
        let lock = lock_id(&database);
        client
            .execute("SELECT pg_advisory_lock($1)", &[&lock])
            .await?;
        // sqlx's session: a migration reads a time or a date as it did.
        let outcome = match client
            .batch_execute("SET TimeZone = 'UTC'; SET DateStyle = 'ISO, MDY'")
            .await
        {
            Ok(()) => self.run_locked(client).await,
            Err(e) => Err(e.into()),
        };
        let unlocked = client
            .execute("SELECT pg_advisory_unlock($1)", &[&lock])
            .await;
        outcome?;
        unlocked?;
        Ok(())
    }

    async fn run_locked(&self, client: &mut Client) -> Result<(), MigrateError> {
        let table = &self.table;
        client
            .batch_execute(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
);"
            ))
            .await?;
        let dirty = client
            .query_opt(
                &format!(
                    "SELECT version FROM {table} WHERE success = false ORDER BY version LIMIT 1"
                ),
                &[],
            )
            .await?;
        if let Some(row) = dirty {
            return Err(MigrateError::Dirty(row.try_get(0)?));
        }
        let applied: HashMap<i64, Vec<u8>> = client
            .query(
                &format!("SELECT version, checksum FROM {table} ORDER BY version"),
                &[],
            )
            .await?
            .into_iter()
            .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
            .collect::<Result<_, tokio_postgres::Error>>()?;
        if let Some(missing) = applied
            .keys()
            .copied()
            .filter(|v| !self.migrations.iter().any(|m| m.version == *v))
            .min()
        {
            return Err(MigrateError::VersionMissing(missing));
        }
        for migration in &self.migrations {
            match applied.get(&migration.version) {
                Some(recorded) if *recorded != migration.checksum() => {
                    return Err(MigrateError::VersionMismatch(migration.version));
                }
                Some(_) => {}
                None => self.apply(client, migration).await?,
            }
        }
        Ok(())
    }

    /// The migration's SQL and its history row, in one transaction unless the file opts out, then
    /// its execution time (lost if the process dies between the two, as with sqlx).
    async fn apply(&self, client: &mut Client, migration: &Migration) -> Result<(), MigrateError> {
        let table = &self.table;
        let start = Instant::now();
        let record = format!(
            "INSERT INTO {table} (version, description, success, checksum, execution_time) \
             VALUES ($1, $2, TRUE, $3, -1)"
        );
        let execute = |e| MigrateError::Execute {
            version: migration.version,
            source: e,
        };
        let checksum = migration.checksum();
        if migration.no_tx {
            client.batch_execute(migration.sql).await.map_err(execute)?;
            client
                .execute(
                    &record,
                    &[&migration.version, &migration.description, &checksum],
                )
                .await?;
        } else {
            let tx = client.transaction().await?;
            tx.batch_execute(migration.sql).await.map_err(execute)?;
            tx.execute(
                &record,
                &[&migration.version, &migration.description, &checksum],
            )
            .await?;
            tx.commit().await?;
        }
        let elapsed = i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX);
        client
            .execute(
                &format!("UPDATE {table} SET execution_time = $1 WHERE version = $2"),
                &[&elapsed, &migration.version],
            )
            .await?;
        Ok(())
    }
}

/// Builds a crate's migrations from files under `dir`, relative to the crate's manifest:
/// `migrations!("migrations": "0001_init.sql", "0002_more.sql")`. Each file's text is in the
/// binary (a missing file fails the build); a name that is not `<integer>_<description>.sql` is an
/// error when the list is built. Check the list against the directory in a test:
/// [`Migrator::check_directory`].
#[macro_export]
macro_rules! migrations {
    ($dir:literal : $($file:literal),+ $(,)?) => {
        [$(
            $crate::Migration::from_file(
                $file,
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $file)),
            )
        ),+]
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
    };
}

/// Why a migration run did not complete.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrateError {
    /// A migration file is not named `<integer>_<description>.sql`.
    #[error("the migration file {0:?} is not named <integer>_<description>.sql")]
    FileName(String),
    /// The history table's name is not letters, digits and `_`.
    #[error("the history table {0:?} is not a name of letters, digits and _")]
    TableName(String),
    /// Two migrations of one version.
    #[error("two migrations have the version {0}")]
    DuplicateVersion(i64),
    /// A migration of the history did not succeed: someone has to look before anything runs.
    #[error("migration {0} is recorded as failed; repair it before migrating again")]
    Dirty(i64),
    /// The history has a version this build does not: a newer build migrated this database.
    #[error("migration {0} was applied but this build does not have it")]
    VersionMissing(i64),
    /// A migration's text differs from what the history recorded for its version.
    #[error("migration {0} was applied with other text than this build has")]
    VersionMismatch(i64),
    /// A migration's SQL failed; its transaction (when it has one) was rolled back.
    #[error("migration {version} failed")]
    Execute {
        /// The migration.
        version: i64,
        /// The database's error.
        #[source]
        source: tokio_postgres::Error,
    },
    /// The connection, the lock or a bookkeeping statement failed.
    #[error("the database failed while migrating")]
    Database(#[from] tokio_postgres::Error),
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_is_read_as_sqlx_reads_it() {
        let m = Migration::from_file("0001_user_secrets.sql", "SELECT 1;").unwrap();
        assert_eq!((m.version(), m.description()), (1, "user secrets"));
        assert!(!m.no_tx);
        let no_tx = Migration::from_file("12_a_b_c.sql", "-- no-transaction\nSELECT 1;").unwrap();
        assert_eq!((no_tx.version(), no_tx.description()), (12, "a b c"));
        assert!(no_tx.no_tx);
        for bad in [
            "0001.sql",
            "x_y.sql",
            "0001_y.txt",
            "_y.sql",
            "2_x.down.sql",
            "2_x.up.sql",
        ] {
            assert!(
                matches!(
                    Migration::from_file(bad, ""),
                    Err(MigrateError::FileName(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_table_name_and_versions_are_checked() {
        let one = || Migration::from_file("1_a.sql", "").unwrap();
        assert!(matches!(
            Migrator::new("x; DROP TABLE y", vec![one()]),
            Err(MigrateError::TableName(_))
        ));
        assert!(matches!(
            Migrator::new("t", vec![one(), one()]),
            Err(MigrateError::DuplicateVersion(1))
        ));
        let sorted = Migrator::new(
            "t",
            vec![Migration::from_file("2_b.sql", "").unwrap(), one()],
        )
        .unwrap();
        let versions: Vec<i64> = sorted.migrations().iter().map(Migration::version).collect();
        assert_eq!(versions, [1, 2]);
    }

    #[test]
    fn the_lock_key_is_sqlx_s() {
        // sqlx-postgres 0.9 `generate_lock_id`: 0x3d32ad9e * CRC-32/ISO-HDLC of the name. The
        // CRC-32 of "123456789" is the algorithm's check value, 0xCBF43926.
        assert_eq!(lock_id("123456789"), 0x3d32_ad9e * 0xCBF4_3926_i64);
    }
}
