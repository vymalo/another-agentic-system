# orch-secrets-openbao

`SecretStore` (ADR 0046, ADR 0009) in a self-hosted OpenBao or Vault **KV v2** mount. The Postgres sealed store
(`orch-secrets-pg`) stays the local and test adapter.

| Call | Does |
|---|---|
| `put` | plain KV write; KV numbers writers of a path as they land |
| `get` | read; 404 or a tombstone is `None` |
| `delete` | writes a tombstone version, then destroys the older ones; idempotent |
| `rotate` | KV write with `cas` = expected version; a moved path is `Conflict` |

Versions are KV's own and are never reset (the adapter never deletes a path's metadata), so a version is not given twice
across a `delete`. A path is named by `sha256(owner, server)`; the same hash is stored in the secret and checked on read.
Errors hold a status code at most: no URL, path, token, owner or material.

## Tests

`tests/conformance.rs` runs `secret_store_conformance!`. Skipped unless `ORCH_TEST_OPENBAO_URL` is set
(`ORCH_TEST_OPENBAO_TOKEN`, default `dev-only-token`). Start a dev server with
`docker compose --profile openbao up -d --wait openbao`. CI starts one as a service of the `orchestrator.yml` Postgres job.

**Status: unverified.** The crate compiles; the testkit has not been run against a live OpenBao (no container daemon
where it was written). Not yet built: the `secrets.store: openbao` configuration, its composition in the binary, the
chart value, and the `VaultStore` port (named secrets) on OpenBao.
