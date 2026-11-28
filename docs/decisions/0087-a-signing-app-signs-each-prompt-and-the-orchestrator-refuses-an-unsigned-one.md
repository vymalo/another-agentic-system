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

*Amended 2026-11-28 (ANR-1422): what a verifier needs is computed at intake and kept with the envelope.* The envelope
alone cannot be checked: the person's payload also names `aud`, `thread`, the agent **as sent**, `msg` and two hashes,
and the original `content` and `forwardedProps` cannot be rebuilt from the text that is kept. This changes names said
above and nothing of the decision, the names in the ANR-1375 note above included: `ForkAt::Replace.prompt_signature` is
now `ForkAt::Replace.signed_prompt`.

- **One value, `SignedPrompt`,** goes from intake through the input, the command, the outbox row, the job ledger of a
  Stop & send and the agent's request: `{ envelope, claims? }`, with
  `claims { aud, thread, agent, msg, content_sha256, props_sha256 }`. A later member is one change. `env_sha256` is not
  kept: it is a function of the envelope's bytes. The types that held a plain string, whose `Debug` the note on
  ANR-1376 above warns about, hold this value, which shows lengths. The AG-UI wire types still print the message
  `metadata` they receive.
- **Where each claim comes from,** on the run route. `aud` is the configuration's `server.publicOrigin` (decision 2),
  never the request. `thread` is the `threadId` as written, and `agent` the path segment, percent-decoded once, before
  an alias is resolved. `msg` is the message id, or for a resume the `interruptId`. The hashes are taken from the body
  **read as plain JSON**: the typed AG-UI view drops members it does not define, inside a content part too, so a hash
  made from it could differ from the app's. `content` is the message's `content`, or an answer's whole `payload`.
- **`server.publicOrigin` is built,** in decision 2's normal form. A file that lists a signing client with the key
  absent or empty **does not start**. `production` refuses `http://` except for the loopback. With no signing client it
  may be absent; an envelope an unlisted client sends is then carried with no claims. The resource API's two signing
  routes, `startChildSession` and `forkThread`, keep claims of their own (the first since 2026-11-28, below; the second
  still open, below).
- **A helper-session start signs these claims (decided 2026-11-28).** `POST /api/threads/{threadId}/sessions` has no
  AG-UI `content`, `forwardedProps` or `agentId` segment, so each claim is taken from what the route does carry: `aud` is
  the configuration's `server.publicOrigin` (decision 2, as on a run); `thread` is the **parent** thread's id, from the
  path (the child context does not exist when the request arrives); `agent` is the body's `agent`, the helper's name
  **exactly as the card lists it** (ADR 0049's aliases name orchestrator agents, not helpers, so they do not apply);
  `msg` is the body's `id`, the request id the caller chooses so that a repeat is recognised; `content` is the body's
  `text`, one JSON string, so `content_sha256` is `base64url(SHA-256(JCS(text)))`; and `props_sha256` is the hash of the
  empty object, `base64url(SHA-256("{}"))`, since the route has no `forwardedProps`. `thread` and `msg` are the ids in
  their **normal form** — lower case, hyphenated — not as the caller wrote them, because both are parsed into a UUID and
  printed again; a client that signs an upper-case id signs a different payload. The claims, for the text
  `List every place that writes the session header.`:

  ```json
  {
    "aud": "https://chat.example.com",
    "thread": "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c33",
    "agent": "planner",
    "msg": "0194a6c2-7c10-7a1e-8f42-1b2c3d4e5f60",
    "content_sha256": "zJA-Pfw1qI_cyfimdG87eRX4_7u91p6bvhHsKwaCM8E",
    "props_sha256": "RBNvo1WzZ4oRRq0W9-hknpT7T8If536DEMBg9hyq_4o"
  }
  ```

  (`zJA-…` is the SHA-256 of `"List every place that writes the session header."`, quotes included, and `RBNv…` of `{}`;
  both were computed outside the orchestrator's code, as the other vectors are. The **signed payload** is not this
  object: it is decision 2's `{typ, aud, thread, agent, msg, content_sha256, props_sha256, nonce, iat}`, canonicalised,
  of which these six members are the part this route supplies.) **This is the contract for what a client signs. The
  route did not build the claims when this was written** — it kept the envelope **alone**
  (`SignedPrompt::envelope_only`, `orchestrator/crates/api/src/routes.rs`, and `ApiConfig` carried no
  `server.publicOrigin`) — so a start was **signed and not verifiable**, and ANR-903 had to refuse one from a signing
  client until **ANR-1441** kept them. *ANR-1441 does now: the note at the end of this section.* The intake behaviour
  of ANR-1383 is unchanged.
- **The same holds for the REST fork and edit** (`POST /api/threads/{id}/fork`, ANR-1375): its body has no AG-UI
  `content`, `forwardedProps` or `agentId` segment, and its `messageId` is optional, so it carries the envelope
  **only**. **It is signed and not yet verifiable, and the verifier (ANR-903) must refuse it from a signing client
  until the contract says what it signs. Open.** A fork made by a run (`vymalo.fork`) keeps claims, as any run does.
- **The stored names are new:** `signed_prompt` on the outbox payload and `afterStopSignedPrompt` on the job, where
  `prompt_signature` and `afterStopPromptSignature` were. There is no migration and no reading of the old members:
  they are ignored. **During a rolling upgrade, a message queued or held across it loses its signature and goes out
  unsigned.** With a verifier on, that message is refused downstream, which fails closed; today nothing shows it.
- **The verifier's own clock, not an intake time (decided 2026-11-28).** It compares the signature's `iat` with **its**
  clock inside decision 4's window — **300 seconds in the past and 60 seconds in the future** — and both bounds are a
  deployment's value, not this contract's. No `received_at` is kept and the statement contract and its vectors do not
  change. (The cloud path checks no `iat` at intake today, presence only; the window is the device path's, and this
  decision extends it to the verifier.) **The consequence belongs to the hold, not to this record:** a Stop & send
  message that outlives the window is refused by the verifier, and neither the held envelope (ANR-1376, merged) nor the
  claims kept at intake (ANR-1422) re-signs anything. Two answers are buildable: the hold is **bounded by the window**
  (Stop & send refuses a message that would outlive it) or the held message is **dropped at delivery** when it is too
  old and the person is told to send it again. A third — the client re-signing at delivery — needs a server-to-client
  flow that does not exist. **ANR-1442** decides which.

*Verified 2026-11-28 by reading `main`'s code: `ChildSessionStart`'s `id` is required (`docs/api/chat-api.yaml`), the
route passes `body.agent` to the card's `agents` as an exact string (`orchestrator/crates/api/src/routes.rs`,
`core/src/child_sessions.rs`), `text` is not trimmed (`crates/app/src/app.rs`), the body has no `forwardedProps`
(`deny_unknown_fields`), and the route kept `SignedPrompt::envelope_only` with no `public_origin` on `ApiConfig`
*(superseded by ANR-1441, the note at the end of this section).*

*Unverified: nothing of this change was built or run where it was written; CI is the first build. The shared vectors
have no resume case; the test of one compares with a hash computed outside the orchestrator's code.*

*Added 2026-11-28 (ANR-1430: the statement's `aud` and the signer are configuration):* the two values the statement
needs are keys of the configuration file, not constants, because both are a deployment's: `promptStatement.aud` is the
**model gateway's origin** the statement is for (written in `server.publicOrigin`'s normal form, and `production` takes
`http://` only for the loopback), and `promptStatement.signer` selects the signer, with `promptStatement.static { key,
chain }` for the only kind built. **An absent section is signing off**: the process starts, and **no statement metadata
goes with an agent's messages**, even to an agent whose card lists `prompt-statement/v1`; a verifier must then refuse
those calls. Nothing sends a statement yet (ANR-1431 is its first caller), so today the absence is observable only as
the port set's signer being `NoPromptSigner`.

- **The static signer is not a deployment's.** It is behind the binary's **off-by-default** Cargo feature
  `prompt-signer-static`; a build without it refuses a file that selects it (exit 78, naming the feature) rather than
  signing nothing, and a `production` file is refused by the file's rules (exit 78, naming the key) whatever the build,
  because the signer never renews or revokes its certificate. The PKI signer is its own change.
- **The bundle carries it, statically.** `PortSet` gained `prompt_signer: P = NoPromptSigner` and `Ports` gained
  `type Signer: PromptSigner` / `prompt_signer()`, like every other port: `PromptSigner::sign` returns an `impl Future`,
  so the trait is **not dyn-compatible** and no `Arc<dyn PromptSigner>` is possible. `NoPromptSigner` is always
  `Unavailable`, so a caller asks `state()` and sends no statement; the binary's `ConfiguredSigner` is `None` or the
  static signer.
- **The key is the twenty-second secret of the schema** (`promptStatement.static.key`, a reference, resolved at startup
  and never printed); the chain is a file path read through the same resolver and must hold a certificate. A key that is
  not a PKCS#8 Ed25519 one, or a leaf that certifies another key, stops startup naming the key and never its text.
- **Not built:** the PKI signer, the statement on the agent's messages, and the verifier's refusal of a call whose agent
  lists the extension while this deployment signs nothing.

*Verified 2026-11-28 on this branch: `cargo test -p orch-config` (the section, its rules, the schema) and
`cargo test -p orchestrator` pass without and with the feature, and the production refusal was seen to fail when its
guard was removed. The missing-feature refusal is a unit test of the binary (`a_static_prompt_signer_without_its_feature_stops_startup`).*

*Added 2026-11-28 (ANR-1441: the helper-session start keeps its claims).* The gap the ANR-1422 note above recorded is
closed: `POST /api/threads/{threadId}/sessions` builds the six members decided there, from the body and the path, and
keeps them beside the envelope as `SignedPrompt { envelope, claims }`, exactly as a run does.

- **`server.publicOrigin` reaches the resource API.** The key already existed (decision 2, and `docs/api/config.md`); only
  the wiring was missing. `orch_api::ApiConfig` gains `public_origin: Option<Arc<str>>`, the binary fills it from
  `cfg.public_origin` as it already does for the AG-UI surface's `Intake`, and `ApiState` carries it to the route. It is
  the `aud` and is **never** taken from the request. With none configured the envelope goes alone, as before; a
  deployment that lists a signing client cannot reach that state (the file refuses to start without the key).
- **Each member, from what the route has:** `aud` is the configured public origin; `thread` is the parent of the path;
  `agent` is the body's `agent`, the helper's exact name; `msg` is the body's `id`; `content` is the body's `text` as one
  JSON string, so `content_sha256` is `base64url(SHA-256(JCS(text)))`; `props_sha256` is the hash of `{}`. `thread` and
  `msg` are parsed into a UUID and printed again, so they are in their **normal form** — an upper-case `id` in the
  request signs the lower-case `msg`. The builder is `orch_api::prompt_claims` over `Received`, the same one the run
  route uses, so the two cannot hash differently.
- **Where they ride:** round 1's outbox row (`OutboxPayload::SessionStart.signed_prompt`) and the dispatcher's
  `SendRequest`, unchanged, as a run's do. No event, frame, step, export or log line holds the envelope or a hash; the
  request id the parent's `child_session_start_asked` records is the same value as `msg`, as it was before, and is not
  the claim.
- **The REST fork and edit** (`forkThread`) still keeps the envelope alone: that part of the ANR-1422 note stands.

*Verified 2026-11-28 by reading `main`'s code and the change; nothing was compiled or run on this host (owner ANR-Q79),
so CI is the compiler. Unverified: the flipped `crates/surface-agui/tests/prompt_signature.rs` and the new
`crates/app/tests/prompt_signature.rs` case have not been executed.*
