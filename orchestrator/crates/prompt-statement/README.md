# orch-prompt-statement

The prompt statement of the cloud path ([ADR 0087](../../../docs/decisions/0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md),
the note of 2026-11-25): what the orchestrator signs in the place of a device's prompt key, so that the verifier behind the
model gateway can check a person's prompt it never saw. **Pure**: no I/O, no async, no clock (the caller gives `iat`) and no
randomness (the caller gives `jti`). **It never verifies a signature.**

## Where it sits

Depends on `serde_json`, `sha2`, `base64`, `thiserror` and `ryu-js` (ECMAScript's printing of numbers; already in the lock
through `adam-a2a`); on no crate of the workspace. [`orch-ports`](../ports/README.md) re-exports `AttestedPayload`, the one thing
a `PromptSigner` takes; the signer adapters ([`orch-prompt-signer-static`](../prompt-signer-static/README.md)) write the JWS with
its `jws` module.

## API at a glance

| Item | What |
|---|---|
| `Statement { aud, iat, exp, jti, src, sub, azp, thread, agent, a2a, content_sha256, user }` | the members of the contract's payload; times are whole seconds since the Unix epoch, `jti` 16 bytes (`JTI_LEN`), `src` the closed `Source` (`Chat`, `Resume`, `Schedule`, `Webhook`, `Mcp`), `a2a` an `A2aIds { context_id, message_id }`, `user` an `Option<UserClaims>` (`None` is `null`: a web client, a schedule, a webhook) |
| `statement.attest(Option<&Received>)` | **the integrity rule**, then the payload: with a `user`, its `thread`, `agent`, `msg`, `content_sha256` and `props_sha256` must be what the request holds (the hashes recomputed from it), and the request must be given. Returns an `AttestedPayload` (the RFC 8785 bytes with `typ`) or a `StatementError`; nothing may be signed then |
| `Received { thread, agent, msg, content, forwarded_props }` | the person's message as the AG-UI run route received it: `agent` as sent and percent-decoded once, `content` the member exactly as received, `forwarded_props` `None` (or JSON `null`) when the run has none |
| `UserClaims::from_request(aud, &Received, envelope)` | the `user` of a request: every member from it, `aud` from the configuration, `env_sha256` from the envelope's exact bytes. `.check_against(&Received)` is the rule alone |
| `StatementError::{Mismatch(Field), Unbound, Lifetime}` | `Mismatch` and `Unbound` are ADR 0087's refusal: `.is_prompt_signature_invalid()`, `.code()` = `prompt_signature_invalid`, `.client_reason()` = `invalid`, `.log_reason()` = `mismatch` (for the log, never the response). `Lifetime` (`exp` not after `iat`, or more than `MAX_LIFETIME_SECS`, a day, after it) is the orchestrator's own mistake and has no code. No variant holds a value of the request |
| `AttestedPayload` | the bytes, `.as_bytes()`; made only by `attest`; `Debug` shows the length (the payload names the person) |
| `check_presence(Option<&str>)` | the presence check of a signing client's envelope (ADR 0087, decision 1): present, at most `MAX_ENVELOPE_LEN` bytes, three dot-separated base64url parts (the middle one may be empty: a detached payload). Returns the received `&str` itself, never a re-encoded copy, or a `PresenceError` (`Missing`, `Malformed`), refused as `403 prompt_signature_invalid`, reason `invalid`. No cryptography |
| `statement_exp(iat, leaf_not_after)` | `min(iat + 24 h, notAfter)` |
| `canonical(&Value)`, `hash_of(&Value)`, `props_hash(Option<&Value>)`, `sha256_b64u(&[u8])` | RFC 8785 (members by UTF-16 code units, §3.2.2.2 escapes, ECMAScript numbers), and `base64url(SHA-256(...))` without padding; `props_hash` of none is the hash of `{}` |
| `jws::protected_header(&x5c)`, `jws::signing_input(&header, &payload)`, `jws::compact(&input, &signature)` | the statement's compact JWS: header `{"alg":"EdDSA","typ":"prompt-statement/v1","x5c":[...]}` in RFC 8785 form, the certificates' DER in standard base64 |
| `STATEMENT_TYP`, `PERSON_TYP`, `MAX_LIFETIME_SECS`, `JTI_LEN` | the contract's constants; `STATEMENT_TYP` is *proposed* until the shared signature contract fixes it |

## Tests

`cargo test -p orch-prompt-statement`:

* `tests/vectors.rs`: the shared prompt-signature vectors ([`tests/vectors/`](tests/vectors/README.md)): the content of V3a (a
  string) and V3b (two parts with non-ASCII text and an emoji) hashes as they say; absent `forwardedProps` hashes as `{}`;
  V3a's `user` is theirs; **the statement built for V2's input is V2's payload byte for byte**, and its header and signing
  input are the ones V2 was signed over.
* `tests/integrity.rs`: each member of the rule changed once (the content, the content hash in `user`, an option, the
  props hash, the thread, the agent, the message) is refused with its field and ADR 0087's code; a `user` without a request
  is refused; no `user` needs none; a lifetime of zero, negative or over a day is refused without a code; every `src`; the
  payload holds exactly the contract's members; `Debug` does not show the person.
* `tests/jcs.rs`: members sorted by UTF-16 and not by code point, the escapes, nesting, the number samples of RFC 8785
  appendix B and numbers as a request parses them, and the hash form.
* The doctest of `src/lib.rs`: a statement attested, and refused once its content is changed.
