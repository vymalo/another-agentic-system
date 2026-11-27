//! [`RunBindingStore`] over Postgres: the run leases of B1 (ADR 0084), one row per run. A run that
//! ends keeps its row as a tombstone (`ended_at`, migration `0035`, ADR 0108 decision 6).

use jiff::Timestamp;
use orch_core::{AgentId, UserId};
use orch_ports::{RunBinding, RunBindingStore, RunId, RunLeaseError};

use crate::codec::to_db;
use crate::error::SqlError;
use deadpool_postgres::Object;

use crate::sql::{PgPool, query, query_as};

/// The run bindings in the orchestrator's own Postgres: one row per run in `run_leases`, created by
/// [`PgStore::migrate`](crate::PgStore::migrate) (migration `0030`; `ended_at` is `0035`).
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
             ON CONFLICT (run) DO UPDATE SET model = EXCLUDED.model, \
                 expires_at = EXCLUDED.expires_at, ended_at = NULL \
             WHERE run_leases.agent = EXCLUDED.agent \
               AND run_leases.person = EXCLUDED.person \
               AND (run_leases.ended_at IS NOT NULL \
                    OR run_leases.model IS NOT DISTINCT FROM EXCLUDED.model) \
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
        if written.is_none() {
            // A different binding for a run that ended: the run id is the agent's to choose, so the
            // tombstone is never handed to another person (ADR 0108 decision 6). It is dropped, the
            // run is unattributed and the bind is refused: fail closed. A live row is not touched.
            let dropped = query("DELETE FROM run_leases WHERE run = $1 AND ended_at IS NOT NULL")
                .bind(binding.run.as_str())
                .execute(&mut *tx)
                .await;
            if let Err(err) = dropped {
                let _ = tx.rollback().await;
                return Err(unavailable(err));
            }
        }
        tx.commit().await.map_err(unavailable)?;
        match written {
            Some(_) => Ok(()),
            None => Err(RunLeaseError::Conflict),
        }
    }

    async fn end(
        &self,
        run: &RunId,
        ended_at: Timestamp,
        sweep_before: Timestamp,
    ) -> Result<(), RunLeaseError> {
        let mut conn = Object::take(self.pool.acquire().await.map_err(unavailable)?);
        // The two statements are each idempotent and need no transaction: a crash between them
        // leaves a tombstone the next `end` sweeps, and `person_of` filters by the bound anyway.
        // The first end's time stands: a second end does not extend the tombstone.
        query("UPDATE run_leases SET ended_at = $2 WHERE run = $1 AND ended_at IS NULL")
            .bind(run.as_str())
            .bind(to_db(ended_at))
            .execute(&mut *conn)
            .await
            .map_err(unavailable)?;
        // The sweep: a tombstone past the bound is gone (the partial index `run_leases_ended_at`).
        // `<=` as `person_of`'s `>`: answered while `ended_at > bound`, swept at or before it, so a
        // bound of zero deletes the run that just ended.
        query("DELETE FROM run_leases WHERE ended_at IS NOT NULL AND ended_at <= $1")
            .bind(to_db(sweep_before))
            .execute(&mut *conn)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    async fn get(&self, run: &RunId) -> Result<Option<RunBinding>, RunLeaseError> {
        let mut conn = Object::take(self.pool.acquire().await.map_err(unavailable)?);
        let row: Result<Option<BindingRow>, SqlError> = query_as(
            "SELECT agent, person, model, expires_at FROM run_leases \
             WHERE run = $1 AND ended_at IS NULL",
        )
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

    async fn person_of(
        &self,
        run: &RunId,
        ended_after: Timestamp,
    ) -> Result<Option<UserId>, RunLeaseError> {
        let mut conn = Object::take(self.pool.acquire().await.map_err(unavailable)?);
        let row: Result<Option<(String,)>, SqlError> = query_as(
            "SELECT person FROM run_leases \
             WHERE run = $1 AND (ended_at IS NULL OR ended_at > $2)",
        )
        .bind(run.as_str())
        .bind(to_db(ended_after))
        .fetch_optional(&mut *conn)
        .await;
        Ok(row
            .map_err(unavailable)?
            .map(|(person,)| UserId::new(&person)))
    }
}
