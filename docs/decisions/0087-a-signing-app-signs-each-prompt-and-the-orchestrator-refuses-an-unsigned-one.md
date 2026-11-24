# ADR 0087 — A signing app signs each prompt, and the orchestrator refuses an unsigned one from it

- **Status:** **proposed (2026-11-24)**, on the owner's decision of 2026-11-23: the phone and desktop apps sign each
  prompt a person sends with a **prompt key**; the desktop's embedded agent loops sign each turn with a separate **turn
  key** ([adam-rs ADR 0068](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0068-adam-signs-its-own-history.md),
  `TurnKeys`); **the orchestrator checks prompt signatures and refuses an unsigned or badly signed prompt** from those
  apps; **the web app does not sign.** What follows (names, shapes, limits, codes) is the proposal that builds that
  decision, which the owner may change by a dated note. **Nothing is built.** Extends
  [ADR 0077](0077-push-notifications-for-a-persons-runs.md) (a person's devices),
  [ADR 0047](0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md) (one public client per
  platform) and [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md) (who the person is).
  *Revised 2026-11-24 after a security review of the first draft (merged the same day): who must sign now follows the
  token's client, not a header the client chooses; key changes are signed by an existing key; the payload carries
  everything the verifier needs and binds the deployment and the run's options; a refusal is a `403`; resumes are
  signed too. The first draft is in this file's history.*

## Context

A prompt reaches the orchestrator as the last user message of an AG-UI run, `POST /agui/agents/{agentId}`
([`api/agui.md`](../api/agui.md)), with the person's bearer token. The token says **who** sent it. It does not say that
the text the orchestrator received is the text the person wrote on their own device: a proxy, a compromised relay or a
token stolen from one device can send any text, and any run option, in the person's name.

The owner wants the apps that hold a key of their own, the phone and the desktop app, to prove that each prompt came
from that device, and the orchestrator to refuse a prompt that should be signed and is not. The web app runs in a
browser that keeps no device key, and the owner left it out.

Two signatures exist on the desktop, and they are different things:

- the **prompt signature** (this record): the app signs what the person sent; the orchestrator checks it;
- the **turn signature** (adam-rs ADR 0068): the embedded adam signs its own history so that a turn it re-reads is one
  it wrote. It never leaves the device and the orchestrator never sees it.

Different keys keep them apart, and the `typ` of each signature keeps them apart even if one key were used for both
(open question 1).

## Decision

1. **Who must sign: the token's client, not the request.** ADR 0047 gives each platform its own public OAuth client.
   The orchestrator's configuration lists the **signing clients**, `auth.signingClients` (proposed), for example the
   desktop, iOS and Android clients. A request whose token was issued to a listed client (its `azp`, or `client_id`
   when there is no `azp`) **must** carry a valid prompt signature. A request from any other client (the web's) is not
   asked for one, and none is checked. A relay or a thief can't take the web's path by dropping a header, because the
   token still names the app it was issued to.

2. **What is signed.** The client signs each user message with a JWS in compact form with a **detached payload**
   (RFC 7515, appendix F), the construction adam-rs ADR 0068 uses for its turns.
   - The **protected header** is `{"alg":"EdDSA","kid":"<key id>","typ":"prompt/v1","nonce":"<16 random bytes,
     base64url>","iat":<seconds since the epoch>}`. The nonce and the time are in the header, so they are signed, and
     the verifier reads them from there.
   - The **payload**, never sent, is the RFC 8785 canonical JSON of
     `{typ, aud, thread, agent, msg, content_sha256, props_sha256, nonce, iat}`. The verifier rebuilds it from the
     request and the header:
     - `typ` is `"prompt/v1"`, and must equal the header's `typ`;
     - `aud` is the deployment's API origin, as `GET /api/config` serves it, so a signature can't be replayed on
       another deployment;
     - `thread` is the thread id; `agent` is the `agentId` path segment **as sent**, not the agent an alias resolves to
       ([ADR 0049](0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md));
     - `msg` is the AG-UI message id;
     - `content_sha256` is `base64url(SHA-256(JCS(content)))`, where `content` is the message's `content` member
       exactly as received, a string or an array, every part included (an image too, though the orchestrator may skip
       it). There is no Unicode normalisation; a string RFC 8785 refuses (a lone surrogate) is `malformed`;
     - `props_sha256` is `base64url(SHA-256(JCS(forwardedProps)))`, or of `{}` when the run has none. It binds every
       option the person chose with the message: the access tier, the model and reasoning pick, the attached tools, the
       mentions, `vymalo.send`, the fork and gate options, the locale, and any future member.
   - The client sends the compact JWS as the message's metadata member `vymalo.promptSignature`.
   - The orchestrator also checks that the device the `kid` names belongs to the authenticated person.

3. **What else is a prompt.** Anything the run route turns into the person's words is signed the same way:
   - a `resume` entry whose payload becomes a user message (`status: "resolved"` with text) carries
     `vymalo.promptSignature` in its metadata, with `msg` replaced by the `interruptId` and `content` by the payload;
   - the other routes that write a `user_message` (an MCP `start_job`, a person starting a helper session, any steer
     path) are listed in P1's contract, each marked signed or not signed, with the reason.

   Whether an answer to a form or a confirm (`a2uiAction`) is a prompt is **open question 3**. Until the owner
   answers, a signing client's `a2uiAction` is accepted unsigned, and P1 says so.

4. **Replay.** The orchestrator refuses a header whose `iat` is more than **300 seconds** in the past or **60 seconds**
   in the future of its clock. It records `(kid, nonce)` **in the same transaction** as the `user_message`, under a
   unique constraint in the shared store, so every replica sees it. A second request with the same nonce **and** the
   same `thread`, `msg` and `content_sha256` is the honest retry AG-UI already allows, and attaches to the first. Any
   other reuse of a nonce is refused. Records older than 10 minutes are dropped.

5. **The device's key: registration, rotation and revocation.** A **prompt key** is an Ed25519 public key (a JWK) kept
   beside a device. A device may exist **without** a push channel: ADR 0077's device gains a `promptKey`, and a device
   needs a push address, a prompt key, or both. When a push provider answers `404` or `410`, the orchestrator drops only
   the push address and keeps the prompt key.
   - The `kid` is `<device id>#v<n>`. `POST` and `PATCH` return it.
   - **The first key** a person registers needs a recent sign-in, a token whose `auth_time` is at most 10 minutes old,
     and **proof of possession**: the request carries a signature by the new private key over
     `{typ:"prompt-key/v1", aud, device, jwk_thumbprint, nonce, iat}`.
   - **Every later key** (a new device, or a rotation) also needs a signature by a **current** prompt key of the same
     person over the same object. So a stolen token alone can't add or swap a key once the person has one.
   - **Recovery,** when every key is lost, is a recent sign-in, and removing the lost devices first.
   - **Rotation:** the new version is current at once, and the previous one verifies for 10 more minutes, unless the
     rotation says `compromised: true`, in which case it stops at once.
   - **Revocation:** `DELETE /api/me/devices/{id}`, which needs only the token, so that revoking stays easy, removes the
     device's keys at once.
   - A person has at most the 10 devices ADR 0077 allows. Adding one needs the step-up above, so a thief can't fill them.

6. **Which key verifies.** The `kid` in the header names the key. A request from a signing client may also send the
   device id as `X-Vymalo-Device`, the name ADR 0077 already uses, to help the client and the logs. It never decides
   whether a signature is required (decision 1). `server.cors` allows the header, since the desktop app calls from its
   own origin.

7. **The refusal.** A missing, malformed, expired, replayed or wrong signature from a signing client is a **`403`**
   problem with `code: prompt_signature_invalid`. It is not a `401`, which only the identity layer answers (ADR 0033,
   `tests/only_identity_answers_401.rs`), and which would make the app refresh its token and ask the person to sign in
   again. Nothing is written to the thread and no agent is called.
   - The client sees `reason: expired` or `reason: invalid`. The finer reason (`missing`, `malformed`, `replayed`,
     `unknown_key`, `revoked_key`, `not_owner`, `bad_signature`) goes to the orchestrator's log, without the
     signature's value.
   - The person sees "This message couldn't be verified as coming from your device. Please try again."
   - The app re-signs and resends once on `expired`, and stops otherwise.

8. **A person may require signatures everywhere.** `requireSignedPrompts`, a setting of the person's (proposed), **off
   by default**, refuses their unsigned prompts from any client, the web included. So the web can no longer send for
   them. Turning it **off** needs a signature by a current prompt key, as in decision 5, so a stolen token can't turn
   it off.

## What it protects, and what it does not

| Threat | Protected? |
|---|---|
| A relay changes a prompt, or its options, sent by a signing app | **yes**: the content and the options are bound, and the token's client still requires a signature |
| A signed prompt is replayed, or moved to another thread, agent or deployment | **yes**: the nonce, the time, `thread`, `agent` and `aud` |
| A token stolen from a signing app is used from elsewhere | **yes** for prompts sent with that token (its client must sign); the thief can't register a key (decision 5) |
| Someone with the person's sign-in uses the web app | **only with `requireSignedPrompts` on**; the web doesn't sign, by the owner's decision |
| A compromised device with its key | **no**: the device's key signs whatever it is given |
| A form or confirm answer from a signing app | **not yet**: open question 3 |

## Slices

| # | What | Decisive test |
|---|---|---|
| P1 | The contract page `docs/api/prompt-signature-v1.md`: the header, the payload's rebuild, the routes that write a `user_message` and which are signed, test vectors made by an independent implementation (a string, a multi-part array, a non-ASCII string, an image part, a `forwardedProps` object) | the vectors verify; one changed byte of the content or of an option fails them |
| P2 | `auth.signingClients`; `promptKey` on the device routes, a device without a push channel, the step-up and proof of possession, rotation and revocation (an append-only migration) | a second key with only a token is refused; a rotated key verifies for 10 minutes, a compromised one never; a push `410` keeps the key |
| P3 | The check on the run route and on a signed resume: the window, the nonce under its unique constraint in the message's transaction, the honest retry, the `403` | each reason has a test; a signing client's request with no signature is refused and nothing is written; a web client's is not checked; two concurrent retries attach |
| P4 | `requireSignedPrompts`, and turning it off with a signature | with it on, a web request is refused; turning it off with only a token is refused |
| P5 | A scenario on mocks in `dev/` | a signed message is accepted; a tampered text, a tampered option and a replay are refused |

## Open questions

1. **Which key is the prompt key: the app's existing device key or a new one?** The app side and the owner decide. If
   one key also signs something else (for example DPoP proofs), the verifier must check `typ`, which decision 2 does.
2. Whether `requireSignedPrompts` should default to on for a person who has a signing device. That would ask every such
   person to give up the web app, a product choice.
3. **Is an answer to a form or a confirm (`a2uiAction`) a prompt that must be signed?** Some confirm a hand-off or a
   gate. The owner decides.
4. Whether a thread that has received a signed message should refuse unsigned ones afterwards, even from the web. A
   product choice for the owner.

## Consequences

- The orchestrator keeps one more kind of durable state beside the log: the devices' public prompt keys, and for 10
  minutes the nonces it has seen. Neither is a secret, and neither is in the log.
- A device can exist without push, so ADR 0077's "a device is a push registration" becomes "a device is a push
  registration, a prompt key, or both".
- A signing app keeps a private key in the platform's keychain (open question 1).
- An AG-UI client that isn't a listed signing client is unchanged.
