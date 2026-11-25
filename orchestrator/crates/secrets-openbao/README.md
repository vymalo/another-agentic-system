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

`OpenBaoConfig` has `address`, `mount`, `prefix`, `token`, `timeout` and `ca_pem` (PEM certificates added to the system's roots; a
bundle that is not PEM is `Unavailable` with no text of it). The binary builds it from `secrets.store: openbao`
([`docs/api/config.md`](../../../docs/api/config.md#secrets)): the prefix is fixed to `orchestrator` and the timeout to 10 s.

## Tests

`tests/client.rs` needs no server: a CA bundle is accepted or refused, and no error or `Debug` holds the token.
`tests/conformance.rs` runs `secret_store_conformance!`. Skipped unless `ORCH_TEST_OPENBAO_URL` is set
(`ORCH_TEST_OPENBAO_TOKEN`, default `dev-only-token`). Start a dev server with
`docker compose --profile openbao up -d --wait openbao`. CI starts one as a service of the `orchestrator.yml` Postgres job.

**Status: unverified.** The crate compiles; the testkit has not been run against a live OpenBao (no container daemon
where it was written). The configuration (`secrets.store: openbao`) and its composition in the binary are built; not yet built:
the `VaultStore` port (named secrets) on OpenBao.
