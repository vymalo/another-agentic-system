---
sidebarTitle: "0087 · A signing app signs each prompt"
---

# ADR 0087 — A signing app signs each prompt, and the orchestrator refuses an unsigned one from it

- **Status:** **proposed (2026-11-24)**, on the owner's decision of 2026-11-23: the phone and desktop apps sign each
  prompt a person sends with a **prompt key**; the desktop's embedded agent loops sign each turn with a separate **turn
  key** ([adam-rs ADR 0068](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0068-adam-signs-its-own-history.md),
  `TurnKeys`); **the orchestrator checks prompt signatures and refuses an unsigned or badly signed prompt** from those
  apps; **the web app does not sign.** What follows (names, shapes, limits, codes) is the proposal that builds that
  decision, which the owner may change by a dated note. **Nothing is built.** *Amended (2026-11-27): parts are built since; see [the status note of 2026-11-27](#status-note-2026-11-27-what-is-built).* Extends
  [ADR 0077](0077-push-notifications-for-a-persons-runs.md) (a person's devices),
  [ADR 0047](0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md) (one public client per
  platform) and [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md) (who the person is).
  *Revised 2026-11-24 after a security review of the first draft (merged the same day): who must sign now follows the
  token's client, not a header the client chooses; key changes are signed by an existing key; the payload carries
  everything the verifier needs and binds the deployment and the run's options; a refusal is a `403`; resumes are
  signed too. The first draft is in this file's history.*
  **Amended 2026-11-25 (the cloud path), on the owner's decisions of that day:** on the cloud path the orchestrator
  **no longer verifies the person's signature**. It checks **presence** at intake, attests, and the verifier behind the
  model gateway's authorizer verifies:
  - **Presence at intake.** A prompt from a signing client (decision 1) that carries no `vymalo.promptSignature`, or
    one that is not a three-part compact JWS, is refused with decision 7's `403` before anything is written or any model
    call is made. No cryptography is done there.
  - **The orchestrator attests.** It passes the person's envelope on byte for byte and signs a **prompt statement** of
    its own: a compact JWS with an attached RFC 8785 payload of `typ`, `aud` (the model gateway's origin), `iat`, `exp`
    (at most a day after `iat`, never after its certificate ends), `jti`, `src` (`chat`, `resume`, `schedule`, `webhook`
    or `mcp`), `sub`, `azp`, `thread`, `agent` (as resolved), `a2a`, `content_sha256` (of the A2A parts as sent) and
    `user`. `user` is `null` with no envelope, or holds `env_sha256` (the SHA-256 of the envelope's exact bytes) and the
    members of decision 2's payload that only the chat request holds: `aud`, `thread`, `agent` (as sent), `msg`,
    `content_sha256`, `props_sha256`. Its key is certified by the deployment's PKI (Ed25519, 72 hours, renewed at 48;
    `x5c` in the header).
  - **The integrity rule.** Before it signs, it recomputes `content_sha256` and `props_sha256` from the request it
    received and compares them, with `thread`, `agent` and `msg`, against `user`. A difference is decision 7's
    `403 prompt_signature_invalid`, reason `invalid`, and nothing is signed. It never verifies the envelope's signature.
  - **The verifier verifies**, in one place for every signature: the statement's chain, the person's envelope against
    `user`, the person's key and its owner, and replay. So on this path the checks of decisions 4 to 6 that need the
    person's key or the nonce are the verifier's, not the orchestrator's. A person's key may be Ed25519 or ES256.
  - Decisions 1, 2, 3 and 7 stand. The device path, where an app's own orchestrator holds the key, is unchanged. The
    statement's `typ` (`prompt-statement/v1`) and the header names are proposals until the shared signature contract
    fixes them.
  - **Built:** `orch-prompt-statement` (the payload, RFC 8785 and the integrity rule; pure), the `PromptSigner` port in
    `orch-ports` with `prompt_signer_conformance!`, and `orch-prompt-signer-static` (a test signer from files), held to
    the shared test vectors. **Not built:** the PKI signer, the intake check, the statement on the agent's messages.

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
   when there is no `azp`) **must** carry a valid prompt signature. A relay or a thief can't take the web's path by
   dropping a header, because the token still names the app it was issued to.
   - **It fails closed.** A token from the apps' issuer that carries neither `azp` nor `client_id` is treated as a
     signing client's. The orchestrator logs the list at startup, and in a `production` profile it refuses to start
     when the list is empty or names the web's client.
   - **A signature is checked whenever one is present**, from any client. A signed prompt from a client that isn't
     listed is accepted and logged as a warning, which shows a misconfigured list instead of hiding it.
   - The web's client is not asked for a signature.

2. **What is signed.** The client signs each user message with a JWS in compact form with a **detached payload**
   (RFC 7515, appendix F), the construction adam-rs ADR 0068 uses for its turns.
   - The **protected header** is `{"alg":"EdDSA","kid":"<key id>","typ":"prompt/v1","nonce":"<16 random bytes,
     base64url>","iat":<seconds since the epoch>}`. The nonce and the time are in the header, so they are signed, and
     the verifier reads them from there.
   - The **payload**, never sent, is the RFC 8785 canonical JSON of
     `{typ, aud, thread, agent, msg, content_sha256, props_sha256, nonce, iat}`. The verifier rebuilds it from the
     request and the header:
     - `typ` is `"prompt/v1"`, and must equal the header's `typ`;
     - `aud` is the deployment's canonical API origin, so a signature can't be replayed on another deployment. The
       server takes it from its configuration (`server.publicOrigin`, proposed), **never** from `Host` or
       `X-Forwarded-*`. The client takes it from its own runtime `config.json` (ADR 0047), not from the server's
       answer. Its normal form is lower-case scheme and host, no default port, and no trailing slash;
     - `thread` is the thread id; `agent` is the `agentId` path segment **as sent**, not the agent an alias resolves to
       ([ADR 0049](0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md));
     - `msg` is the AG-UI message id;
     - `agent` is compared after percent-decoding the path segment, once;
     - `content_sha256` is `base64url(SHA-256(JCS(content)))`, where `content` is the message's `content` member
       exactly as received, a string or an array, every part included (an image too, though the orchestrator may skip
       it), hashed as a generic JSON value before any typed parsing. There is no Unicode normalisation; a string RFC
       8785 refuses (a lone surrogate) is `malformed`;
     - `props_sha256` is `base64url(SHA-256(JCS(forwardedProps)))`, or of `{}` when the run has none. It binds every
       option the person chose with the message: the access tier, the model and reasoning pick, the attached tools, the
       mentions, `vymalo.send`, the fork and gate options, the locale, and any future member.
   - The client sends the compact JWS as the message's metadata member `vymalo.promptSignature`.
   - The orchestrator also checks that the device the `kid` names belongs to the authenticated person, and that the
     payload's `nonce` and `iat` equal the header's.

3. **What else is a prompt.** Anything the run route turns into the person's words is signed the same way:
   - a `resume` entry whose payload becomes a user message (`status: "resolved"` with text) carries
     `vymalo.promptSignature` in its metadata, with `msg` replaced by the `interruptId` and `content` by the whole
     payload object;
   - every other route that writes a person's `user_message` (starting a helper session, any steer path) is listed in
     P1's contract. **From a signing client, each one is signed or refused.** An exception goes to the owner as an
     open question, not into the table. An MCP static token names no signing client, so it isn't one.

   Whether an answer to a form or a confirm (`a2uiAction`) is a prompt is **open question 3**. Until the owner
   answers, a signing client's `a2uiAction` is accepted unsigned, and P1 says so.

4. **Replay.** The orchestrator refuses a header whose `iat` is more than **300 seconds** in the past or **60 seconds**
   in the future of its clock. It records `(kid, nonce)` **in the same transaction** as the `user_message`, under a
   unique constraint in the shared store, so every replica sees it. The signature, `props_sha256` included, is
   verified **before** a request is judged a retry. A second request with the same nonce **and** the same `thread`,
   `msg` and `content_sha256` is then the honest retry AG-UI already allows: it attaches to the first, applies no
   option and writes nothing. Any other reuse of a nonce is refused. The nonce records aren't erased with a thread
   (ADR 0043); they live until they are 10 minutes old, so deleting and re-creating a thread doesn't reopen a replay.

5. **The device's key: registration, rotation and revocation.** A **prompt key** is an Ed25519 public key (a JWK) kept
   beside a device. A device may exist **without** a push channel: ADR 0077's device gains a `promptKey`, and a device
   needs a push address, a prompt key, or both. When a push provider answers `404` or `410`, the orchestrator drops only
   the push address and keeps the prompt key.
   - The `kid` is `<device id>#v<n>`. `POST` and `PATCH` return it.
   - **Every new key**, the first or not, a new device or a rotation, needs two things:
     - **a fresh sign-in:** a token whose `auth_time` is at most 10 minutes old. A token without `auth_time` is refused.
       The app asks the issuer with `max_age=600` for this step, so a live single sign-on session can't hand it an old
       `auth_time`, and the refusal says "sign in again";
     - **proof of possession:** a signature by the new private key over
       `{typ:"prompt-key/v1", aud, device, jwk_thumbprint, compromised, nonce, iat}`.
   - **The fresh sign-in is the real bar.** A thief who has only a token can't add, swap or rotate a key. A thief with a
     fresh sign-in can, as the person can; that is the same trust as signing in. A token thief can still delete the
     person's devices, since revoking needs only the token (below): that denial of service is accepted.
   - **Recovery,** when every key is lost, is the same: a fresh sign-in and a new key.
   - **Rotation:** the new version is current at once, and the previous one verifies for 10 more minutes, unless
     `compromised` is `true` in the signed object, in which case it stops at once.
   - **Revocation:** `DELETE /api/me/devices/{id}`, which needs only the token so that revoking stays easy, removes the
     device and its keys at once. `PATCH` can remove the push address alone, so withdrawing push consent doesn't cost
     the person their key.
   - A person has at most the 10 devices ADR 0077 allows. Each needs a fresh sign-in, so a token thief can't fill them.

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
   them. Turning it **on** needs a current prompt key, and turning it **off** needs a signature by one, so a stolen
   token can neither turn it off nor use it to lock the person out of the web.

## What it protects, and what it does not

| Threat | Protected? |
|---|---|
| A relay changes a prompt, or its options, sent by a signing app | **yes**: the content and the options are bound, and the token's client still requires a signature |
| A signed prompt is replayed, or moved to another thread, agent or deployment | **yes**: the nonce, the time, `thread`, `agent` and `aud` |
| A token stolen from a signing app is used from elsewhere | **yes** for prompts sent with that token (its client must sign); without a fresh sign-in the thief can't register a key (decision 5), but can delete the person's devices |
| Someone with the person's sign-in uses the web app | **only with `requireSignedPrompts` on**; the web doesn't sign, by the owner's decision |
| A compromised device with its key | **no**: the device's key signs whatever it is given |
| A form or confirm answer from a signing app | **not yet**: open question 3 |

## Slices

| # | What | Decisive test |
|---|---|---|
| P1 | The contract page `docs/api/prompt-signature-v1.md`: the header, the payload's rebuild, the routes that write a `user_message` and which are signed, test vectors made by an independent implementation (a string, a multi-part array, a non-ASCII string, an image part, a `forwardedProps` object with numbers in it, a resume payload) | the vectors verify; one changed byte of the content or of an option fails them |
| P2 | `auth.signingClients` and its fail-closed rules; `server.publicOrigin`; `promptKey` on the device routes, a device without a push channel, the fresh sign-in and proof of possession, rotation and revocation (an append-only migration); dated notes on ADR 0077 (key-only devices, the dispatcher skips them) and ADR 0047 (CORS allows `X-Vymalo-Device`) | a key with an old `auth_time` or none is refused; a rotated key verifies for 10 minutes, a compromised one never; a push `410` keeps the key; an empty list refuses to start in `production` |
| P3 | The check on the run route and on a signed resume: the window, the nonce under its unique constraint in the message's transaction, the signature before the retry rule, the `403` added to `runAgent` in `chat-api.yaml` | each reason has a test; a signing client's request with no signature is refused and nothing is written; a web client's is not checked; two concurrent retries attach; a deleted and re-created thread doesn't accept a replay |
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
5. *Unverified:* that the deployment's identity provider puts `auth_time` in the access tokens it issues, and honours
   `max_age`. P2 checks it first. If it doesn't, the fresh-sign-in bar needs another source, for example an ID token
   the app sends with the key's registration.

## Consequences

- The orchestrator keeps one more kind of durable state beside the log: the devices' public prompt keys, and for 10
  minutes the nonces it has seen. Neither is a secret, and neither is in the log.
- A device can exist without push, so ADR 0077's "a device is a push registration" becomes "a device is a push
  registration, a prompt key, or both".
- A signing app keeps a private key in the platform's keychain (open question 1).
- An AG-UI client that isn't a listed signing client is unchanged.

## Status note, 2026-11-27: what is built

*Verified 2026-11-27 against `main` (`a6fb6d7`).* What the amendment of 2026-11-25 lists as built is on `main`:
`orch-prompt-statement` (the statement's payload, RFC 8785 and the integrity rule; pure), the `PromptSigner` port with
`prompt_signer_conformance!` in `orch-ports`, and `orch-prompt-signer-static`, a test signer from files
([#530](https://github.com/vymalo/another-agentic-system/pull/530)). Since then
[#535](https://github.com/vymalo/another-agentic-system/pull/535) added `auth.signingClients` to the configuration with
its rules, and the presence check of a signing client's envelope as a pure function
(`orchestrator/crates/prompt-statement/src/presence.rs`). Nothing calls that check at intake yet, there is no PKI
signer, and no statement goes with an agent's messages: no prompt is refused or attested today.

*Added 2026-11-28 (ANR-1338, slices 1 and 2; the ordering and the gaps below as of the security review of the same
day):* the check is called at intake. `Principal.client_id` carries the token's `azp`, else its `client_id`, else
`Some("")` for a token that names neither — **by presence, not readability**, so an `azp` that is there and unusable
is the sentinel and `client_id` is not consulted — and `requires_signature` decides from `auth.signingClients`.

On the AG-UI run route the check runs **after the translation and before any write**, and only for an input that
writes the person's words (`Input::UserMessage`, `Input::StopAndSend`): an attach, a cancel and an A2UI action write
none and are never refused for a missing signature, and an unlisted agent answers its 404 before the check. The
envelope is read from the **new message's `metadata["vymalo.promptSignature"]`**, and for an answer from the
**`resume` entry's `metadata`** (decision 3) — not from `forwardedProps`, which the statement hashes whole as
`props_sha256`. It is passed on as it arrived to `Input::UserMessage.prompt_signature`, the delegation's outbox row
and the dispatcher's `SendRequest`, and it is in no event, frame, step, export or log line. **Nothing reads it off
`SendRequest` yet**: the statement and the A2A wire are ANR-903, so today the bytes are kept for the send path and
nothing more. The presence check runs before the message rate limit and the allowance, so an unsigned message that is
also over the limit is the `403` and is not counted; the client-version refusal is a layer in front of the whole API
and comes first.

**Recorded, not built — every route that still writes a person's words unsigned** (the AG-UI run route is the only
one that checks, and an answer on it is checked from its `resume` entry since the B1 fix):

- **The REST fork and edit** (`POST /api/threads/{id}/fork`, both `after` with `text` and `replace`) writes and
  delegates the person's words without an envelope, and the body has no metadata member to carry one: **ANR-1375**.
- **The helper-session start** (`POST /api/threads/{id}/sessions`) writes the person's first round to a helper with
  no envelope, and its body has no metadata member either: **ANR-1383**.
- **A Stop & send's held text** (`Job.after_stop`) is unsigned: the job ledger is part of the export, so the
  envelope is not stored there, and the next job's delegation — and a stop the agent refuses that becomes a steer —
  carry none: **ANR-1376**.

Still not built: the PKI signer, the statement on an agent's messages, and the `production` rule that refuses an
empty `auth.signingClients`.

*Verified 2026-11-28 by reading the code of this change (`23c667ee` plus the review's fixes); nothing was built or
run on the reviewing host (owner ANR-Q79), so CI and the compose build are the compiler. Unverified: the five new
route tests and the `azp` test have not been executed locally.*

*Added 2026-11-28 (ANR-1383): the helper-session start is checked.* The second gap above is closed.
`POST /api/threads/{id}/sessions` takes the envelope in its body's `metadata["vymalo.promptSignature"]` and runs the
run route's check, now one function for both (`orch_api::check_prompt_signature`), with the same list
(`auth.signingClients`) and the same `403`. It runs after the body is read and before the application reads the
thread or the card, makes an agent ready or writes anything. The envelope goes from the body to the start's outbox
row (`OutboxPayload::SessionStart`) and round 1's `SendRequest`; it is not in the job ledger, and the child's logged
`user_message` has no place for it. As on the run route, nothing reads it off `SendRequest` yet (ANR-903). A later
message **in** a helper's thread is a run, so the run route's check already covers it.

**Two gaps remain:** the REST fork and edit (**ANR-1375**) and a Stop & send's held text (**ANR-1376**).

*Verified 2026-11-28 by reading the code; the new tests have not been built or run.*

*Amended 2026-11-28 (ANR-1376): a Stop & send's held message keeps its signature.* This closes that gap, and changes
two things said above: that the envelope is not stored in the job ledger, and that the outbox row is the only durable
place it has.

- **The ledger may hold it,** for the one message a stop holds: `Job.after_stop_prompt_signature`, beside
  `Job.after_stop`. The held text is in the ledger already, and both are saved by the same commit under the same version
  check, so they survive a restart together. A side table would be a migration and a new store operation for one field,
  and the ledger is JSON with defaults, so there is no migration. It goes, byte for byte, to the next job's delegation
  (the task ends, the job is cancelled before it starts, or the delegation is given up), or to the steer of a stop the
  agent refuses for good. It is cleared with the text, and by Stop.
- **The export takes it out.** `App::export_thread` removes it, so the export document and any later user of a
  `ThreadExport` never has it. It is in no event, frame or step, as before. `Input`, `Command`, `Job`, the outbox
  payload, the fork's first message (`Replacement`) and the testkit's recorded send (`SendCall`) hold it as
  `PromptSignature`, whose `Debug` shows its length, and the port's `SendRequest`, which holds a string, has a
  `Debug` of its own that does the same. **Not every type does:** the application's `Inbound`, `FirstMessage` and
  `StartChildSession` hold a plain string that their derived `Debug` prints, and so do the AG-UI wire types, in the
  message `metadata` it arrives in. None of them may be put in a log line.
- **Nothing signed is joined.** A signature covers one message, and the statement has one `user`. Keeping several
  would need a form the statement does not have, and one signature on a joined text would present text that was not
  signed as covered. So while a message is held, a second one (a Send or a Stop & send) is refused when **either** of
  the two carries a signature: signed then signed, signed then unsigned, unsigned then signed. It is a `409`,
  `code: stop_pending`, and nothing is written. The person sends it again once the held message is no longer waiting. Two unsigned
  messages are joined as before, so a client that is not listed sees no change.
- **A repeat is still a repeat.** Two copies of one request that race are answered as one: before the `409`, the commit
  loop looks up the request's idempotency key.
- **Limits:** a message refused this way is not counted against `messages_per_minute`, which is a count of the messages
  in the log. The limit and the allowance are checked before the transition, so a person at either one gets that answer
  instead. What the application does before the transition has run too, as for any message the core refuses: the card
  is read, and a person's own agent is made ready.

**Every client's envelope is bounded at intake.** A client that is not asked to sign is still never refused, but
what it sends is carried only when it is not empty and at most 16 KiB, the presence check's bound; otherwise it is
dropped, as if absent. Without this a Stop & send would keep a value of almost the whole request body in the job
ledger. An empty string is no envelope for anyone, so it cannot make a message count as signed.

**One gap remains:** the REST fork and edit (**ANR-1375**).

*Unverified: nothing of this change was built or run where it was written; CI is the first build.*
*Added 2026-11-28 (ANR-1375, after the note above; the first of the two remaining gaps is closed):* the REST
fork and edit route is checked. `POST /api/threads/{id}/fork` reads the envelope from the **`promptSignature` of its
body** — the member the body had no room for until now — and refuses a signing client's missing or malformed one with
the same `403 prompt_signature_invalid` (reason `invalid`, the finer one logged) as the AG-UI run, **before anything is
written**: for `after` with `text` (the fork's first message) and for `replace` with `text` (the edit). A fork with no
`text` writes no words and is never checked, and `promptSignature` without `text` is a `400`. The check is **one
function**, `orch_api::check_prompt_signature` (the one the AG-UI run and `startChildSession` already call), and the list
is `ApiConfig.signing_clients`, the same `auth.signingClients` the AG-UI surface is given. The edit's envelope rides `ForkAt::Replace.prompt_signature` → `Replacement::edit` →
`Input::UserMessage` → the delegation's outbox row, as a first message's does, and it is in no event, frame or export.
A Stop & send's held text (ANR-1376) is the one gap still open at this note; *(2026-11-28, later: ANR-1376's own note above closed it — **both gaps the 2026-11-28 note named are now closed**.)*

*Verified 2026-11-28 by reading the code of this change; nothing was compiled or run on this host (owner ANR-Q79), so
CI is the compiler. Unverified: `crates/api/tests/fork_signature.rs`, the core test of an edit's envelope, and the
`ApiConfig.signing_clients` wiring have not been executed.*
