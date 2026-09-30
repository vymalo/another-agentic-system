//! The webhook surfaces: CI systems report the result of a check on a pushed commit to the
//! orchestrator, signed, and the report becomes an inbox row that a worker applies to the job
//! that pushed the commit (ADR 0017, `docs/api/webhooks.md`).
//!
//! # Machine routes
//!
//! These are the only routes without a user. They are [machine routes](orch_api::SurfaceRoutes::machine):
//! mounted outside the identity layer, never reading `X-Auth-Request-Email`, each wrapped in a
//! guard that checks the signature **before anything is written** and fails closed. The edge
//! must route `/webhooks/*` to the orchestrator without injecting an identity header.
//!
//! # The generic route
//!
//! [`generic::routes`] serves `POST /webhooks/ci` (ORCH_SURFACES name `webhook-generic`): a JSON
//! body `{version: 1, repository, sha, branch?, name, conclusion, url?, summary?}`, signed with
//! HMAC-SHA-256 over `"<timestamp>.<body>"`, with a delivery id for idempotency. See
//! [`generic`] for the order of the checks and [`signature`] for the scheme.
//!
//! # The GitHub route
//!
//! [`github::routes`] serves `POST /webhooks/github` (`webhook-github`): GitHub's own deliveries,
//! signed with `X-Hub-Signature-256` over the raw body. `ping` is 204, `check_suite`, `check_run`
//! and `workflow_run` with `action` = `completed` become the same report as the generic body,
//! and every other event is acknowledged with 202 and not stored. See [`github`].
//!
//! Nothing here decides anything about a job. A route authenticates a delivery, normalises it to
//! a [`CiReport`](orch_core::CiReport) and calls [`App::receive`](orch_app::App::receive); the
//! inbox worker and the pure core do the rest. A valid report can add a check result to a job
//! that asked for that very commit, and nothing else: it cannot start a job, approve or merge.
//!
//! The crate depends on `App`, `orch-core` and the shared HTTP pieces of `orch-api` only, so a
//! webhook surface can be switched off (the binary's Cargo feature `surface-webhook`) or replaced
//! without touching the rest.

pub mod generic;
pub mod github;
mod secrets;
pub mod signature;
mod wire;

pub use generic::GenericConfig;
pub use github::GithubConfig;
pub use secrets::{MAX_SECRETS, Secrets, SecretsError};
