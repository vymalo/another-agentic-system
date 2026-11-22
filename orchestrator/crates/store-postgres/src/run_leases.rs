//! [`RunBindingStore`] over Postgres: the run leases of B1 (ADR 0084), one row per run.

use jiff::Timestamp;
use orch_core::{AgentId, UserId};
use orch_ports::{RunBinding, RunBindingStore, RunId, RunLeaseError};

use crate::codec::to_db;
use crate::error::SqlError;
use deadpool_postgres::Object;

use crate::sql::{PgPool, query, query_as};

/// The run bindings in the orchestrator's own Postgres: one row per run in `run_leases`, created by
/// [`PgStore::migrate`](crate::PgStore::migrate) (migration `0030`).
///
/// Cheap to clone: clones share the pool. **It holds no secret**: the row names the run, its agent,
/// its person, the model and the deadline.
#[derive(Clone, Debug)]
pub struct PgRunBindings {
    pool: PgPool,
}

impl PgRunBindings {
    /// The store over `pool` (the orchestrator's own, shared with the thread store, or one that pins
    /// a `search_path` in a test). The table is created by [`PgStore::migrate`](crate::PgStore::migrate);
    /// this does not migrate.
    pub fn from_pool(pool: PgPool) -> Self {
        PgRunBindings { pool }
    }

    /// The underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// The columns of a binding: agent, person, model, deadline.
type BindingRow = (String, String, Option<String>, Timestamp);

fn unavailable(err: SqlError) -> RunLeaseError {
    RunLeaseError::Unavailable(err.to_string())
}

impl RunBindingStore for PgRunBindings {
    async fn upsert(&self, binding: &RunBinding) -> Result<(), RunLeaseError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await
            .map_err(unavailable)?;
        let written: Result<Option<(String,)>, SqlError> = query_as(
            "INSERT INTO run_leases (run, agent, person, model, expires_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (run) DO UPDATE SET expires_at = EXCLUDED.expires_at \
             WHERE run_leases.agent = EXCLUDED.agent \
               AND run_leases.person = EXCLUDED.person \
               AND run_leases.model IS NOT DISTINCT FROM EXCLUDED.model \
             RETURNING run",
        )
        .bind(binding.run.as_str())
        .bind(&binding.agent.0)
        .bind(binding.person.as_str())
        .bind(&binding.model)
        .bind(to_db(binding.expires_at))
        .fetch_optional(&mut *tx)
        .await;
        let written = match written {
            Ok(row) => row,
            Err(err) => {
                let _ = tx.rollback().await;
                return Err(unavailable(err));
            }
        };
        tx.commit().await.map_err(unavailable)?;
        match written {
            Some(_) => Ok(()),
            None => Err(RunLeaseError::Conflict),
        }
    }

    async fn delete(&self, run: &RunId) -> Result<(), RunLeaseError> {
        let mut conn = Object::take(self.pool.acquire().await.map_err(unavailable)?);
        query("DELETE FROM run_leases WHERE run = $1")
            .bind(run.as_str())
            .execute(&mut *conn)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    async fn get(&self, run: &RunId) -> Result<Option<RunBinding>, RunLeaseError> {
        let mut conn = Object::take(self.pool.acquire().await.map_err(unavailable)?);
        let row: Result<Option<BindingRow>, SqlError> =
            query_as("SELECT agent, person, model, expires_at FROM run_leases WHERE run = $1")
                .bind(run.as_str())
                .fetch_optional(&mut *conn)
                .await;
        let row = row.map_err(unavailable)?;
        Ok(row.map(|(agent, person, model, expires_at)| RunBinding {
            run: run.clone(),
            agent: AgentId(agent),
            person: UserId::new(&person),
            model,
            expires_at,
        }))
    }
}
