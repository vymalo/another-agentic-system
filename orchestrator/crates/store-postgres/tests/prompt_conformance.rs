//! The prompt library's conformance suite (ADR 0070) against Postgres. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set; each case runs in its own schema. A file of its own: the macro
//! writes the names of its cases at the top of the module, and the tool servers' suite has some of them.
#![allow(missing_docs)]

mod support;

use orch_store_postgres::PgStore;
use support::TestDb;

async fn make_store() -> Option<PgStore> {
    Some(TestDb::new().await?.store().await)
}

orch_ports::prompt_store_conformance!(make_store);
