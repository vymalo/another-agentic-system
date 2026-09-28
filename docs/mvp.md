# MVP — build order

Smallest working loop first. Each step is usable on its own; nothing
multi-agent until one agent works end to end.

This system needs agents to drive but does not host them. The first coding
agent is built **once** as another-agentic-platform's scenario-B harness
(ADK-Rust + `opencode acp`, toolchain image from vymalo/another-agentic-images) and
run **standalone** until the platform exists — the same A2A endpoint either
way.

| Step | Delivers | Done when |
|---|---|---|
| 1. **Skeleton** | Postgres schema (jobs, inbox, outbox, events, timers), stateless orchestrator, chat surface; model calls to a configured OpenAI-compatible endpoint | A job typed in the chat appears in the event log, and a second orchestrator replica takes over when the first is killed. |
| 2. **One agent, end to end** | Chat → orchestrator → one A2A coding agent → pushed branch → CI result (webhook) streamed into the chat | A job ends as a pushed branch plus a CI result card, and survives an orchestrator restart mid-job. |
| 3. **Verify/rework loop** | Checks gate the job; failures loop back with findings, bounded by attempts | A deliberately failing task reworks and goes green, or ends `Failed` with findings — never "done" while red. |
| 4. **Planner + parallel agents** | A planner agent splits the job; agents run in parallel; results merge into one PR | A two-part task produces two branches worked in parallel and one PR. |
| 5. **Reviewers** | Any A2A agents as reviewers | A reviewer's change request sends the job back to `Working` with its findings. |
| 6. **More inputs/outputs** | MCP server (`start_job`), GitHub/Slack webhooks, timers | A job started from Claude Code via MCP reports progress back to Claude Code. |
| 7. **Platform integration** | Release picker via the release-channels A2A extension (ADR 0008) | For a platform-hosted target, the dropdown lists channels and revisions from the live agent card, and the chat shows the revision that actually ran; a plain A2A target shows no picker. |

## Out of scope for the MVP

- Hosting agents, sandboxes or runtimes (another-agentic-platform's job).
- Multi-tenant auth beyond "you, via Keycloak".
- Any UI beyond the chat surface and a job list.
