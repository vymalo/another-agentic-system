# ADR 0072 — Child sessions are nested threads, followed through `child-sessions/v1`

- **Status:** **proposed (2026-11-18).** The feature is the owner's decision of 2026-11-14, recorded in adam-rs
  [ADR 0045](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0045-a-subagent-can-run-in-the-background-as-a-session.md)
  (*"we need to add the capability that opencode has to create child session, and exchange messages with it. At creation, there
  should be a button to auto-close after work is done, so that the child session can further be reused."*), whose decision 11 puts
  the contract of the extension in this repository. The shape below is proposed, slice by slice. **Amended (2026-11-19, S3): the person's half is decided, see the note at the end; S3a is this documentation, the code follows in S3b to S3e.** **Built: S1, S2a–S2f** (the contract
  [`child-sessions-v1.md`](../api/child-sessions-v1.md) and the orchestrator's reading of the card; the pure core; the store; the A2A
  client's activation and strict reading of what an agent says; the application and the dispatcher that make the child threads and follow
  their rounds, and the API that says them; the AG-UI projection; the scenario on mocks, [`dev/child-sessions-e2e.sh`](../../dev/child-sessions-e2e.sh), *verified 2026-11-19* by running it twice against the real orchestrator, Postgres and the WireMock agent `mock-sessions` (modelled on adam-rs's `subscribe.rs`, `session_events.rs` and `child_sessions.rs`), without the edge, the web or a real agent; the full stack runs it in CI only). Epic:
  [#332](https://github.com/vymalo/another-agentic-system/issues/332). Amends [ADR 0042](0042-the-thread-list-is-the-owners.md),
  [ADR 0055](0055-the-agent-assigns-the-a2a-context.md), [ADR 0029](0029-forking-a-thread-copies-its-log.md),
  [ADR 0036](0036-sending-while-an-agent-works.md), [ADR 0050](0050-the-chat-has-sub-agents.md) and
  [ADR 0026](0026-agent-mentions-as-structured-references.md) (dated notes in each).

## Context

adam-rs runs a helper as a **session** (its ADR 0045, slices 1 and 2 built): a chain of child runs in a conversation of its own,
started by the tool `subagent_start`, kept open between messages when asked, reported to the parent when a round ends. The parent's
model can start, read, send to, list and close sessions. The person cannot: the orchestrator sees the helper only as a step of the
parent's task ([`steps-v1.md`](../api/steps-v1.md)), and its rounds' work is folded into the parent's.

What this system has to build on, *verified 2026-11-18* at `470e156`:

| Fact | Where |
|---|---|
| A thread has one binding to an agent context, and the agent assigns the context: the binding adopts what the first answer names. | [ADR 0055](0055-the-agent-assigns-the-a2a-context.md) |
| The rail nests a thread under another by `rail_parent`, today set only for a fork. | `orchestrator/crates/core/src/thread.rs` (`rail_parent`), `fork.rs` (`rail_parent_of_fork`), [ADR 0042](0042-the-thread-list-is-the-owners.md) |
| One log per thread; usage is attributed from the log to the person (the ledger is written in the commit of the event that carries it). | [ADR 0056](0056-token-usage-per-model-call.md), [ADR 0065](0065-a-usage-ledger-per-person.md) |
| A job is a message the orchestrator sent and follows to its end. | `orchestrator/crates/core/src/transition.rs`, `docs/orchestrator.md` |
| An extension that carries params is read into a typed offer, never cached, and a card that lists it badly offers nothing. | `access-tier/v1`: `orchestrator/crates/core/src/access_tier.rs`, `crates/agent-a2a/src/access_tier.rs` |
| AG-UI already has `SUBAGENT_STARTED` and `SUBAGENT_FINISHED` frames for a sub-agent's work under a run. | `orchestrator/crates/agui-proto/src/event.rs`, `crates/agui-projection/src/projector.rs` |

## Decision

1. **The contract is [`child-sessions-v1.md`](../api/child-sessions-v1.md)**, an optional extension (ADR 0008): params `{agents,
   maxOpen}` on the card; events `opened`, `round`, `reported`, `closed` on the parent task's status updates; a sessions object on every
   task of the parent context, which is the truth the events only point at; a person's `start` on a message to the parent; plain
   messages to the child context, delivered into a working round as adam-rs delivers to a busy context; each round saying its
   session's state; a close as `{close: {cancel}}` in the message metadata (adam-rs ADR 0045 said a data part: metadata cannot be
   mistaken for another extension's data part, such as an A2UI action); `InvalidParams` for a child context that did not activate it.
   The orchestrator reads the card like `access-tier/v1`: an offer only when the URI is listed exactly and the params are well formed,
   read on every send, never kept. The capabilities document carries `custom[<URI>] = {agents, maxOpen}`.
2. **A session is a child thread.** On `opened`, or on a session it finds in the sessions object and does not know, the core decides a
   new thread with `child_of` (new: the parent thread and the session id) and `rail_parent` set to the parent, owned by the parent's
   owner, bound to the child context the agent named (ADR 0055: the context is adopted, never made up). A session the orchestrator
   already knows is matched by its session id; an event seen twice changes nothing.
3. **A round is an observed job** (new): a job of the child thread that the orchestrator did not send but follows, by
   `SubscribeToTask` of the round's task, to its end. Its steps, text and usage go to **the child thread's log only**, so a round's
   usage is counted once, against the person who owns the parent thread (see the *Amended (2026-11-18)* note for which thread
   counts which round). A round the person starts in the child thread is an ordinary job of it.
4. **The parent's log** records `child_session_opened`, `child_session_reported` and `child_session_closed` (new events, decided by the
   pure core from the parent task's events and from the session state each round it follows carries, whichever comes first, once
   each: a session the person started while no parent task runs is known only from its rounds); the helper's work is not copied
   into it. The parent thread's view shows the session as a
   step card that links to the child thread.
5. **The person and a session.** In a child thread the person sends a message (the next round when the session is idle, a steer through
   `steer/v1` when it works, ADR 0036), stops the round (`CancelTask`), and closes the session. From the parent thread they start one:
   an agent of the card's `agents`, a description, a prompt and **"Close when done"**, on by default (adam-rs ADR 0045 default 1). The
   thread permissions are the parent's: the owner writes, nobody else sees it ([ADR 0039](0039-nobody-reads-another-persons-thread.md)).
6. **A fork does not take sessions.** A fork of a parent thread copies its log (ADR 0029) and has no child threads; the parent's
   sessions stay the parent's.
7. **Without the extension nothing changes:** a helper is a step, as today. A client of the orchestrator's API that does not know child
   threads sees them as ordinary threads with a `childOf` member it may ignore.
8. **AG-UI.** A child thread streams as any thread. In the parent's stream an opened session is the `SUBAGENT_STARTED` and
   `SUBAGENT_FINISHED` pair the projection already has, with a new member that names the child thread (`agui.md` gains it with S2).

```mermaid
sequenceDiagram
  autonumber
  participant W as Web
  participant O as Orchestrator (new: child threads)
  participant A as Agent (lists child-sessions/v1)
  W->>O: a message in the parent thread
  O->>A: SendStreamingMessage, child-sessions/v1 activated
  A-->>O: parent task update: opened {session, contextId, round 1}
  O->>O: child_session_opened in the parent's log, a child thread bound to contextId (new)
  O->>A: SubscribeToTask round 1 (an observed job, new)
  A-->>O: round 1's steps, text, usage, completed
  O->>O: the child thread's log, then child_session_reported in the parent's log
  W->>O: a message in the child thread
  O->>A: SendStreamingMessage {contextId: the child's}
  A-->>O: round 2
```

```mermaid
stateDiagram-v2
  [*] --> Open: child_session_opened, a child thread is made
  Open --> Open: a round reported, or the person's round
  Open --> Closed: child_session_closed
  Closed --> [*]: the child thread stays readable, its composer is gone
```

## Alternatives considered

- **Fold the helper's rounds into the parent's log.** It is what happens today; the person cannot talk to the helper, and a kept-open
  helper's later rounds would land in whatever turn of the parent is open, or nowhere.
- **A thread of its own with no link to the parent.** The rail would fill with threads the person did not start and could not place.
- **The orchestrator makes the child's context.** ADR 0055 forbids it, and adam-rs derives the child context from the parent's.

## Consequences

- New: the core's `child_of` on a thread, the three parent events, the observed job; a migration for `child_of` (S2). A store adapter
  gains what S2 adds to the store port, with its conformance cases.
- Invariant 3 holds: a child thread is a thread, with its own log.
- The usage ledger (ADR 0065) counts a round once: in the parent for a round the parent's model started, in the child for a
  round a person started (*Amended (2026-11-18)*).
- **Until adam-rs moves it**, the live usage of a round the parent's model starts is reported on the parent's task (as it is today),
  and the round's own task carries its totals when it ends. S2 must count such a round once: from the parent's reports or from the
  round's totals, never both.
- Slices, in [#332](https://github.com/vymalo/another-agentic-system/issues/332):

| Slice | What | Depends on |
|---|---|---|
| S1 | The contract, this ADR, the card read into a typed offer, the capabilities key, the fake agent's word `child-sessions` | nothing |
| S2 | Child threads, observed jobs, the parent's events, a WireMock agent that serves the extension (S2a to S2f are built) | S1 |
| S2e | The AG-UI projection of a session: the parent's card and the child's log in the stream (built; `agui.md`) | S2 |
| S2f | The scenario on mocks: a WireMock agent that serves the extension and `dev/child-sessions-e2e.sh` (built; `dev/README.md`) | S2 |
| S3 | The person sends, steers, stops, closes and starts a session | S2 |
| S4 | The web: the nested thread, the step card with "Open", the strip of sessions, "New subagent" with "Close when done" | S3 |
| S5 | The adam pin with adam-rs ADR 0045 slice 6, the chat's `planner` kept open in `dev/agents-e2e.sh` | S4, adam-rs slice 6 |

## Amended (2026-11-18)

**Which thread counts a round's usage.** A round the parent's model started is counted **once, in the parent**: adam already
adds it to the parent's totals and reports it live on the parent's task. The child thread logs that round's `model_usage` and
`model_usage_total` with `counted: false`, so the ledger skips them. A round a **person** started is counted in the child. A round
whose `by` the agent did not say is taken for the agent's and is not counted in the child. A child that is blocked and answered
within the same job keeps the `by` of its round; what counts that answer is decided with S3. If adam moves a round's usage onto
the round itself, the rule flips: the flag is per event, so the history already written stays right. This replaces the wording of
decision 3 and of the consequences that left the choice open.

## Amended (2026-11-18, S2d review)

- **A background round that outlives its parent's task** (a helper the model started and the parent's task did not wait for) is counted in
  the parent **only if adam adds it to the parent task's totals** once it ends. *Unverified*: adam-rs was read for the case of a round
  the parent waits for, not for one that ends after the parent's task did. If adam does not, **the round is counted nowhere** (the
  child's totals are `counted: false` and the parent's task is over). To be checked when S5 pins adam, with the flag per event
  (decision above) as the lever if the answer is no.
- **Decision 2: where a child nests.** A child of a thread that is itself a nested fork is nested under that fork's **root row** of the
  person's list, as a fork of it would be (`rail_parent_of_fork`, one level deep), not under the fork; and under its parent's row
  even when that row is archived (the child is archived with the block).
- **A child is read-only until S3.** A message, a steer, a Stop & send, an action and an AG-UI run on a child thread are `422
  child_thread` like a fork and a share; a cancel is allowed. S3 opens them deliberately.
- **Fail closed against an agent that lies about a round.** An envelope whose `contextId` is not the context the child adopted is
  refused and counted (`child_sessions_dropped_total{reason="foreign_context"}`) and the follow fails, so a task id naming the
  parent's task or another conversation's is never copied into the child. A dispatcher follows at most
  `MAX_OBSERVED_PER_PARENT` (8) rounds of one parent at once; the rest wait with backoff and are never dropped.

## Amended (2026-11-19, S2e)

- **Decision 8 as built.** The new member is the metadata key `vymalo.childThread` of the `SUBAGENT_STARTED`, beside
  `vymalo.session`; the session is also a `vymalo.session` card. A subagent id is used once in a run (the reference client refuses
  a second start), so the first subagent is `sub-session-<seq>` and each one a later report starts is
  `sub-session-<seq>-<seq of the report>`. A `blocked` report ends its subagent as suspended and says the card `working`; the
  core reports that round again when it ends otherwise, and the projection says each report that differs from the last
  ([`agui.md`](../api/agui.md#child-sessions-as-subagents)).
- **A session event never opens or reopens the agent's invocation**: its subagent has the open invocation as its parent, or none.
  A session still open when the invocation (or, with none, the run) ends is suspended, not canceled. A fork closes the cards it copied.
- **`PROJECTION_VERSION` stays 2.** No released agent serves `child-sessions/v1`, so no log holds a session event, and a session
  event changes no frame of any other event. `childOf` in `STATE_SNAPSHOT` stays S4's.

## Amended (2026-11-19, S3)

The person's half: a person sends, steers, stops, closes and starts. Decisions D1, D3, D4 and D5 are **the owner's** (2026-11-19,
relayed from the owner's chat); D2, D6, D7 and D8 are **proposed defaults**, accepted with the plan and open to the owner's change. The
contract side is [`child-sessions-v1.md`](../api/child-sessions-v1.md) (amended the same day) and the routes are in
[`chat-api.yaml`](../api/chat-api.yaml) (`startChildSession`, `closeChildSession`); the frames are in
[`agui.md`](../api/agui.md#a-person-starts-sends-and-closes-a-session). **Not built yet:** nothing here runs until S3b to S3e, and
nothing has run against a real adam (S5).

| # | Question | Decided | By |
|---|---|---|---|
| D1 | Start: asynchronous or synchronous | **Asynchronous.** `POST /api/threads/{id}/sessions` is 202 with `{request}`; the outcome is `child_session_start_ended` in the parent's log, written through the outbox, and adam's own words reach the person as its `reason` | the owner |
| D2 | A start on a parent with no A2A context yet | 409 `no_context` ("send the agent a message first") | proposed |
| D3 | Stop & send in a child | **Only for a kept-open session**: cancel the round, then the next one. Otherwise 422 `session_closes_on_stop`, since stopping a session that closes when done closes it | the owner |
| D4 | Who counts a person's answer to a round the agent started and left `blocked` | **The round keeps its starter's `by`**: the usage rule is unchanged and the parent counts it | the owner |
| D5 | Mentions in a child's message | **Refused**, 422 `child_mentions`: a helper session cannot ask other agents | the owner |
| D6 | Closing a closed session | 202 and nothing written; so is a close already asked with the same or a stronger `cancel` | proposed |
| D7 | The text of a start that fails | Not kept in the log (`child_session_start_asked` holds no text); the web keeps the draft until the outcome arrives | proposed |
| D8 | Starts waiting for their outcome, per parent | 4, then 409 `too_many_starts` | proposed |
| D9 | A parent that notes its 256 sessions (the ledger's bound, whatever their state) | 409 `too_many_sessions` before anything is written; a start accepted earlier that opens into a ledger that filled meanwhile ends `failed` with a word and makes no thread (counted as `ledger_full`) | proposed |

- **Decision 5 as built (the person talks to a child).** A message in a child thread is the helper's next round when the session is
  idle, a steer into the round while it works (`vymalo.send: steer`, as ADR 0036) and the answer to the round when the helper asked
  (`blocked`). The core decides, not the route. The "read-only until S3" note of 2026-11-18 is closed: a message, a steer, a Stop &
  send and an action are allowed, and refused only for a closed session (409 `session_closed`), a mention (D5) and a Stop & send on
  a session that does not stay open (D3). A fork and a share of a child stay 422 `child_thread`; a start from a child is 422
  `child_thread`. Every one of these is the thread owner's only (another person gets 404, ADR 0039).
- **A start is a request, not a turn.** The API writes `child_session_start_asked` in the parent's log (no text) and an outbox row in the
  steer lane; the dispatcher sends the start to the parent's context, requires an answer whose round object names a new context and
  the parent's own as `parentContextId` (else it fails closed), makes the child thread with the person's text as its first message and
  the first round observed as the person's, and writes `child_session_start_ended`. adam derives the session from the message id, so
  a resend after a crash finds the same session.
- **The race rule.** A working parent may report the person's session (`opened`, `by: person`) before the start's answer: the
  orchestrator holds such a session back while the parent has a pending start, and the answer makes the child. After the outcome, a
  session left over (adam started it and the answer was lost) is made from the next listing.
- **Refusals only adam can make** (its open, working and per-session round limits, a closed session the ledger did not know of)
  arrive after the 202 or the run, with adam's own words: as `reason` of a failed start, and as the error of a message's failed job.
  They are untrusted text; the web draws them as text.
- **A close is a request.** `POST /api/threads/{id}/session/close` writes `child_session_close_asked` in the child's log and sends a
  `close {cancel}` message to the child context. The child's log then says the session's state (`child_session_state`, written only
  when the state or `closing` changes), and the parent's `child_session_closed` follows from the report. The closing round, which only
  closes, is not followed as a round of its own.
- **Visible to the person only as two cards** (S4 draws them): `vymalo.sessionStart` in the parent, and the child's own
  `vymalo.session`, which the composer reads. `PROJECTION_VERSION` stays 2: no released log holds these kinds.
- **Four new event kinds** (one migration, S3b): `child_session_start_asked` and `child_session_start_ended` in the parent's log,
  `child_session_close_asked` and `child_session_state` in the child's. A build that does not know them cannot read a log that has
  them: roll the build out before the routes are used.

```mermaid
sequenceDiagram
  autonumber
  participant W as Web
  participant O as Orchestrator
  participant A as Agent (child-sessions/v1)
  W->>O: POST /api/threads/{parent}/sessions {id, agent, description, text}
  O->>O: card checks, child_session_start_asked (no text), outbox row
  O-->>W: 202 {request}
  O->>A: message to the parent context, start {agent, description, keepOpen}
  A-->>O: round 1's task in a new child context (or an A2A refusal)
  O->>O: child thread with the person's text, child_session_opened by person, child_session_start_ended (opened, or failed with the agent's words)
  W->>O: run on the child thread (a message, or vymalo.send steer)
  O->>A: message to the child context (next round, steer, or the answer)
  W->>O: POST /api/threads/{child}/cancel
  O->>A: CancelTask of the round
  W->>O: POST /api/threads/{child}/session/close {cancel}
  O->>O: child_session_close_asked
  O->>A: message to the child context, close {cancel}
  A-->>O: the closing round, session closed
  O->>O: child_session_state closed, parent: child_session_closed
```

```mermaid
stateDiagram-v2
  [*] --> Asked: startChildSession (child_session_start_asked)
  Asked --> Opened: the agent answered with round 1 (child_session_start_ended opened)
  Asked --> Failed: the agent refused, or could not be reached (child_session_start_ended failed)
  Failed --> [*]: nothing was made
  Opened --> Working: round 1 runs
  Working --> Idle: a round ends, a kept-open session
  Idle --> Working: a message (the next round)
  Working --> Working: a steer, or the answer to a blocked round
  Working --> Closing: a close asked while the round works
  Idle --> Closed: a close
  Working --> Closed: a round ends and the session does not stay open
  Closing --> Closed: the round ends
  Closed --> [*]: the thread stays readable, no message is taken
```

## Amended (2026-11-19, the pin)

The compose pin and the chart's `chat.image` are adam-rs `7418467` ([ADR 0014](0014-adam-coder-default-agent-over-a2a.md)'s status
note of 2026-11-19), which has adam-rs ADR 0045 slice 6 (#173, #174) and its A6c (#198): the cards of the coder and of `adam-agent`
list `child-sessions/v1` when the root has a subagent (the coder and the chat here), so S5's dependency on adam-rs is met. S5 itself (the chat's `planner` kept open in `dev/agents-e2e.sh`) is
not built by the bump. *Unverified* until the Coder E2E workflow has run on the pin: the orchestrator activating the extension on the
real cards.

## Amended (2026-11-20, S4)

The web draws it ([`web/README.md`](../../web/README.md#subagents-child-sessions), [`web/DESIGN.md`](../../web/DESIGN.md#subagents); slices S4a to S4g of #332). The owner's decisions for S4:

- **A tab, not a strip.** The list is the right-hand panel's **Subagents** tab; the chat holds only the step cards and the start lines.
- **The list's source** is the rail's rows with `childOf` merged with the cards in the chat, not a new `?childOf=` filter.
- **Tools are hidden in a child**: no tools picker, and no `@`, `/` commands, fork, edit, share or agent switch.
- **"Subagent"** is the UI's word; "helper session" stays in the docs.
- **Late report runs are not drawn as turns**: a card is drawn once, in the run that opened it, with its live state.
- **A start takes the tier chip's current tier**; the dialog has no tier control.
- **The Activity tab's line for a subagent is deferred.**

*Unverified:* a real agent's cards in the web (the mock plays the contract; adam-rs ADR 0045 slice 6 has not been seen in a browser).
