//! A database brought to an older schema the way an older release did: the migrations of a directory
//! the test wrote, applied into the history every release keeps (`_sqlx_migrations`), by the runner
//! that continues sqlx's (`orch-pg-migrate`, whose equivalence with sqlx's migrator its own tests
//! prove).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::Path;

use deadpool_postgres::Object;
use orch_pg_migrate::{Migration, Migrator};
use orch_store_postgres::PgPool;

/// Applies the `*.sql` files of `dir`, in the order of their versions, on a connection taken out of
/// `pool` and closed after the run.
pub async fn migrate_dir(pool: &PgPool, dir: &Path) {
    let mut migrations = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if !name.ends_with(".sql") {
            continue;
        }
        // the runner keeps a migration's text for the life of the process, as `include_str!` does
        let sql: &'static str = Box::leak(std::fs::read_to_string(&path).unwrap().into_boxed_str());
        migrations.push(Migration::from_file(&name, sql).unwrap());
    }
    let migrator = Migrator::new(orch_store_postgres::MIGRATIONS_TABLE, migrations).unwrap();
    let mut client = Object::take(pool.acquire().await.unwrap());
    migrator.run(&mut client).await.unwrap();
}
