# ADR 0005 — Model access through any OpenAI-compatible endpoint

- **Status:** accepted (2026-09-28). Amends the earlier "AISIX as the single LLM gateway".

## Context

The orchestrator itself makes a few model calls (summaries, routing), and the
agents it drives make many. Gateways such as EAIG (Envoy AI Gateway, now
**Agent Router**) and AISIX all expose OpenAI-compatible endpoints and add
routing, credentials, token limits and observability.

## Decision

This system depends on **an OpenAI-compatible endpoint**, configured by URL
and key — never on a specific gateway product. Which gateway sits behind that
URL (EAIG / Agent Router, AISIX, or a provider directly in development) is a
deployment choice.

## Consequences

- No gateway-specific client code, headers or CRDs in this repository.
- Token and cost budgets are read from the gateway's usage reporting when
  available (see [open questions](../open-questions.md) #6); the orchestrator
  enforces attempts and wall-clock budgets itself.
- Agents this system calls over A2A choose their own model access; that is
  their host's concern (e.g. another-agentic-platform AD-018).
