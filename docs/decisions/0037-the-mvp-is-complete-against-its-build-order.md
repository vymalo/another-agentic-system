# ADR 0037 — The MVP is complete against its build order

- **Status:** proposed (2026-10-03). It records what the planner means by "complete" and where the MVP stops; it becomes
  accepted when the owner has tried the stack and agrees, or is amended by a dated note listing what they found missing. Until
  then the owner's verdict of 2026-10-01, "the MVP is not ready", is the last word from the owner, and nothing here
  contradicts it: it answers the list that verdict started.

## Context

On 2026-10-01 the owner tested MVP steps 1 and 2 and judged them plumbing: "the adam part is not complete; the system part is
not even 10% in the direction I thought it would be" ([`vision.md`](../vision.md)). The page was rewritten from their words, the
build order was re-planned ([`mvp.md`](../mvp.md)), ADRs 0022 to 0026 were accepted on their delegation, and two further
rounds of feedback (2026-10-02) added requirements that became ADRs 0027 to 0036. Plan 11, "finish the MVP", built what was left:
tools per conversation, sending while an agent works, mentions in the API and the web, devcontainers, roles, files, and, last,
asked agents (the job ledger, the dispatcher, `ask_agent`, the web's nesting) and the football example as one scenario.

Without a statement of what "complete" means, the MVP never ends (each requirement of the vision has a larger version) or ends by
accident (the last open pull request merges). This ADR fixes the boundary so that a reader can check it.

## Decision

1. **The MVP is complete when every row of the requirement tables in [`vision.md`](../vision.md) is either built, with a
   scenario script or a test that asserts it, or listed as not built with a reason.** On 2026-10-03 that is the case: slices 1 to 10
   and 7b of the build order, and the requirements the owner added on 2026-10-02, are built; what is not built is the list below.
   The check is `vision.md`'s tables against `dev/e2e-all.sh`, `orchestrator/crates/e2e/tests/` and `web/e2e/`.
2. **"Built" means proven on mocks.** Agents are WireMock agents or adam agents on a scripted model, GitHub, the search and the
   registry are mocks, the identity provider is a mock issuer, and the web is tested against its own mock server. A live service
   is never needed to call a slice built, because the stack must stay offline and deterministic ([`dev/README.md`](../../dev/README.md)).
3. **Post-MVP, explicitly:**
   - the components List, Stepper, Agent suggestion, Skill request, Web view and Notification opt-in, and rich input other than a choice (questions 37 and 38 for the last two);
   - a person's own MCP server URL, and tool icons from a URL (question 38);
   - a **real browser agent** (none exists: the browser in the football example is a mock, the owner's decision), and the real researcher and coder in that example;
   - a planner agent and parallel agents, and reviewers (the first plan's steps 4 and 5; [ADR 0026](0026-agent-mentions-as-structured-references.md) option B);
   - a read of a real platform (it has no code; question 10), Kubernetes devcontainers (question 41), publishing the coder's checks as GitHub check runs, and a job that pushes to several repositories under the gate (question 42);
   - thread deletion and the retention of files and journals (questions 28, 29 and 46), hot reload of the configuration (43 and 44), token budgets (6), trace context (27), Slack webhooks, an explicit default agent (23);
   - **live verification**: the stack against a real model, GitHub.com (a token and an App), a search provider, an identity provider and a real browser.
4. **Completion is not acceptance.** The owner decides whether it is the system they meant. A finding becomes a slice in
   [`mvp.md`](../mvp.md) and a dated status note on this ADR.

## Consequences

- `README.md`, `CLAUDE.md`, `vision.md` and `mvp.md` say the same thing: complete against the build order, owner review pending,
  everything proven on mocks. None of them says "done" without the second half.
- A new requirement from the owner is a new slice, not a reopening of this ADR; a requirement moved from the post-MVP list to
  the build order gets a dated note here.
- The post-MVP list is the only place an unbuilt thing is allowed to hide, and it names a reason for each entry; `tools/docs-check`
  cannot check that, so review does.
- Anything the owner finds missing in the first live use is expected, not a regression of this decision: the live checks are on the list.

## Alternatives rejected

- **Wait for live verification before saying "complete".** No credentials, platform or browser agent exist to verify against, and
  the offline stack is the only thing CI can run; the claim would never be made. The honest form is to say what was not run.
- **Mark it accepted on the planner's own authority.** The owner's judgement is the only acceptance, and their last verdict was
  "not ready"; a record that said otherwise would overwrite it.
- **No ADR.** The boundary is a decision (what is out, and why), and four documents repeat it; one place keeps them from drifting.
