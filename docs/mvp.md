# MVP — build order

Smallest working loop first. Each step is usable on its own and is the
foundation for the next; nothing multi-agent until one agent works end to end.

| Step | Delivers | Done when |
|---|---|---|
| 1. **Gateway** | AISIX in the cluster; all model access goes through it | A request from inside the cluster reaches two providers through one endpoint, and usage shows up per key. |
| 2. **One worker, end to end** | Chat → orchestrator → one opencode worker in an ephemeral sandbox → branch → CI result streamed back to the chat | A job typed in the chat ends as a pushed branch plus a CI result card, and survives an orchestrator restart mid-job. |
| 3. **Verify/rework loop** | Checks gate the job; failures loop back with findings, bounded by attempts | A deliberately failing task reworks and goes green, or ends `Failed` with findings — never "done" while red. |
| 4. **Planner + parallel workers** | A planner splits the job; workers run in parallel; results merge into one PR | A two-part task produces two branches worked in parallel and one PR. |
| 5. **Reviewers + kagent** | Specialist agents (review, ops, research) on kagent, reached over A2A | A reviewer's change request sends the job back to `Working` with its findings. |
| 6. **More inputs/outputs** | MCP server (`start_job`), GitHub/Slack webhooks, timers | A job started from Claude Code via MCP reports progress back to Claude Code. |

## Out of scope for the MVP

- Multi-tenant auth beyond "you, via Keycloak".
- Cost optimisation beyond AISIX budgets.
- Any UI beyond the chat surface and a job list.
