# ADR 0005 — Model access through any OpenAI-compatible endpoint

- **Status:** accepted (2026-09-28). Amends the earlier "AISIX as the single LLM gateway". Amended (2026-10-01): the orchestrator makes its first model call, thread titles, through the `ChatModel` port (status note at the end).

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

## Status note, 2026-10-01: the orchestrator's first model call, thread titles

The context said the orchestrator "makes a few model calls (summaries, routing)"; until MVP slice 6 it made none.
The first is the **title of a thread** ([`vision.md`](../vision.md): a chat list is read by its titles, and the first
words of a first message are a poor one). It is built as the decision says and as [ADR 0009](0009-swappable-implementations-at-build-time.md)
asks of every boundary:

- **One port, one adapter.** `orch_ports::ChatModel` is one question and its answer (`ChatRequest { model, system,
  user, max_tokens }` to a `String`, or a `ModelError` whose class says whether asking again helps), with `NoModel`
  for a deployment that has none and a scripted model for tests. `orch-model-openai` is the adapter: one JSON
  `POST {base}/chat/completions` with `stream: false`, over `reqwest`, no vendor SDK and no gateway-specific header
  ([ADR 0007](0007-protocol-only-dependencies.md)). The key is a sensitive bearer header, in no error and no `Debug`,
  and a redirect is never followed.
- **Configuration is the endpoint, not a product:** `ORCH_MODEL_BASE_URL` (up to and not including
  `/chat/completions`), `ORCH_MODEL_API_KEY`, `ORCH_TITLE_MODEL` (the model's name there; **unset turns titles off**,
  and then no model is ever asked) and `ORCH_MODEL_TIMEOUT_SECS` (20). Which gateway sits behind the URL stays a
  deployment choice. The same endpoint and key as the agents' is the usual setup (`compose.live.yaml`).
- **What it may do.** After the agent's first reply (a final message, or a status that ends or interrupts the turn with
  words) the core asks for a title (`Command::RequestTitle`, an outbox row of kind `title`), at most **twice** per
  thread, the second time only after the first was answered with none; the answer comes back as `Input::Titled` or
  `Input::TitleDeclined`, and a title the model wrote is a `thread_titled` event (`source: model`) and the thread's
  title. **A person's rename is final** (`source: user`): the model is never asked once a person has renamed, and a
  title the model wrote after is dropped. A thread keeps the first words of its first message when the model has no
  topic yet (`NONE`), is not configured, fails or is slow: **a title is a nicety and never fails a thread.**
- **The conversation is untrusted data going out, and the answer untrusted data coming back.** The model is shown the
  first six messages of the people and the agent (each cut at 500 characters, 4 KiB in all) in a code fence their text
  cannot close, told they are data and never instructions; what it says is cut to its first line of plain text, 80
  characters at most, with the quotes, markdown and control characters taken off, and the core checks the title again
  before it is logged. A title is rendered as text by every screen.
- **What stays as decided:** the agents' model access is their host's concern; token and cost budgets are still the
  gateway's. This call is small and bounded (a few hundred tokens, at most three tries, 20 seconds each, at most two
  asks per thread).

