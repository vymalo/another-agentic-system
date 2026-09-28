# ADR 0002 — Verification over consensus

- **Status:** accepted (2026-09-28)

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
