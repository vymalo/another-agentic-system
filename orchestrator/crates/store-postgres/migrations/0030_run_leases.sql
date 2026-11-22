-- The run leases of B1 (ADR 0084): the orchestrator binds (run, agent, person, model, deadline) at
-- run creation; the injector leases the model credential only to the bound agent. The binding is
-- not a secret; the credential it lends lives in the sealed store or the vault.
CREATE TABLE IF NOT EXISTS run_leases (
    run        text PRIMARY KEY,
    agent      text NOT NULL,
    person     text NOT NULL,
    model      text,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- The sweep of expired bindings, and the lookup by run (the primary key).
CREATE INDEX IF NOT EXISTS run_leases_expires_at ON run_leases (expires_at);
