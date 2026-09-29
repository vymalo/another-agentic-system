//! Postgres implementation of the orchestrator's [`ThreadStore`](orch_ports::ThreadStore) and
//! [`Wakeup`](orch_ports::Wakeup) ports (ADR 0001, ADR 0009).
//!
//! - [`PgStore`]: threads, the per-thread event log (`seq` from a counter row updated in the
//!   same transaction as the insert), the A2A binding and the outbox
//!   (`FOR UPDATE SKIP LOCKED` claims with leases). Migrations are embedded and applied by
//!   [`PgStore::migrate`], which every replica may run at boot.
//! - [`PgWakeup`]: `LISTEN/NOTIFY` fan-out; a reconnect or a lagging subscriber yields
//!   [`Topic::Resync`](orch_ports::Topic::Resync).
//!
//! Only runtime-checked `sqlx::query` is used, so building needs no database.
//!
//! ```no_run
//! # async fn boot(url: &str) -> Result<(), orch_ports::StoreError> {
//! use orch_store_postgres::{PgStore, PgWakeup};
//! let store = PgStore::connect(url).await?;
//! store.migrate().await?;
//! let wakeup = PgWakeup::start(store.pool().clone());
//! # let _ = wakeup;
//! # Ok(())
//! # }
//! ```

mod codec;
mod error;
mod store;
mod wakeup;

pub use store::PgStore;
pub use wakeup::PgWakeup;
