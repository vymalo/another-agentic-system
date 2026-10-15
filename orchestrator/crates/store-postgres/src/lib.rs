//! Postgres implementation of the orchestrator's [`ThreadStore`](orch_ports::ThreadStore),
//! [`Wakeup`](orch_ports::Wakeup) and
//! [`UserToolServerStore`](orch_ports::UserToolServerStore) and
//! [`PendingAuthorizations`](orch_ports::PendingAuthorizations) ports (ADR 0001, ADR 0009, ADR 0046).
//!
//! - [`PgStore`]: threads, the per-thread event log (`seq` from a counter row updated in the
//!   same transaction as the insert), the A2A binding and the outbox
//!   (`FOR UPDATE SKIP LOCKED` claims with leases). Migrations are embedded and applied by
//!   [`PgStore::migrate`], which every replica may run at boot.
//! - [`PgWakeup`]: `LISTEN/NOTIFY` fan-out; a reconnect or a lagging subscriber yields
//!   [`Topic::Resync`](orch_ports::Topic::Resync).
//! - [`PgUserToolServers`]: the definitions of people's own MCP servers, at most eight each (the
//!   limit kept under a lock of the owner), every write of one a compare and swap on a revision.
//!   Its table is made by the same migrations.
//! - [`PgPendingAuthorizations`]: the OAuth authorizations of those servers that have begun and whose
//!   callback has not come, by the SHA-256 of the `state`, taken once. Same migrations.
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
mod pending_authorizations;
mod store;
mod user_tool_servers;
mod wakeup;

pub use pending_authorizations::{PENDING_LOCK_TIMEOUT, PgPendingAuthorizations};
pub use store::PgStore;
pub use user_tool_servers::{LOCK_TIMEOUT, OWNER_LOCK_CLASS, PgUserToolServers};
pub use wakeup::PgWakeup;
