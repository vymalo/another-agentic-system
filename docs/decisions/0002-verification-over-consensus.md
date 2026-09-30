# ADR 0002 — Verification over consensus

- **Status:** accepted (2026-09-28). Status note (2026-09-30): [ADR 0018](0018-verification-gate-and-rework-loop.md)
  makes the verify step and the attempt budget concrete (planned, not built): a configurable gate over
  CI, agent-reported checks and a verifier agent, 3 attempts by default. It also answers open question 8
  and the attempts and wall-clock part of question 6; token budgets stay open. The decision stands.

## Context

The goal is "agents sit together and craft the perfect solution". Agents
discussing among themselves converge on something *plausible*, and each round
multiplies token cost.

## Decision

Quality is decided by an **external judge**: tests, typecheck, lints, CI. The
loop is work → verify → rework (with findings) until green, bounded by
budgets. Review agents critique a diff that already passes. A job never ends
"done" while its checks are red.

## Consequences

- Every job needs acceptance criteria the verifier can check; the planner's
  first output is criteria, not code.
- Budgets (attempts, tokens, wall clock) are part of job state; exhausting one
  ends in `Failed` with the findings in the chat.
