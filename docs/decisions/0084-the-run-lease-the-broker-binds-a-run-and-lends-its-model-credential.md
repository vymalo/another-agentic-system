# ADR 0084 — The run lease: the broker binds a run and lends its model credential to the injector

- **Status:** **proposed (2026-11-21); decided 2026-11-22.** The owner answered the two open questions on
  2026-11-22: the gateway injects **the deployment's provider key**, leased per run and metered against the
  person's balance; the lease is **30 s, renewed by Authorino's cache**. Both are this record's
  recommendations; the code follows them. It fixes the contract of sys#334's slice **B1** (the broker's first
  part) that S9's injector consumes. The requirements are the epic's own (*Key Assumptions* 3, 14 and 15, and
  the constraint that the lease route is not at the edge).

## Context

S8 makes every model call of a run leave adam with **no key of its own**, tagged with `X-Run-Id` (the root run's id,
ADR 0067 decision 1; subagents and child sessions carry the same). S9 puts **Agent Router** in front of the model
endpoint and **Authorino** in front of Agent Router's routes; Authorino authenticates the agent pod, asks the broker for
the run's credential, adds `Authorization` upstream only, caches it per run within the lease, and strips a client-sent
`Authorization`.

The missing half is the broker. Today `orch-credential-broker` serves **people's own MCP servers**: `store`,
`begin_oauth`, `complete_oauth`, `lease`, `revoke`, keyed by a user tool server; it has no notion of a run. The epic
splits the broker's first part out ahead of hooks (B1): **its store, the lease contract, and an internal lease lookup
absent from the edge**, keyed by the run id and the caller's identity, answering only when the caller is the agent the
run is bound to.

What binds this record: the value never travels through A2A, the chat, the event log, outbox, AG-UI frames, step
input/output, exports, shares, forks, the agent's store or the model's requests; only the vault keeps it, and the
injector holds it for at most 30 s to set a header. Fail closed: a refusal or an unreachable broker parks the run
(adam ADR 0067 decision 9). The broker is already the only holder of the sealed store, and the owner chose **OpenBao**
as the vault for real deployments, the broker's sealed store for small and development ones (epic *Key Assumptions* 14).

## Decision

1. **The orchestrator binds the run at its creation.** `POST /internal/runs` takes `{run, agent, person, model?,
   expiresAt}`. The broker keeps the binding (run id, agent id, person id, model endpoint name if given, expiry,
   revocation). The same binding sent again is **idempotent**; a different binding for a run id already bound is
   refused (`409`), because a run is bound once. `expiresAt` is the run's deadline, **capped by the broker** (24 h), and
   the binding is not a secret: it names people and agents, no value.

2. **The lookup answers the orchestrator and the injector — never an agent** (amended 2026-11-24; the original text
   read "only the bound agent's"). `GET /internal/runs/{run}/lease` takes the caller's identity from `RunCallerAuth`
   (decision 5). The broker answers only when all of these hold: the caller is `RunCaller::Orchestrator` or
   `RunCaller::Injector`, the binding is live (not expired, not revoked), and the deployment has a credential for the
   run's model. An agent is refused **its own run included**: the binding names the agent, it does not authorise it, so
   a reachable agent holds no key. A refusal is one of the ruled slugs — `run_not_bound`, `lease_expired`,
   `unavailable` — and `model_locked` is **not** this route's (the balance is checked per call by the injector's
   authorization, not per lease).

3. **The lease is short and cannot outlive the binding.** The answer is a bearer token for the model endpoint and its
   expiry: at most **30 s** and never past the binding's. The response is `no-store`, the value is never logged, and
   `Debug` shows the expiry and the model, never the value. Authorino caches it per run for no longer than the lease, so
   a revoked binding stops spending within 30 s.

4. **One port, two stores.** A `RunLeases` port keeps the binding (the broker's Postgres store, beside its existing
   tables); a `ModelCredentials` port reads the value — **OpenBao** in production, the broker's **sealed store** in small
   and development deployments. A missing credential is `unavailable` (fail closed), never an empty header; the store's
   availability is the lookup's.

5. **The route is not at the edge.** It is served on the broker's internal listener only: no Ingress, no route in the
   public API document, and a NetworkPolicy that admits the orchestrator's pods (to bind) and the injector's pods (to
   look up). The broker authenticates the caller itself (a TokenReview of the presented service-account token, or
   mTLS), and trusts the agent identity only from that caller: the `X-Run-Id` header is an opaque capability, never a
   credential (S9's rule).

6. **Expiry and revocation are the binding's.** The binding expires at `expiresAt`; a canceled or finished run deletes
   it (`DELETE /internal/runs/{run}`, idempotent, the orchestrator's call); a sweep removes expired rows. After either,
   the next lookup is `run_not_bound` or `lease_expired`, and the injector's cache holds nothing past one lease.

7. **Metering keeps its owner.** The lookup is how the injector learns the run's identity; the usage ledger charges the
   run's person (ADR 0065), and the balance check with `model_locked` belongs to the injector's per-call authorization.
   This route answers "who is this run" and "what may it spend with", not "may it spend now".

## Consequences

- The agent holds no model key, a revoked run stops within one lease, and the broker knows the run's person before the
  first model call.
- The broker gains one table and two routes; the orchestrator gains the bind/delete calls; Authorino gains the lookup.
  No new dependency, and no driver type in a signature (invariant 6).
- The lookup route's reachability is the S9 half: a NetworkPolicy and a listener, and the agent's identity authenticated
  by Authorino (TokenReview or a client certificate).
- Two stores behind one port: the vault's availability is the lookup's availability; the sealed store is the dev
  fallback, and the conformance suite must hold both to the same answers.

## Alternatives

- **The credential in the AgentConfig or a Secret the agent mounts**: the agent, or its pod, would hold it; refused by
  the epic's constraint that the agent never holds a model credential.
- **The broker as the model proxy** (the call goes through the broker, which adds the key): a second proxy beside Agent
  Router, which the owner chose; refused.
- **A lease for the run's whole life**: fewer broker calls, but a revocation waits for the run to end; the 30 s lease
  with the injector's cache is the recommendation.
- **A per-person provider key** (open question 1): a different store shape (per person and model) and a different
  balance semantics; the ADR keeps the deployment's key as the default.

## Open questions

1. **Which credential is injected** — the deployment's provider key for the model endpoint, leased per run and metered
   against the person's balance (**recommended**: the frontier plan prices per task and the balance is the person's), or
   a per-person key kept in the vault. The epic's words, "the person's run-scoped credential", read as the latter; the
   frontier plan reads as the former. The answer changes the store's shape, not the routes.
2. **The lease TTL** — 30 s per lease, renewed by Authorino's cache (**recommended**, the epic's "at most 30 s"), or the
   run's whole life.
3. **The lookup's caller identity** — ~~Authorino's service account plus the agent identity it authenticated
   (**recommended**), or mTLS from the injector.~~ **Answered 2026-11-24** (the owner, ANR-Q31): the orchestrator and
   the injector's pods, and **agents never call it** — see decision 2 as amended. The transport (a token, a TokenReview
   of a service-account token, or mTLS) stays decision 5's, and the identity that answers is `RunCaller::Injector`.
4. **Does the binding name the model endpoint**, or does the lookup resolve it from the agent's AgentConfig?
   **Answered 2026-11-24** (the owner, ANR-Q53): the binding names it, and the agent's card lists, per choice, the
   endpoint label the alias maps to. See the status note of the same date below.

## What this ADR does not fix

The vault's own deployment and unsealing, the balance check and `model_locked`, the egress proxy's interception for
people's own hosts, and the operator's parity slice — each is its own slice of the epic.

### Status note, 2026-11-24: question 4 answered — the binding names the endpoint, the card lists the choices' endpoints

**The owner, ANR-Q53, 2026-11-24 06:32Z:** *"(a) adam's agent card lists, for each model choice, the endpoint label its
alias maps to; the orchestrator records it when the run starts."*

1. **The card's field.** On `model-pick/v1` each choice gains `endpoint`:
   `params {choices: [{id, label, endpoint}], default?}`. `endpoint` is a **label** — the deployment's own name for a
   model endpoint, the key of `models.endpoints.<name>` in the orchestrator's configuration — and never a provider, a
   host, a URL or a key. It is optional: a card that lists none is the card of today, and nothing about it changes.
2. **At bind time, once per run.** When the dispatcher first sees the run's task id (`app/src/dispatcher.rs`,
   `bind_run`), it resolves the person's picked alias against the **live** card and writes that choice's `endpoint`
   into `RunBinding.model` (`ports/src/credentials.rs`, the field this question introduced). With no pick, the card's
   `default` choice is resolved the same way; with a card that lists no `endpoint`, `model` stays `None`.
3. **One bind, one endpoint.** A later message of the same run that picks a different alias is a **`409 Conflict`**,
   refused rather than re-pointed: the lease is for one endpoint, and re-pointing a live run would spend a person's
   balance against a model the binding never named. This is decision 1's "a run is bound once" applied to the pick.
4. **Fail closed.** A binding whose `model` is unset makes the lookup answer `unavailable`, never an empty credential
   (decision 4): a deployment that lists no endpoint lends nothing.
5. **The route keeps its override.** `POST /internal/runs`'s optional `model` stays, and wins over the card's
   resolution, for a caller that already knows the endpoint (decision 1's "if given").

### Status note, 2026-11-24: the lease names the account (ANR-659)

The lease answer is `{token, accountId, expiresAt}`. `accountId` is the run's person: a `UserId`, the key the usage ledger is kept by
(`orch_core::UsageRow.owner`, the column `usage_tasks.owner` of migration `0023`, ADR 0065), which is the person id the injector reports as
`x-account-id` — **not** a provider account and not an account number of a ledger service, since the orchestrator holds no other identity for the
person. The injector gets it beside the key so it can say whose call it is. It answers only behind decision 2's caller check, and like the token
it is never logged, never in a refusal or an error, and never in a message to an agent; `RunLease`'s `Debug` redacts both. *Unverified:* the
injector that reads it (B2) and the gateway that trusts it (B3) are not built.

### Status note, 2026-11-24: the meter asks for the account (ANR-663)

The operator's pod-minute meter finds who pays for a run pod without any person id on a pod: `GET /internal/runs/{run}/account` answers
`{accountId}` (the same `UserId` as the lease's) to **one caller, `RunCaller::Meter`**, the operator's own identity. Every other caller (the
orchestrator, the injector, any agent, its own run included) and an unknown run get `run_not_bound`, so the answer never says whether a run
exists; no identity is `unauthenticated`. The meter holds no lease, bind or unbind. The answer holds no credential, so it does not stop at the
binding's deadline (a pod can outlive its run's lease); it ends with the binding. `accountId` is in no log line, no refusal and no error. `RunLeases`
gains the required method `account`, which breaks an implementer. *Unverified:* the binary's configuration key for the meter's token
(`SharedTokens::with_meter`) is not wired yet, as the injector's is not; the meter itself is the platform's.
