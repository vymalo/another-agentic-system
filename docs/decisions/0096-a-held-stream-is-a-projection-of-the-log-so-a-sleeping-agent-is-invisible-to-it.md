# ADR 0096 — A held stream is a projection of the log, so an agent sleeping is invisible to it; the activator is what a wake costs

- **Status:** **Proposed (2026-11-25).** Third gap of the live-connections epic ([#403](https://github.com/vymalo/another-agentic-system/issues/403)). It **consumes** the platform's plan for sleep when idle (AD-058, [plat#83](https://github.com/vymalo/another-agentic-platform/pull/83), ANR-255) and changes none of its decisions. Nothing is built.
- **It waits on nothing:** the plan it cites is merged, and the seam it names is the activator.

## Context

Today a client's stream already outlives the agent task it is showing. The AG-UI stream is a **projection of the log** (`docs/api/agui.md`, ADR 0059): the orchestrator renders the thread's events, so a task that ends, a worker that restarts or a client that reconnects changes what the stream *says*, not whether it exists.

The platform now plans the case that breaks the assumption underneath that: an agent whose workload goes to **zero** when it is idle, woken by an **activator** that holds a request while the operator scales it back (AD-058: the agent reports `busy`/`nextDueAt` from its own store; the activator answers the agent card without waking; a held request either resolves or answers `503` with `Retry-After` at `wakeTimeout`; `Sleeping` is a phase of its own and `spec.suspend` is never woken).

Those two facts fit together better than they look. **Because the stream is a projection, sleep is invisible to the client's connection**: there is nothing for the orchestrator to hold open on the agent's behalf and nothing for a client to reconnect to. What sleep does cost is a **dispatch**, and that is what this ADR has to specify.

## Decision

1. **The stream belongs to the log, not to the agent.** No new connection, no long-poll to the platform, no change to what a client holds while an agent sleeps. The orchestrator keeps projecting; a thread whose agent is asleep is simply a thread with no new events.
2. **A wake is a dispatch, and the orchestrator treats it like any other send with a budget.** The person's message is logged (`user_message`), the thread goes `queued`, and the dispatcher sends to the agent's A2A endpoint — where the activator may **hold** the request or answer **`503` with `Retry-After`**. The orchestrator retries inside its **existing** dispatch budget: no new timer, no platform API, no polling. A `503` is a retryable failure, not a thread failure.
3. **What the client sees during a wake is a state, a time limit, and a readable error.** The thread's status carries **`waking`** while a dispatch is being retried against a sleeping agent (mapped to the AG-UI run state a client already knows, plus one custom event so a client can say *why* it is waiting). The bound is the plan's **`wakeTimeout`**, surfaced in `GET /api/config` as a capability (`agents.sleep.wakeTimeoutSecs`), announced per ADR 0067's client-agnostic rule so a client that does not know it renders the ordinary queued state and loses nothing. If the wake does not happen within the budget the thread goes **`failed`** with an error a client can read — *"the agent did not wake within N seconds"* — never a silent hang and never a raw `503`.
4. **The orchestrator never asks the platform whether an agent is asleep.** Its only evidence is what the send answers: a hold that resolves, a `503` that retries, a success, or a failure. That keeps the two repositories coupled by A2A alone, which is the platform's own rule.
5. **adam needs nothing for the stream, and one row for the wake.** Its A2A send must be willing to wait at least `wakeTimeout`; if its client timeout is shorter, that is a row on adam's side (its own card), because a send that gives up first would report a failure for an agent that was waking correctly. Nothing else in adam changes: it does not know it was asleep, which is the point.
6. **`spec.suspend` is never woken** (AD-058 decision 5). A suspended agent answers `503` for a different reason and the error names it: a person suspended it, and a retry will not help.

## Slices

One row per pull request, one repository per row, each with its decisive test.

| Slice | What it delivers | Repository | Decisive test |
|---|---|---|---|
| S1 | The `waking` state, its AG-UI mapping and custom event, the readable failure, and `agents.sleep.wakeTimeoutSecs` in `GET /api/config` | sys | **a stream stays open across the agent going to zero and back, on mocks** — the epic's decisive check |
| S2 | The dispatch retry across both answers the activator gives: a held request that resolves, and a `503` + `Retry-After` that succeeds on the retry | sys | a `503` inside the budget ends as a delivered message, and one outside it ends as the readable failure — both seen red first |
| S3 | adam's send timeout against `wakeTimeout` — **only if S2 shows it is shorter**; a row on adam's board either way | adam | a send that outlasts a wake of `wakeTimeout` returns the agent's answer, not a timeout |

The platform's own rows (A1–A3, P1–P4 in AD-058's plan) are not duplicated here.

## Consequences

- **The epic's premise is nearly free.** Because the stream is already a projection, the *connection* half of "live connections that survive sleep" needs no work at all, and the remaining cost is one state, one capability and one retry policy.
- **A client that knows nothing about sleep sees what it sees today**, which is the client-agnostic rule working: `waking` degrades to `queued`.
- **The failure mode worth naming:** a wake that silently never resolves. Decision 3 bounds it with a timeout and a readable error, and decision 4 forbids asking the platform, so the only way to know is the answer to the send — which is also the only thing that matters.

## Alternatives rejected

- **Keeping a connection to the agent open so a client's stream "survives".** The stream does not depend on the agent's connection, and a connection held to a sleeping agent is exactly what the platform's plan exists to avoid.
- **The orchestrator polling the platform for an agent's phase.** It would couple the two repositories by an API rather than by A2A, and it would tell the orchestrator something it cannot act on.
- **Treating a `503` from the activator as a thread failure.** A wake is normal operation for a sleeping agent; failing the thread on the first `503` would make sleep visible as an error.
