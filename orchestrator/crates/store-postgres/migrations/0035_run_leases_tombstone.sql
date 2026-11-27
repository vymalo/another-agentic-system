-- The run -> account tombstone (ADR 0107 decision 6). A run that ends keeps its row, marked ended,
-- so the operator's meter can still ask whom a run pod's last minutes are charged to: a pod lives up
-- to 900 s idle after its job and the meter backfills up to 24 hours. Only the account route reads an
-- ended row; the lease and the live binding never do, so no credential outlives the job. The broker
-- deletes ended rows older than its bound whenever it ends a run.
ALTER TABLE run_leases ADD COLUMN IF NOT EXISTS ended_at timestamptz;

-- The sweep of old tombstones.
CREATE INDEX IF NOT EXISTS run_leases_ended_at ON run_leases (ended_at) WHERE ended_at IS NOT NULL;
