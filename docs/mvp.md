# MVP — build order

Smallest working loop first. Each step is usable on its own; nothing
multi-agent until one agent works end to end.

This system needs agents to drive but does not host them. The first coding
agent is built **once** as another-agentic-platform's scenario-B harness
(ADK-Rust + `opencode acp`, toolchain image from vymalo/another-agentic-images) and
run **standalone** until the platform exists — the same A2A endpoint either
way.

*Note (2026-09-29):* the first agent is adam-coder from
[`vymalo/another-adam-rs`](https://github.com/vymalo/another-adam-rs), a plain A2A agent listed
first in `AGENTS_FILE` ([ADR 0014](decisions/0014-adam-coder-default-agent-over-a2a.md)). The
platform harness remains the way to host agents later.

| Step | Delivers | Done when | Status (checked against the code, 2026-09-29) |
|---|---|---|---|
| 1. **Skeleton** | Postgres schema (jobs, inbox, outbox, events, timers), stateless orchestrator, chat surface; model calls to a configured OpenAI-compatible endpoint | A job typed in the chat appears in the event log, and a second orchestrator replica takes over when the first is killed. | **Built, except the model endpoint.** Schema: `threads` (with the job ledger), `events`, `a2a_bindings`, `outbox`, `inbox` and `watches` (timers are inbox rows); there is no job table: a thread is the unit. Orchestrator, resource API, AG-UI surface and `web/` exist. "Second replica takes over" is tested: `orch-e2e` `restart` and `replicas`, and the binary's SIGKILL smoke test. Nothing calls a model yet. |
| 2. **One agent, end to end** | Chat → orchestrator → one A2A coding agent → pushed branch → CI result (webhook) streamed into the chat | A job ends as a pushed branch plus a CI result card, and survives an orchestrator restart mid-job. | **Built, except the CI webhook.** Chat → orchestrator → one A2A agent works end to end, streams its status and artifacts into the chat, and survives a restart mid-task (`orch-e2e` `restart`). The pushed branch is the agent's job; the webhook input and the CI result card are not built. **Planned** (2026-09-30): CI results arrive as a signed webhook, GitHub or a generic shape ([ADR 0017](decisions/0017-ci-results-by-webhook.md), [`api/webhooks.md`](api/webhooks.md)), through the inbox ([ADR 0016](decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md)); slices 5 to 9 below. |
| 3. **Verify/rework loop** | Checks gate the job; failures loop back with findings, bounded by attempts | A deliberately failing task reworks and goes green, or ends `Failed` with findings — never "done" while red. | **Built for the agent's own checks (2026-09-30, slices 2 and 3) and for a verifier agent (2026-09-30, slice 10), not yet for CI.** [ADR 0018](decisions/0018-verification-gate-and-rework-loop.md): the gate, the rework and the attempts are in the pure core and configurable (`ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_MAX_ATTEMPTS_CAP`, an `AGENTS_FILE` `gate`, `forwardedProps["vymalo.gate"]`); a run stays open while the job is verified, and `orch-e2e` proves red once → rework in a new task → green, and red always → `Failed` with `checks_failed`. With a verifier (`ORCH_VERIFIER`, `ORCH_VERIFIER_TIMEOUT_SECS`) the dispatcher asks the verifier agent over A2A and its `verdict` decides, the same way, and a verifier that is down or too slow holds the thread (`Blocked`, no attempt spent). CI is refused by configuration (exit 78, a 400) until slices 5 and 6 of the CI plan. **The web renders it (slice 4, 2026-09-30):** a `verifying` badge, the attempt counter, a card per source with its findings, the rework divider and "Checks failed after N attempts". |
| 4. **Planner + parallel agents** | A planner agent splits the job; agents run in parallel; results merge into one PR | A two-part task produces two branches worked in parallel and one PR. | Planned |
| 5. **Reviewers** | Any A2A agents as reviewers | A reviewer's change request sends the job back to `Working` with its findings. | Planned |
| 6. **More inputs/outputs** | MCP server (`start_job`), GitHub/Slack webhooks, timers | A job started from Claude Code via MCP reports progress back to Claude Code. | Planned, designed 2026-09-30 ([ADR 0019](decisions/0019-mcp-server-over-streamable-http.md)): the MCP server over streamable HTTP with bearer tokens (OIDC later), slices 11, 12 and 14 below. *Amended:* the note "also needs the inbox" no longer holds for MCP, which goes straight to `App`; the inbox and timers ([ADR 0016](decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md)) serve the webhooks and the CI deadline, slices 5 to 9. GitHub webhooks are designed for CI results only; Slack webhooks are not designed. |
| 7. **Platform integration** | Release picker via the release-channels A2A extension (ADR 0008) | For a platform-hosted target, the dropdown lists channels and revisions from the live agent card, and the chat shows the revision that actually ran; a plain A2A target shows no picker. | **The picker is built, ahead of order**, and tested against the WireMock and in-process agents only; against a real platform it is *unverified* (open question 10). |

Diagrams of what is built: [Architecture: as built](architecture.md#as-built).

## The slices of steps 2, 3 and 6

**Slices 2, 3, 4, 5, 6, 10, 11 and 12 are built (2026-09-30); the rest is planned, not built** (owner decisions and design of 2026-09-30: [ADR 0016](decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md),
[ADR 0017](decisions/0017-ci-results-by-webhook.md), [ADR 0018](decisions/0018-verification-gate-and-rework-loop.md),
[ADR 0019](decisions/0019-mcp-server-over-streamable-http.md)). One pull request each; size S < M < L.
Slice 1 is the documentation these ADRs are in.

| # | Slice | Size | Builds and tests | Depends on |
|---|---|---|---|---|
| 1 | `docs`: ADRs 0016–0019, this page, the orchestrator and architecture docs, open questions 6 and 8, [`api/webhooks.md`](api/webhooks.md) | S | Documents only | none |
| 2 | **Built.** Job ledger and the verification gate in the pure core | L | `Snapshot`/`Job`, `Verifying`, the agent-checks source, rework, attempts, the new events. Migration `0003`: `threads.job`, `state` and event-kind `CHECK`s widened to every new kind, outbox `verify` + `task_id`. Transition-table rows; property tests (never `Done` while a required source is failed or pending; attempts never exceed the maximum; terminal states absorb; replay is deterministic); store conformance for `job` | 1 |
| 3 | **Built.** Gate configuration and the AG-UI projection of verification | M | The gate variables, `AGENTS_FILE` `gate`, `forwardedProps["vymalo.gate"]`, `vymalo.check`/`vymalo.rework`, a run open while verifying, `chat-api.yaml`. Goldens `verify-green`, `verify-red`; conformance; end-to-end tests. Dev stack: mock-agent `red-once`/`red-always`, `mock-coder-gated`, `dev/verify-e2e.sh`. *As built:* `ci` and `verifier` were refused in every layer until their slices (they needed commands the application dropped); `pending_reason` in `orch-app` is the one place that decides, and slice 10 changed its `verifier` arm | 2 |
| 4 | **Built.** Web: verification states, attempt counter and findings | M | The `verifying` badge, the attempt counter, the `vymalo.check` card and `vymalo.rework` divider (findings as plain text, long ones expandable), "Checks failed after N attempts". The mock replays `verify-green` and `verify-red` (and the mock-only `verify-ci`, `verify-wait`). Unit and DOM tests (a reconnect mid-verification, findings as text), mock Playwright with axe, and the system suite against the real orchestrator with the gated fake agent | 3 |
| 5 | Inbox, watches and timers | M–L | **Built (2026-09-30).** Migration `0004`; `ThreadStore` inbox methods; `Commit.{watches,timers,inbox}`; `InboxWorker`; `Watch`/`Schedule`/`TimerFired`. Conformance (dedupe, park and re-arm in one transaction, fencing, expiry, a timer not due, a replayed commit arms no second timer); end-to-end on both stores: a deadline fires and blocks the thread, a restart mid-inbox, a report received before its watch. Configuration: `INBOX_LEASE_SECS`, `INBOX_POLL_SECS`, `INBOX_PARKED_TTL_SECS`, `INBOX_MAX_ATTEMPTS`. What differs from the design: [ADR 0016](decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md#built-in-slice-5) | 2 |
| 6 | **Built.** CI results in the core and the generic signed webhook | M–L | `CiReported`, `ci_result`, the CI source, the CI deadline to `Blocked` (the core was complete after slices 2 and 5; the slice's tests drive it through the route), `SurfaceRoutes::machine` (built with slice 11), the generic route of `orch-surface-webhook`, `ORCH_CI_TIMEOUT_SECS`, `WEBHOOK_GENERIC_SECRETS` / `_MAX_SKEW_SECS`; `ci` is honoured by the gate. HMAC and skew vectors ([`api/webhooks.md`](api/webhooks.md#known-answer-vectors)); 401 with no write; duplicate to 202; parked then matched, a red report reworks, a deadline blocks (end to end on both stores with a clock the test holds); a binary smoke test that posts a signed report. Dev stack: Caddy `handle /webhooks/*` without an identity, `mock-coder-ci`, `dev/ci-webhook.sh`, `dev/ci-e2e.sh`. What differs from the design: [ADR 0017](decisions/0017-ci-results-by-webhook.md#built-slice-6) | 5 |
| 7 | CI result card in AG-UI | S | The `vymalo.ci` activity, golden `ci.agui.json` | 6 |
| 8 | Web: CI result card | S | Conclusion badge, name, short SHA, link | 7 |
| 9 | GitHub webhook adapter | M | `/webhooks/github`, `ping`, the three events, recorded fixtures. Dev stack: `mock-ci`, coder `gate: {require: [ci]}`, the coder end-to-end script asserts the CI card and `done` | 6 |
| 10 | **Built.** Verifier agent in the gate | L | **Built (2026-09-30).** Outbox `verify` (`outbox.task_id` now used, `ThreadStore::mark_verify_sent`), the dispatcher's verifier path (read, never applied as the worker's; one `VerifierReported` from the `verdict` artifact, or `VerifierFailed`), the verifier deadline (`ORCH_VERIFIER_TIMEOUT_SECS`, the slice 5 timer), the verifier subagent in the projection; `ORCH_VERIFIER`, the `verifier` key and source honoured. Tests: dispatcher cases against a scripted verifier (verdicts, no verdict, a broken or unreachable verifier, a lost claim, a crash), `orch-e2e` `verifier` on both stores (findings → rework → pass at attempt 2, out of attempts, no verdict, a timeout to `Blocked`, a message that abandons a verification, a crash mid-verify with one verdict), a binary smoke test. Goldens `verify-verifier-green`, `verify-verifier-red`. Dev stack: `mock-verifier`, `mock-coder-verified`, `dev/verifier-e2e.sh`. What differs from the design: [ADR 0018](decisions/0018-verification-gate-and-rework-loop.md#built-slice-10) | 3, 5 |
| 11 | MCP server with `start_job` and static bearer tokens | L | **Built (2026-09-30).** `orch-surface-mcp`; `list_agents`, `start_job`, `get_job`, `answer`, `cancel_job`; `MCP_TOKENS_FILE`, `MCP_ALLOWED_HOSTS`, `ORCH_PUBLIC_URL`; `origin`; `SurfaceRoutes::machine`. 401 paths, idempotent `start_job`, an in-process rmcp client on the memory store and on Postgres, a binary smoke test. Dev stack: Caddy `handle @mcp`, `dev/mcp-tokens.yaml`, `dev/mcp.json.example`, `dev/mcp-e2e.sh`. rmcp's stateless mode is checked with rmcp's own client only; against Claude Code it is *unverified*. **Review fixes (2026-09-30):** `start_job.gate`, a retry that says something else is refused, job ids of UUID version 8 reserved, caps on open waits, a token-less wait capped at one heartbeat, `MCP_ALLOWED_ORIGINS` and the 32-byte token minimum (see the [ADR 0019 status note](decisions/0019-mcp-server-over-streamable-http.md#status-note-2026-09-30-review-fixes)) | 2 (3 for the gate) |
| 12 | `wait_for_job` with MCP progress notifications | M | **Built (2026-09-30).** Progress stream, heartbeat, timeout, shutdown; `MCP_WAIT_MAX_SECS`; increasing progress then the result; a timeout and a re-call with `after_seq` that lose nothing; a finished job at once; a shutdown; a replica killed mid-wait and the call repeated on another (two `App`s over one Postgres). The wait loop is tested on a paused clock. Dev stack: `dev/mcp-e2e.sh` reads the progress from the SSE response | 11 |
| 13 | Complete local stack for the MVP | M | The `app` profile stays offline and deterministic, one script per scenario (chat to PR; `red-once` reworks to green; `red-always` fails; verifier findings rework; MCP `start_job` with progress); an optional `compose.live.yaml` and `.env.example`; opt-in smee relay; the coder re-pinned | 4, 8, 9, 10, 12 |
| 14 | OIDC bearer tokens for MCP (after the MVP) | M | JWKS validation against WireMock | 11 |

After slice 2, {3 → 4}, {5 → 6 → 7, 8, 9} and {11 → 12} can run in parallel; 10 can start after 3 and
5. Migration numbers are fixed now (`0003` in slice 2, which widens every `CHECK` once; `0004` in
slice 5). Slice 2 blocks everything and changes the signature of `transition`, so its tests are the
largest change. The shared files are `config.rs`, `compose.yaml`, the Caddyfile, `dev/README.md` and
`transition.rs`: rebase before merging. **Cross-repository:** adam-coder must emit a `checks {passed,
commit, summary, findings}` artifact from `run_checks` (an adam-rs pull request); until it does, the
coder's dev gate is CI only.

## Beyond the numbered steps: AG-UI

The user-facing protocol moves to AG-UI 1.0 ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md),
binding in [`api/agui.md`](api/agui.md)). It is a set of slices, not an MVP step:

| Slice | Status |
|---|---|
| Wire types with schema conformance (`orch-agui-proto`) | Built |
| `message_id` and `run_id` in the event log, `auth_required` status | Built |
| Configuration with clap; surfaces mounted by `ORCH_SURFACES` (the chat API moved into `orch-surface-chat-api`, since removed) | Built |
| The pure projection, both directions (`orch-agui-projection`) | Built |
| Goldens read through the reference client in CI (`tools/agui-conformance`) | Built |
| The AG-UI run route (`orch-surface-agui`): `POST /agui/agents/{agentId}` | Built |
| The AG-UI connect stream and capabilities (`GET /agui/threads/{id}/connect`, `GET /agui/agents/{agentId}/capabilities`) | Built |
| The AG-UI operations in `chat-api.yaml` (the vendored schema by reference); the four legacy operations `deprecated: true` and answering with `Deprecation` (RFC 9745), until they were removed | Built |
| The web on `@assistant-ui/react-ag-ui` (`ThreadAgent` over the connect stream, live runs, interrupts by `resume`, patched `cancelled` outcome; the REST interaction path removed) | Built |
| The legacy chat API surface off by default (`ORCH_SURFACES` defaults to `agui`; `agui,chat-api` keeps the legacy routes; compose and the dev scripts run on AG-UI only) | Built |
| The legacy chat API surface removed (2026-09-30): the crate `orch-surface-chat-api`, its feature `surface-chat-api`, the four operations and their schemas in `chat-api.yaml`; `ORCH_SURFACES` naming `chat-api` fails closed at startup (exit 78) and points to AG-UI; the resource API stays | Built |
| A2UI on the orchestrator: surfaces from agents, actions from users, capability detection ([ADR 0013](decisions/0013-a2ui-generative-ui.md)) | Built |
| A2UI rendering in the web: the validator (64 KiB, 400 components, 2000 nodes after expansion, 100 per template, depth 24, a ten-component vocabulary, http(s) links only), the shadcn vocabulary, actions on a user gesture only, the mock and the system tests ([`web/README.md`](../web/README.md#a2ui-surfaces)) | Built |

## Out of scope for the MVP

- Hosting agents, sandboxes or runtimes (another-agentic-platform's job).
- Multi-tenant auth beyond "you, via Keycloak".
- Any UI beyond the chat surface and a job list.
