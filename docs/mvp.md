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
| 1. **Skeleton** | Postgres schema (jobs, inbox, outbox, events, timers), stateless orchestrator, chat surface; model calls to a configured OpenAI-compatible endpoint | A job typed in the chat appears in the event log, and a second orchestrator replica takes over when the first is killed. | **Built, except the model endpoint.** Schema: `threads`, `events`, `a2a_bindings`, `outbox` (no `inbox` or `timers` yet, no job table: a thread is the unit). Orchestrator, chat API and `web/` exist. "Second replica takes over" is tested: `orch-e2e` `restart` and `replicas`, and the binary's SIGKILL smoke test. Nothing calls a model yet. |
| 2. **One agent, end to end** | Chat → orchestrator → one A2A coding agent → pushed branch → CI result (webhook) streamed into the chat | A job ends as a pushed branch plus a CI result card, and survives an orchestrator restart mid-job. | **Built, except the CI webhook.** Chat → orchestrator → one A2A agent works end to end, streams its status and artifacts into the chat, and survives a restart mid-task (`orch-e2e` `restart`). The pushed branch is the agent's job; the webhook input and the CI result card are not built. |
| 3. **Verify/rework loop** | Checks gate the job; failures loop back with findings, bounded by attempts | A deliberately failing task reworks and goes green, or ends `Failed` with findings — never "done" while red. | Planned |
| 4. **Planner + parallel agents** | A planner agent splits the job; agents run in parallel; results merge into one PR | A two-part task produces two branches worked in parallel and one PR. | Planned |
| 5. **Reviewers** | Any A2A agents as reviewers | A reviewer's change request sends the job back to `Working` with its findings. | Planned |
| 6. **More inputs/outputs** | MCP server (`start_job`), GitHub/Slack webhooks, timers | A job started from Claude Code via MCP reports progress back to Claude Code. | Planned. Also needs the inbox ([orchestrator](orchestrator.md#event-flow)). |
| 7. **Platform integration** | Release picker via the release-channels A2A extension (ADR 0008) | For a platform-hosted target, the dropdown lists channels and revisions from the live agent card, and the chat shows the revision that actually ran; a plain A2A target shows no picker. | **The picker is built, ahead of order**, and tested against the WireMock and in-process agents only; against a real platform it is *unverified* (open question 10). |

Diagrams of what is built: [Architecture: as built](architecture.md#as-built).

## Beyond the numbered steps: AG-UI

The user-facing protocol moves to AG-UI 1.0 ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md),
binding in [`api/agui.md`](api/agui.md)). It is a set of slices, not an MVP step:

| Slice | Status |
|---|---|
| Wire types with schema conformance (`orch-agui-proto`) | Built |
| `message_id` and `run_id` in the event log, `auth_required` status | Built |
| Configuration with clap; surfaces mounted by `ORCH_SURFACES` (the chat API moved into `orch-surface-chat-api`) | Built |
| The pure projection, both directions (`orch-agui-projection`) | Built |
| Goldens read through the reference client in CI (`tools/agui-conformance`) | Built |
| The AG-UI run route (`orch-surface-agui`): `POST /agui/agents/{agentId}` | Built |
| The AG-UI connect stream and capabilities (`GET /agui/threads/{id}/connect`, `GET /agui/agents/{agentId}/capabilities`) | Planned |
| The web on `@assistant-ui/react-ag-ui`; deprecation markers on the chat API | Planned |
| A2UI generative UI ([ADR 0013](decisions/0013-a2ui-generative-ui.md)) | Planned |

## Out of scope for the MVP

- Hosting agents, sandboxes or runtimes (another-agentic-platform's job).
- Multi-tenant auth beyond "you, via Keycloak".
- Any UI beyond the chat surface and a job list.
