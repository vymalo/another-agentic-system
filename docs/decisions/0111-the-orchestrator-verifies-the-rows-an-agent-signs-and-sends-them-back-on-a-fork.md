---
sidebarTitle: "0111 · Verify an agent's signed rows"
---

# ADR 0111 — The orchestrator verifies the signed rows an agent hands it, and sends them back on a fork

- **Status:** **proposed (2026-11-27)**, card ANR-996. No owner decision exists for this record yet; the agent's half is
  accepted (adam-rs ADR 0068). **Nothing is built on either side of the wire**: adam has not built its slice T4, and the
  orchestrator reads no row. **Contract:** [`docs/api/signed-turns-v1.md`](../api/signed-turns-v1.md) (it holds the sequence
  and state diagrams). **Pattern:** [ADR 0008](0008-platform-integration-via-a2a-extension.md) (an optional, live,
  fail-closed extension). **Trust:** [ADR 0087](0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md)
  and [ADR 0101](0101-a-cloud-runs-model-calls-are-signed-for-the-run-and-the-injector-stays-for-custom-cases.md), whose
  keys are different keys (decision 6). The scenario that will prove it, ANR-914's `dev/signed-turns-real-e2e.sh`, is *unbuilt*.

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
| T2a: the engine's seam and the chart's key | T2a built (the engine's seam, the chart's key); the rest of T2 (the server variables) is not marked built in the slice table |
| T3a-1, T3a-2, T3a-3: turns signed at commit, the head signed, every turn verified on re-entry | built (2026-11-26 and 2026-11-27) |
| T3b: the journal's signature | **not built** |
| T4: `signed-turns/v1` itself | **not built**: no URI constant, card entry or message member exists at that revision |
| T5: release N+1, no adoption of unsigned history | not built |

So the rows exist and verify inside adam; nothing hands them to a client. T4 depends on T3 and the T3a part is done
(the pantry note `learned-anr-910` said it was not; it is now).

The orchestrator already trusts a model-call signature chain of its own (the prompt statement, ADRs 0087 and 0101) and
keeps a log that holds steps with their input and output (`steps/v1`). Rows duplicate content the steps already show, so
what is stored and where it is allowed to go is a real decision.

## Decision

1. **It is an optional extension, read live, never cached, removable (ADR 0008).** The orchestrator activates the URI only
   when the agent's live card lists it with well-formed `params`; an agent that does not is plain A2A, and a data part that
   looks like rows is one user turn, as adam's ADR says. The orchestrator never sends rows to an agent whose card does not
   list the URI.
2. **The wire is adam's; the names marked proposed in the page are ours until adam builds T4.** From adam's ADR: the URI,
   `metadata[URI]` on the task's agent message holding the rows with `run`, `cnv`, the first row's `idx` and its `prv`; each
   row's signature at `extra_content.adam.turn_signature`; the header `{"alg":"EdDSA","kid","typ":"adam-turn/v1"}`; the payload
   and the chain. The card's `params` shape and the member names inside `metadata[URI]` are not yet adam's: the page proposes
   them and marks them so.
3. **The orchestrator verifies every row it receives, at ingest, against the key set of the live card it read for that
   send.** It checks: the header (`alg` EdDSA only, `typ` exactly `adam-turn/v1`, `kid` in the set and not `revoked`, no
   other critical member), the detached payload rebuilt from the row, `run`, `cnv`, `idx` and `prv` as given, and the
   signature. Across rows in one hand it recomputes the chain, so a reordered, dropped or altered row breaks every later
   one. The first row's `prv` is taken as given: what verifies is that adam signed that row at that position over that
   prefix hash, not that the orchestrator holds the prefix.
4. **A row that fails is refused and logged, never shown as the agent's.** Nothing of the refused rows is stored,
   projected, exported or sent back. The log gets one entry per refusal (a closed reason, `kid`, `idx`, the task id; **never
   a row's content or a signature**). The task itself is not failed: its message and its steps are plain A2A and are shown as
   before, with no "signed" mark. Whether a deployment may require rows and fail the job without them is **open question 1**.
5. **What is stored: the rows as received, in the commit of the message, beside the log and owned by the thread.** They
   are kept verbatim as text (not re-serialised: a number that survives JSON only by accident would change the canonical
   form), with the `kid` and the time of the check. They are **never** in an AG-UI frame, the search index, a notification
   or a model prompt; they are erased with the thread and with the account. A fork **does not copy them**: a fork's rows come
   from its own tasks. An export carries them with the public keys that verified them (by `kid`), so a third party can
   check the export without the orchestrator.
6. **The keys are the agent's turn keys, not the platform's.** The JWKS on the card names the key adam signs turns with
   (adam ADR 0068, decision 7: not the card's own key, another purpose and rotation). They are not the prompt-statement
   chain's keys (ADR 0087) and not a run's certificate (ADR 0101). The three never stand for each other, and `typ` keeps
   them apart. What the card's JWKS proves is only as much as the card read does: the orchestrator reads the card from the
   URL it was configured with, over the transport it already requires. A deployment that wants more pins the agent's turn
   keys in its configuration; when it does, a key not in the pinned set is refused whatever the card says. The pin's name
   is **open question 2**.
7. **A fork sends the rows back, from the source thread's own rows, under the same subject.** When a thread forked at a
   point has stored rows and the agent's live card lists the URI, the fork's first message activates it and carries the
   contiguous rows from the start of the chain to the last row at or before the fork point. adam verifies them under the
   source run and signs them again under the new run, so every signature of a run names that run. If adam refuses them
   (`-32602`, for example because the rows belong to another person's conversation), the fork continues as it does today,
   with the history as text, and the refusal is logged. A row is never altered by the orchestrator to make it verify.
8. **Revocation is learned, not applied backwards.** A key revoked after a row was stored does not delete the row; the
   row keeps the time of its check. Re-verifying stored rows against a newer card is not done by default (**open question 3**).

## Consequences

- A new, optional card read, one activation, one verification path and one place the log holds agent-signed bytes. With the
  extension off, which is every deployment today, nothing changes.
- A new store member (the rows, per task) in each store adapter, a reason enum for refusals, and an export member. None of
  it is in `chat-api.yaml` until the build adds it (the page lists what).
- A thread with rows holds each tool output twice (as a step, as a row). The bound is adam's: ADR 0068 says about 220
  bytes of signature per row; the content is the cost, and the deployment turns the extension on or not.
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
