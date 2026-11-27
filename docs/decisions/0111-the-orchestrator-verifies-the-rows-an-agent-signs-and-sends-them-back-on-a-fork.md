---
sidebarTitle: "0111 · Verify an agent's signed rows"
---

# ADR 0111 — The orchestrator verifies the signed rows an agent hands it, and sends them back on a fork

- **Status:** **proposed (2026-11-27)**, card ANR-996. No owner decision exists for this record yet; the agent's half is
  accepted (adam-rs ADR 0068). **Nothing is built on either side of the wire**: adam has not built its slice T4, and the
  orchestrator reads no row. **Contract:** [`docs/api/signed-turns-v1.md`](../api/signed-turns-v1.md) (it holds the sequence
  and state diagrams). **Pattern:** [ADR 0008](0008-platform-integration-via-a2a-extension.md) (an optional, live,
  fail-closed extension). **Trust:** [ADR 0087](0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md)
  (the prompt statement's keys are other keys) and [ADR 0101](0101-a-cloud-runs-model-calls-are-signed-for-the-run-and-the-injector-stays-for-custom-cases.md)
  (a run certificate may be the turn key, and is outside v1: decision 6). **Fork:** [ADR 0029](0029-forking-a-thread-copies-its-log.md). The scenario that
  will prove it, ANR-914's `dev/signed-turns-real-e2e.sh`, is *unbuilt*.

## Context

adam-rs decided on 2026-11-22 that adam signs each assistant message and each tool result it commits, with a detached JWS
(`typ` `adam-turn/v1`) over `{typ, run, cnv, idx, prv, msg}`, and verifies every history that re-enters it
([adam-rs ADR 0068](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0068-adam-signs-its-own-history.md),
decisions 2, 3 and 6). Its last slice, T4, is an A2A extension, `https://agents.vymalo.com/a2a/extensions/signed-turns/v1`:
listed on the card only when the deployment turns it on, its JWKS the turn keys; a client that activates it gets the rows
a task committed in the task's agent message, and may send such rows back to continue from them (a fork). The ADR leaves
this system's half to this repository ("what the orchestrator does with `signed-turns/v1`: storing the rows in its log,
sending them back on a fork, and checking them").

**What adam has built, read on 2026-11-27 at adam-rs `origin/main` `6fe56423`** (*verified*: ADR 0068's slice table and
`git grep` of that revision):

| Slice | State |
|---|---|
| T1: `TurnKeys`, the Ed25519 turn signature, the chain | built (`crates/adam-core/src/turn`) |
| T2a: the engine's seam and the chart's key | built, commit `08fe47f3` (#447); the slice table has no row for it. **T2 is not complete**: no adam host passes turn keys to `LlmAgent` or `Runtime` except the ephemeral key of `crates/adam-runtime/src/worker.rs:2011`, so a real adam with a turn key from the stack also depends on T2 |
| T3a-1, T3a-2, T3a-3: turns signed at commit, the head signed, every turn verified on re-entry | built (2026-11-26 and 2026-11-27) |
| T3b: the journal's signature | **not built** |
| T4: `signed-turns/v1` itself | **not built**: no URI constant, card entry or message member exists at that revision |
| T5: release N+1, no adoption of unsigned history | not built |

So the rows exist and verify inside adam; nothing hands them to a client.

The orchestrator already keeps a log that holds steps with their input and output (`steps/v1`). Rows duplicate content the
steps already show, so what is stored and where it is allowed to go is a real decision. And the point of the rows is a
comparison: **the orchestrator shows the agent's final text and each tool step's output, and the rows are what lets it say
those are what the agent committed** (decision 4).

## Decision

1. **It is an optional extension, read live, never cached, removable (ADR 0008).** The orchestrator activates the URI only
   when the agent's live card lists it with well-formed `params`; an agent that does not is plain A2A, and a data part that
   looks like rows is one user turn, as adam's ADR says. The orchestrator never sends rows to an agent whose card does not
   list the URI.
2. **The wire is adam's; the names marked proposed in the page are ours until adam builds T4.** From adam's ADR: the URI,
   `metadata[URI]` on the task's agent message holding the rows with `run`, `cnv`, the first row's `idx` and its `prv`; each
   row's signature at `extra_content.adam.turn_signature`; the header (RFC 8785 of `alg`, `kid`, `typ` and an optional
   `x5c`); the payload and the chain. The card's `params` shape (adam's `KeySet` JWKS, so adam's own `to_jwks` output is the
   entry as it is) and the other members of `metadata[URI]` are proposals to put to adam for T4: the **source context** of
   a fork, a small **statement** over the row count and the chain (decision 3), and that the rows are **every committed row, a
   person's included** (steer/v1 inserts them).
3. **The orchestrator verifies every row it receives, at ingest, against the key set of the live card.** It checks: the
   header (`alg` EdDSA only, `typ` exactly `adam-turn/v1`, `kid` in the set and not `revoked`, no other member than `alg`,
   `kid`, `typ`, `x5c`); the detached payload rebuilt from the row; the signature; and the binding to the task and the
   thread:
   - **`run` equals this response's A2A task id** (adam's task id is the run id: adam-rs `crates/adam-a2a-runtime/src/backend.rs:52`,
     and ADR 0101, Facts), else `wrong_run`. A row signed for another run, replayed into this task, fails here.
   - **`cnv` is pinned to the thread on its first verified message** and must equal it afterwards, else `wrong_conversation`.
   - Within one message, `idx` runs without a gap and each `prv` is the chain hash of the row before it, so a reordered,
     dropped or altered middle row breaks every later one.
   - On **`unknown_key` the card is read once more** (live, never cached) before the row is refused: a rotation may have
     happened since the send. A second miss is the refusal.

   The first row's `prv` is taken as given: a signature over it shows adam signed that row at that position over that prefix,
   not that the orchestrator holds the prefix. **The chain does not detect that the last rows were dropped** (a prefix of a
   valid chain is valid). The orchestrator cannot check `adam-head/v1`, which signs the whole state document. So T4 should
   have adam sign a small **statement** `{typ: "adam-turns/v1", run, cnv, len, chain}` (a detached JWS with the same
   header rules and keys), where `len` is the number of rows of the run and `chain` is `h(len-1)`. **Once the card lists
   the URI the statement is required**: the orchestrator checks that `len` is the first `idx` plus the number of rows and
   that `chain` is the chain hash after the last row. A missing statement is `no_statement`, a failing one `truncated`; the
   rows of that message are refused and the content is `unverified`.
4. **A row that fails is refused and logged, and what it would have vouched for is shown as unverified.** Nothing of the
   refused rows is stored, projected, exported or sent back. "Logged" is a tracing line (the reason, `kid`, `idx`, the task
   id: **never a row's content or a signature**) and a counter `signed_turns_refused_total{reason}` (a closed set, no thread
   or agent in a label), as `child_sessions_dropped_total{reason}` is. The task itself is not failed: its message and steps
   are plain A2A.
   **The mark is a comparison.** On a task whose rows verified, the orchestrator compares the final message text with the last
   assistant row, and each tool step's output with its tool row. A match marks the content `verified`. A mismatch, refused
   rows, or no rows from an agent that listed the URI marks it `unverified`: it is never presented as the agent's verified
   content. The rows arrive with the final agent message, after the steps' `agent_step` events were logged, so **the marks
   ride on the `agent_message`**: `signed` for the message itself (`verified` or `unverified`, two-valued, absent for an
   agent that does not list the URI) and a closed map from step id to `verified` or `unverified` for that task's steps.
   The projection puts `vymalo.signed` on the message and, after the fact, on those steps. The comparison is a **pure
   function** of the verified rows, the message text and the steps' outputs; it lives outside `transition`, and its result
   is an input of the event. A step output the agent shortened for the `steps/v1` bound is compared by the bounded prefix;
   **a difference beyond the bound is not detected**. What a person's client draws from it is the web's decision; the orchestrator never hides the text.
5. **What is stored: the rows as received, in the commit of the message, beside the log and owned by the thread.** They
   are kept verbatim as text (not re-serialised: a number that survives JSON only by accident would change the canonical
   form), with the `kid` and the time of the check. This is durable state beside the log, not the log, and it amends
   invariant 3 (CLAUDE.md). They are **never** in an AG-UI frame, the search index, a notification, a model prompt, or the
   orchestrator's own model calls (title, description); they are erased with the thread and with the account. A fork **does
   not copy them**. An export carries them with the public keys that verified them (by `kid`), so a third party can check
   the export without the orchestrator. *Unverified until T4:* whether a task's message carries the whole transcript, signed
   again under the new run (adam ADR 0068, decision 5). If it does, the store keeps **only the latest run's rows per
   thread** and replaces them at each task (a fork then cuts that run, decision 7); if it carries only that task's new rows,
   the store keeps each run's rows apart.
6. **The keys are the agent's turn keys, looked up by `kid` in the card's set.** The set on the card names the keys adam
   signs turns with (adam ADR 0068, decision 7: not the card's own key, another purpose and rotation). When
   `TURN_SIGNING_KEY_ID` is unset the `kid` is the key's RFC 7638 thumbprint, not a `vault:` reference. These keys are not
   the prompt-statement chain's (ADR 0087). **A run's own certificate (ADR 0101) is outside v1**: adam's form accepts an
   `x5c` member in the header (`crates/adam-core/src/turn/form.rs`) and its T1 amendment (2026-11-25, item 2) says a cloud
   turn key may come from a PKI certificate (`KeyIdentity::Certificate`), with an in-memory per-run key under ADR 0101. v1
   verifies by `kid` against the card's set and **does not trust `x5c`**: a row signed with a run certificate whose `kid` is
   not in the set is refused `unknown_key`. A deployment that wants more pins the agent's turn keys in its configuration;
   a pinned set wins over the card.
7. **A fork sends one run's rows back, under the same subject.** Each task is a new run and adam signs a carried history
   again under it (adam ADR 0068, decision 5), so rows of several tasks are never one chain. A fork whose agent lists the
   URI sends, as `metadata[URI]` of its first message, **the rows of one run, cut at the fork point**, keeping that run's `run`, `cnv`, first `idx` and `prv`, and (proposed) the **source context** (adam's
   decision 6 requires a fork message to name its source context). adam verifies them under the source run and signs them
   again under the new run.
   **Which run, and where the cut is.** A fork point is a cut of the log at the end of a turn (ADR 0029, `fork_cut`). It maps
   to the row paired with that turn's final `agent_message` by the comparison of decision 4: the cut is that row's `idx`,
   inclusive. With **per-run storage** the run is that of the last task at or before the point. In the **whole-transcript
   case** it is the latest run's rows, cut at that `idx`; but adam signs a history again after a compaction or a shortening
   (ADR 0068, decision 5), so if the latest run's prefix up to the cut **no longer matches the history at the fork point**
   (the rows' texts and tool outputs against the log up to the cut), the fork sends **no rows** and continues as text.
   If adam refuses them (`-32602`, for example rows of another person's conversation), or the
   source thread has been deleted ([ADR 0043](0043-deleting-a-thread-erases-it.md)) and there are no rows, the fork
   continues as it does today, with the history as text, and the refusal is logged. A row is never altered by the
   orchestrator to make it verify.
8. **Revocation is learned, not applied backwards.** A key revoked after a row was stored does not delete the row; the row
   keeps the time of its check. Re-verifying stored rows against a newer card is not done by default (**open question 3**).

## Consequences

- A new, optional card read, one activation, one verification path and one place the orchestrator keeps agent-signed bytes.
  With the extension off, which is every deployment today, nothing changes.
- A new store member (the rows, per thread or per task), a closed reason enum, a counter, a `signed` member on two events and
  an export member. None of it is in `chat-api.yaml` until the build adds it (the page lists what).
- A thread with rows holds each tool output twice (as a step, as a row). The deployment turns the extension on or not.
- A person's own messages stay unsigned: the rows prove what the agent said and did, never what the person wrote (ADR 0087
  is the prompt's signature).
- *Unverified*: what subject adam sees on a fork sent by the orchestrator, and so whether rows of a person's own thread are
  accepted when the orchestrator, not the person, makes the call. The scenario of ANR-914 must show it.

## Alternatives rejected

- **Trust the rows without checking them** (store what the agent sends). Then the signature has no reader and a proxy can
  alter a tool result in transit; the point of the extension is the check.
- **Re-sign the rows with the orchestrator's key.** The orchestrator would vouch for what it cannot know; adam's ADR keeps
  the signature the agent's.
- **Fail the job on a bad row.** A transport fault would end a person's work for metadata the chat does not need. The
  deployment that needs it can ask (open question 1).
- **Cache the card's key set.** Against ADR 0008; a revoked key would stay trusted.

## Open questions

1. Should a deployment be able to require rows (fail a job whose agent lists the URI and sends none or bad ones)?
2. What is the configuration key that pins an agent's turn keys, and does the platform's registry publish them (ADR 0101's
   registry item attribute)?
3. Should the orchestrator re-verify stored rows when the card's set changes, and mark rows whose key was revoked since?
4. Should v1 grow to verify a row signed with a run certificate: its `x5c` chain against the agents intermediate (ADR 0101),
   with the leaf's run equal to `run` and the leaf's person equal to the thread's owner?
