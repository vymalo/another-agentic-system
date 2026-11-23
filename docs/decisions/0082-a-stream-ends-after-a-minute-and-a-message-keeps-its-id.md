# ADR 0082 — A stream ends after a minute and is reopened; a message keeps the id its client made

- **Status:** **proposed (2026-11-21)**. **Both halves are built in this change** (branch `stream-hardening`, off `d9258d2c`): the
  orchestrator's lifetime and attach, and the web's dead-stream detection, jitter and draft-owned message id. The owner decided on 2026-11-21 to
  *"propose the generic stream changes in the open source, in neutral wording"* (another-context, the persona answers, decision 52):
  a heartbeat every 15 s, streams that end and reopen, and a client-made message id — **with no market in the text**. The
  constraints it answers are two requirements of a private persona analysis.
  Amends [ADR 0012](0012-ag-ui-user-facing-protocol.md) (the connect stream) and [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md)
  §6 and §10 (an hour is the longest stream; a credential with no expiry is unbounded). Number `0081` is claimed by another session;
  this is `0082`.

## Context

A client on a slow, poor or interrupted connection cannot hold one HTTP response open for an hour and cannot tell a stream
that is quiet from a stream that is dead. The requirements, as tests:

- **C3** — *"Open streams carry a heartbeat at least every 15 seconds, and the client reconnects after two missed ones, with a
  growing, randomised wait."* Test: hold a stream idle: it stays up, or comes back within 45 seconds.
- **C4** — *"A message has an id made by the client, with or without a queue. The id is made when the message is first written,
  kept with the draft, and used again for every send of that text, whether the transport retries or the person presses Send
  again by hand. Sending it five times makes one message, one run and one charge."*

What exists today, read at `ebcee350` and changed by this ADR (the last column is what this build leaves):

| What | Before | Where | Now |
|---|---|---|---|
| Heartbeat | **Done**: `: keepalive` every 15 s on all four SSE responses (run, connect, shared connect, public shared connect) | `orchestrator/crates/api/src/sse.rs:51-55`, `api/src/lib.rs:120`, wired at `surface-agui/src/run.rs:104` and `connect.rs:105,187` | unchanged |
| Stream lifetime | **One hour**, or the token's `exp` + 60 s, **or nothing at all** when the principal carries no `expires_at` (a proxy header, a static token) | `api/src/sse.rs:14,24-31,35-47`; ADR 0033 §6, §10 | **the shortest of the three, always** (`sse::stream_budget`, `ApiConfig::sse_lifetime`, 60 s by default) |
| Resume | **Done**: `Last-Event-ID` on the three connect routes; the log is re-read from 1 and folded to the cursor, then the stream follows | `surface-agui/src/connect.rs:54-70,88-93`, `agui-projection/src/connect.rs:58-71,89-109` | unchanged, and now the ordinary path once a minute |
| Idempotency | **Done server-side**, keyed on the **client's** message id: `agui:<thread>:msg:<id>`, a unique index, `Duplicate` folds to an attach | `surface-agui/src/run.rs:409-431`, `agui-projection/src/translate.rs:389-404,519-548`, `store-postgres/src/store.rs:1852-1868`, `migrations/0001_init.sql:32` | unchanged: the key is the message id, never the run id |
| A resend with a **new** `runId` | **422 `nothing to run`**: the message is already in the log, so there is nothing to start, and it is *not* an attach | `agui-projection/src/translate.rs:539-548`; `docs/api/agui.md:476-478` | **attach to the run that message opened**, when the words are the same: the log's `user_message` records the run and the text, and the projector folds both into `KnownThread` (`message_runs`, `message_texts`); a resend with other words is 422 `message_id_reused` |
| A resend with **other words** under a known id | **Silently dropped**: reconciliation by id kept the logged message and discarded the new text (or answered 422 `nothing to run` when the run id was new) | `agui-projection/src/translate.rs:389-404,539-548` | **422 `message_id_reused`**, before anything is written: an id is bound to its text |
| The web's send | Mints a **fresh** `runId` per send and a fresh message id (the library's), and on a refusal keeps only the **text** | `web/src/features/chat/lib/agui/thread-agent.ts:1495-1515,1540-1541`, `failed-send.ts:8-17`, `use-chat-runtime.ts:192-194` | **the id is the draft's**: `DraftIds` binds one id to one text, `post()` sends it in place of the runtime's, and an edit mints a new one (`lib/agui/draft-ids.ts`) |
| The web's reconnect | On the stream **ending**, with `Last-Event-ID` and a backoff of `min(30 s, 1 s · 2^attempt)` — **no jitter, no dead-stream detection** | `thread-agent.ts:383,593-637`; comments are parsed and discarded at `lib/agui/sse.ts:41` | **built**: `readSse` takes a `silenceMs` window re-armed by any bytes, so two missed heartbeats close the reader and the existing loop reopens from the cursor; the wait is `floor(random() × min(30 s, 1 s · 2^attempt))` (`lib/agui/backoff.ts`) |

Invariants (`CLAUDE.md`): **3, stateless** — a lifetime is a property of a connection, nothing is stored; the resume point is the
log's `seq`, which is already the client's. **5, the core is pure** — untouched: a lifetime is the transport's, and the id is
already an input of `translate`. **1, protocols only** — SSE and the AG-UI contract, nothing host-specific.

## Decision

1. **A stream ends after 60 seconds**, whatever the credential: the shortest of the token's `exp` + 60 s leeway, an hour, and the
   lifetime. **A principal with no `expires_at` is bounded too** — the case that was unbounded.
   *Built:* `sse::stream_budget(principal, now, lifetime) -> Duration` (it can no longer say "unbounded"), `sse::bounded(stream,
   budget: Duration)` (the `Option` is gone, so an unbounded stream cannot be expressed), `ApiConfig::sse_lifetime` (60 s, like
   `sse_keepalive`: not a configuration-file key, shorter in tests), threaded from `boot.rs` (`surface_routes`) into
   `orch_surface_agui::routes(app, sse_keepalive, sse_lifetime)` and applied by all four streams — the run POST, the connect, the
   signed-in shared connect, and the public shared connect (which has no credential at all: its budget is the lifetime, at most an
   hour).
   *Rejected: keep an hour. A proxy, a phone radio or a NAT table drops a quiet connection long before that, and the client
   cannot tell, so the hour is a bound nobody reaches.*
2. **Ending is not losing.** The lifetime closes the response; nothing leaves the log, and a client that reopens with its last
   `id:` sees every later event exactly once. The contract says the reopen is the normal path, not an error path.
   *Built:* nothing new was needed — `Last-Event-ID` already re-reads the log from 1 and folds to the cursor — and the proof is a
   surface test at a short configured lifetime: a stream ends while its run is still open, the run finishes after it, and the
   reopen from the cursor says each later event once and none twice (`orchestrator/crates/surface-agui/tests/lifetime.rs`).
3. **The client reopens after two missed heartbeats** (≈30 s of silence) even though the socket is open, and its wait grows
   **and is randomised**, so a fleet of clients cut by one proxy does not return in one wave.
   *Built:* the keepalive already existed; the client's side is `readSse`'s silence window (any byte re-arms it, so a `: keepalive`
   is enough and a slow reply is not a dead stream), 30 s = two 15 s intervals taken from this contract, and the full-jitter wait.
   The run's own POST response is deliberately **not** watched: a watchdog there could turn a slow accept into a second send.
4. **A message's id is minted once, by the client, when the text is first written** — kept with the draft, reused on every send
   of that text, including a manual press of Send after a refusal.
   *Built:* the server already keyed idempotency on the client's message id; the web now binds one id to one text (`DraftIds`) and
   sends it in place of the runtime's, so a resend after a refusal carries the same id. The binding is by **text**, because the
   server refuses a known id sent with other words (`message_id_reused`): an edited draft is a new message, which is C4's own rule.
5. **A resend of a known message id attaches to the run that message opened**, whatever `runId` it carries. This replaces the
   422 `nothing to run` for that case: five sends are one message, one run, one charge, and the person sees their answer
   instead of an error.
   *Built, and the log already held the answer:* a `user_message` event carries both `message_id` and `run_id` — *the id of the run
   this message started or continued* (`orch-core`, `UserMessageData`) — so no event, no migration and no new column was needed.
   What was missing was the fold: `Projector` now records `message_runs: message id → run id` as it folds a `user_message`, and
   `Projector::view` hands it to `translate` in `KnownThread::message_runs`. When nothing is new and the request's `runId` is not
   the thread's, `translate` attaches to the run the **last** message of the request that the log has opened — the one the client
   is sending again — and returns it as `Translation::attach`; the surface streams that run from its `RUN_STARTED` (its own
   `runId` in the frames, not the request's). A known id whose log entry opened no run (an agent's message, an activity) is still
   the 422: there is nothing to attach to, and the guard stays.
   **The id is bound to its text.** C4 says the id is "used again for every send of *that text*", so a resend repeats the same
   bytes; a request that repeats a known id with **other** words is not a resend but a second message wearing the first one's id,
   and it is refused with 422 `message_id_reused` before anything is written. Accepting another text would make the id
   meaningless, and the two alternatives are both worse: dropping the new words silently (what the reconciliation by id did) or
   attaching to the old run and showing the person an answer to words they no longer sent. The log's `user_message` carries the
   text, so the projector folds `message_texts: message id → text` beside `message_runs`, and `translate` compares exactly, before
   either path that reconciles by id — the drop of a message the thread holds and the attach. A client that edits a draft mints a
   new id, which is C4's own rule, now enforced where the orchestrator can see it. (The comparison is exact bytes of the text the
   message would be sent with: the surface's own reading of the content parts, so a retry that resends the same request passes.)
   *Rejected: keep the 422 and make the client reuse the `runId` too. It works only for our own client; C4 is about the
   protocol, and a retry that a proxy replays cannot be expected to remember a run id.*
   *Rejected: a run id on the event's idempotency key. The key is the message id (that is what makes a retry the same input);
   keying on the run id would make five sends five messages, the opposite of C4.*

## Consequences

- The web's reconnect loop stops being an exception handler and becomes the ordinary path once a minute: every stream test that
  asserts "stays open" now asserts "ends, and reopens where it stopped". In this repository that was
  `surface-agui/tests/roles.rs` (the stream of a day-long token and of a credential with no expiry), rewritten to prove the bound.
- **The web is in this change, and so is its mock.** `web/mock/` ends a followed stream at the same 60 s
  (`MOCK_STREAM_LIFETIME_MS`) and can go silent on demand (`POST /__mock/silence-streams`), so the Playwright specs prove the
  end-and-reopen path and the dead-stream path against a server that behaves like the orchestrator. The client reopens with
  `Last-Event-ID` and a jittered wait; the message it sends carries its draft's id.
- The scenario scripts are clients of this contract too, and they were the first thing it broke: `dev/chat-browser-e2e.sh` read a
  run's POST stream once and lost the terminal event of a run that outlived the minute (CI job 136915064539). `dev/sse.sh` is that
  client now — it reopens with `Last-Event-ID`, drops what the cut stream wrote after its last resume point, and drops the reopen's
  preamble — and the scenarios whose run outlives a minute (the coder's, the browser's, kagent's) read it to its end through it.
  `dev/split-e2e.sh` still proves a viewer's mid-stream reconnect with `Last-Event-ID`; the orchestrator's own tests prove the bound
  and the once-only replay (`surface-agui/tests/lifetime.rs`).
- Billing does not move: usage is folded from the log, and the log gains nothing from a reopen. Five sends of one message now
  add one `model_usage_total` (proven in `surface-agui/tests/attach.rs`).
- The 422 row of the run route narrows to "the message id is known but its run cannot be attached" — a case that should now be
  unreachable for a message the log has a run for, and stays as a guard — and gains one new case, `message_id_reused`, for a
  known id sent with other words. A client that edits a draft must mint a new id; that is C4's own rule, and the alternative was
  either a silent drop or an answer to words the person no longer sent.
- Nothing about a connection is stored: the lifetime is a `Duration` in the router's state, and the message→run and message→text
  maps are folds of the log, rebuilt on every request like the rest of the projector.

## Alternatives considered

- **Server-minted message ids.** The client needs the id before the network — to keep it with the draft and to reuse it after a
  refusal — so the server cannot be the one that mints it.
- **An outbox on the client.** Dropped by the owner on 2026-11-21 (answer 2, "keep drafts only"); C4 does not depend on a queue,
  which is why it is separable here.
- **A `retry:` field in the SSE stream.** It would tell a client how long to wait, but it is one more thing every reader must
  parse and it cannot randomise; the client owns its own wait instead.
- **A run id on the `user_message` that the client can read back** (a new field, or an endpoint that answers "which run did this
  message open"). The log already says it in `user_message.run_id`, and the AG-UI projection already folds every event of the
  thread to answer the request, so a field would be a second source of the same fact.
