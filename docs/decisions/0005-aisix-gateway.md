# ADR 0005 — AISIX as the single LLM gateway

- **Status:** accepted (2026-09-28)

## Context

Planner, workers, reviewers and the orchestrator all call models, across
several providers. Multi-agent loops make cost easy to lose track of.

## Decision

Every model call goes through [AISIX](https://github.com/api7/aisix): one
OpenAI-compatible endpoint in front of all providers, with provider
credentials, model aliases, routing/failover, rate and token limits, and
observability in one place. Rust, single binary, control/data plane split.

## Consequences

- Agents are configured with gateway keys and model aliases, never provider
  keys.
- Per-job/per-agent budgets become enforceable and visible
  (see [open questions](../open-questions.md) #6).
- Subscription logins (Claude/Codex CLIs) bypass the gateway; any worker using
  them is an explicit exception.
