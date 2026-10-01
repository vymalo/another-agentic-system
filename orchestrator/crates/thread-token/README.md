# orch-thread-token

The token of the thread-tools endpoint ([`thread-tools/v1`](../../../docs/api/thread-tools-v1.md)): a JWS in compact
form, signed HS256 (HMAC-SHA-256) with a key from the deployment's configuration, minted by the A2A adapter when it
sends a message and verified by any replica of the orchestrator that serves the endpoint. **Pure**: no I/O, no async, no
clock (every function that needs "now" takes it), so the tests pin the exact bytes of a token.

## Where it sits

Depends on [`orch-core`](../core/README.md) (the ids, and `Caller` and `ToolsGrant`, the non-secret part that travels
inside the orchestrator), `hmac` and `sha2` (RustCrypto), `base64`, `jiff`, `secrecy`, `serde` and `url`. Used by
[`orch-surface-thread-tools`](../surface-thread-tools/README.md) (it verifies) and by
[`orch-agent-a2a`](../agent-a2a/README.md) (it mints, when it attaches the grant to a message); the binary builds the
issuer from `THREAD_TOOLS_*` and gives it to both.
The token never enters the event log, the outbox or a log line: `ThreadToolsGrant` prints `[redacted]` for it, the keys
print only their `kid`.

## API at a glance

| Item | What |
|---|---|
| `ThreadToolsKeys::new(current, previous?)` | the keys, each a `SecretString` of at least `MIN_KEY_BYTES` (32) bytes; the previous one is for **verifying** only (a rotation) and may not be the current one. `KeyError` says which. `.current_kid()`, `.previous_kid()`; `Debug` shows `kid`s, never a key |
| `Claims { thread, job, agent, caller, depth, message_id, issued_at, expires_at }` | what a token says (the wire names are `sub`, `job`, `agt`, `caller`, `depth`, `jti`, `iat`, `exp`, plus `iss` = `orch` and `aud` = `thread-tools`). `.is_well_formed()`: a job from 1, `depth` 0 exactly for `main`, an `ask:<n>` from 1 at depth 1 or more, non-empty `agt` and `jti` of at most 256 bytes, whole seconds, `exp` not before `iat` |
| `mint(&keys, &claims) -> Result<SecretString, MintError>` | writes the token with the current key |
| `verify(&keys, token, now) -> Result<Claims, TokenError>` | the checks that need only the keys and the time, in the contract's order: at most `MAX_TOKEN_BYTES` (2048) and three segments of strict base64url; the header exactly `{alg: HS256, typ: JWT, kid}` (`none` and every other algorithm refused) and a `kid` that is the current or the previous key; the signature, in constant time (`Mac::verify_slice`); the claims, every one present and well formed, no other; `iss`, `aud`; `exp` later than now minus `SKEW_SECS` (30) and `iat` not later than now plus 30. `TokenError` names the first that fails; the endpoint answers all of them with one `401` and says nothing of which |
| `ThreadToolsIssuer::new(keys, base_url, ttl)` | the keys, the address agents reach the orchestrator at (`http` or `https`, a host, optionally a port and a path prefix; no credentials, query or fragment) and the lifetime of a token (whole seconds from `MIN_TTL_SECS` 60 to `MAX_TTL_SECS` 86 400; `DEFAULT_TTL_SECS` is 7200). `IssuerError` for an unusable URL or lifetime |
| `issuer.grant(&ToolsGrant, message_id, now) -> Result<ThreadToolsGrant, MintError>` | `{url, token, expires_at}`: the endpoint of the grant's thread (`<base>/thread-tools/<threadId>/mcp`), the token, and its `exp`. A grant whose fields disagree (`ToolsGrant::is_consistent`) is never minted |
| `issuer.url_for(thread)`, `.base_url()`, `.host()`, `.keys()`, `.ttl()` | the endpoint of a thread; the base URL without a trailing slash; the `Host` header value of the base URL (the name, and the port when the URL names one) |

## The token

```text
header  {"alg":"HS256","typ":"JWT","kid":"<first 16 hex of SHA-256(key)>"}
claims  {"iss":"orch","aud":"thread-tools","sub":"<thread>","job":<n>,"agt":"<agent>",
         "caller":"main"|"ask:<n>","depth":<0..255>,"jti":"<A2A message id>","iat":<s>,"exp":<s>}
token   base64url(header) "." base64url(claims) "." base64url(HMAC-SHA-256(key, first two segments))
```

Base64url is the strict unpadded alphabet: a padded token, another alphabet or non-canonical trailing bits is refused,
so no single-character change of a token can be accepted (a property test checks every position and character). The key
is the bytes of the configured string as written.

## Tests

`cargo test -p orch-thread-token`:

* `tests/vectors.rs`: two **known-answer vectors**, a fixed key, claims and time and the exact token they give, computed
  with an implementation that shares no code with this one (Python `hmac`, `hashlib`, `base64`, 2026-10-01) and written
  in [`docs/api/thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md#known-answer-vectors); the `kid`; the
  boundaries of the lifetime (valid from 30 s before `iat` to 29 s after `exp`); every header a forger who holds the key
  could write (`none`, `HS512`, `typ` variants, extra members, a missing `kid`) is refused by the header alone; claims
  that disagree are refused even when signed; the key and issuer checks; `Debug` never shows a key or a token.
* `tests/properties.rs` (proptest): what is minted reads back; no one-character change, deletion or insertion is
  accepted; another key never reads a token; the lifetime is the claims' plus the skew; garbage is refused without a
  panic.
