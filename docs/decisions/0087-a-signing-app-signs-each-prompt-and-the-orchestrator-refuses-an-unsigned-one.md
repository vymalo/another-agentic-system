# ADR 0087 — A signing app signs each prompt, and the orchestrator refuses an unsigned one from it

- **Status:** **proposed (2026-11-24)**, on the owner's decision of 2026-11-23: the phone and desktop apps sign each
  prompt a person sends with a **prompt key**; the desktop's embedded agent loops sign each turn with a separate **turn
  key** ([adam-rs ADR 0068](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0068-adam-signs-its-own-history.md),
  `TurnKeys`); **the orchestrator checks prompt signatures and refuses an unsigned or badly signed prompt** from those
  apps; **the web app does not sign.** What follows (names, shapes, limits, codes) is the proposal that builds that
  decision, which the owner may change by a dated note. **Nothing is built.** Open question 1 below is deliberately left
  open. Extends [ADR 0077](0077-push-notifications-for-a-persons-runs.md) (a person's devices) and
  [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md) (who the person is).

## Context

A prompt reaches the orchestrator as the last user message of an AG-UI run, `POST /agui/agents/{agentId}`
([`api/agui.md`](../api/agui.md)), with the person's bearer token. The token says **who** sent it. It does not say that
the text the orchestrator received is the text the person wrote on their own device: a proxy, a compromised relay or a
token stolen from one device can send any text in the person's name.

The owner wants the apps that hold a key of their own, the phone and the desktop app, to prove that each prompt came
from that device, and the orchestrator to refuse a prompt that should be signed and is not. The web app runs in a
browser that keeps no device key, and the owner left it out.

Two signatures exist on the desktop, and they are different things:

- the **prompt signature** (this record): the app signs what the person sent; the orchestrator checks it;
- the **turn signature** (adam-rs ADR 0068): the embedded adam signs its own history so that a turn it re-reads is one
  it wrote. It never leaves the device and the orchestrator never sees it.

Different keys keep them apart: a leaked turn key cannot sign a prompt, and the reverse.

## Decision

1. **What is signed.** A signing app signs each user message it sends, with a **detached-payload JWS** (RFC 7515,
   appendix F), the construction adam-rs ADR 0068 already uses:
   - the header is `{"alg":"EdDSA","kid":"<device key id>","typ":"prompt/v1"}`;
   - the payload, never sent, is the RFC 8785 canonical JSON of
     `{typ, thread, agent, msg, content_sha256, nonce, iat}`, where:
     - `typ` is `"prompt/v1"`;
     - `thread` is the thread id the message goes to, and `agent` the `agentId` of the route, so a signature can't be
       moved to another thread or agent;
     - `msg` is the AG-UI message id;
     - `content_sha256` is the SHA-256, base64url, of the message's content as the client serialised it, each part
       canonicalised as RFC 8785 JSON;
     - `nonce` is 16 random bytes, base64url;
     - `iat` is the signing time, seconds since the epoch.
   - The client sends the compact JWS in the message's metadata, `vymalo.promptSignature`.

2. **Replay.** The orchestrator refuses a signature whose `iat` is more than **300 seconds** in the past or **60
   seconds** in the future of its clock. It keeps each accepted `(kid, nonce)` for 10 minutes and refuses one it has
   seen. A message whose id the thread already holds is not a new prompt: the AG-UI surface reconciles messages by id
   (closed question 19), so a client that re-sends its transcript is not asked to sign the old messages again. Only the
   new user message of a run is checked.

3. **Which key.** A device's **public** prompt key (Ed25519, as a JWK) is registered with the device, through the
   routes ADR 0077 already gives every client: `POST /api/me/devices` gains an optional `promptKey`. The
   orchestrator stores the public key beside the device, not in the log. **No private key ever leaves the device.**
   - The `kid` is the device's id and a version, `<device id>#v<n>`.
   - **Rotation:** `PATCH /api/me/devices/{id}` with a new `promptKey` makes version `n+1` current. The previous version
     verifies for 10 more minutes, then never again.
   - **Revocation:** removing the device (`DELETE /api/me/devices/{id}`) revokes its key at once. A future "sign
     this device out" control should do the same.
   - A person has at most the 10 devices ADR 0077 allows, so at most 10 prompt keys.

4. **Who must sign.** A person's message must be signed **when the person has at least one device with a prompt key
   and the request names one of them**, by sending its device id in the header `Vymalo-Device`. That is how a client
   says it is a signing app:
   - a request **with** `Vymalo-Device` naming a device that has a prompt key: the message must carry a valid signature
     by that device's current key, or it is refused;
   - a request **without** the header (the web app): no signature is asked for, and none is checked if one is sent.

   So an attacker holding only the person's token can still send unsigned prompts by leaving the header out. That is
   the web app's path, and it stays open because the owner left the web out. Decision 6 says what closes it.

5. **The refusal.** A missing, malformed, expired, replayed or wrong signature is a `401` problem with
   `code: prompt_signature_invalid` and a `reason` among `missing`, `malformed`, `expired`, `replayed`,
   `unknown_key`, `revoked_key` and `bad_signature`. Nothing is written to the thread and no agent is called. The person
   sees "This message couldn't be verified as coming from your device. Please try again." The app re-signs and resends
   once on `expired`, and stops on any other reason.

6. **A person may require signatures.** `PATCH /api/me/settings` (proposed) gains `requireSignedPrompts`, **off by
   default**. When it is on, every message from that person must be signed by one of their devices, so the web app can
   no longer send for them. This is the person's choice, not a deployment default, because the owner decided that the
   web app doesn't sign.

## What it protects, and what it does not

| Threat | Protected? |
|---|---|
| A relay or proxy changes a prompt sent by a signing app | **yes**: the content hash no longer matches |
| A signed prompt is replayed, or moved to another thread or agent | **yes**: the nonce, the time and the `thread` and `agent` it names |
| A token stolen from a signing app is used from elsewhere | **only with `requireSignedPrompts` on**; otherwise the thief sends without the header, as the web does |
| A compromised device with its key | **no**: the device's key signs whatever it is given |
| The web app's own prompts | **no**: the web doesn't sign (the owner's decision) |

## Slices

| # | What | Decisive test |
|---|---|---|
| P1 | The contract page `docs/api/prompt-signature-v1.md` with test vectors (a JWS and its payload made by an independent implementation) | the vectors verify, and change one byte of the content and they fail |
| P2 | `promptKey` on the device routes, its storage (an append-only migration), rotation and revocation | a rotated key verifies for 10 minutes and then is refused; a removed device's key is refused at once |
| P3 | The check on `POST /agui/agents/{agentId}`: the header, the signature, the clock window, the nonce record, the problem | each `reason` has a test; an unsigned message with the header is refused and nothing is written; without the header, nothing is checked |
| P4 | `requireSignedPrompts` | with it on, an unsigned message without the header is refused |
| P5 | A scenario on mocks in `dev/` | a signed message is accepted, a tampered one refused, a replay refused |

## Open questions

1. **Which key is the prompt key: the app's existing device key or a new one?** The app side and the owner decide. This
   record only needs an Ed25519 public key per device and does not depend on the answer.
2. Whether `requireSignedPrompts` should default to on for a person who has registered a signing device. Asking every
   person to give up the web app is a product choice.

## Consequences

- The orchestrator keeps one more kind of durable state beside the log: a device's public prompt keys, and for 10
  minutes the nonces it has seen. Neither is a secret, and neither is in the log.
- A signing app must keep a private key in the platform's keychain. The app decides which one (open question 1).
- An AG-UI client that doesn't sign is unchanged.
