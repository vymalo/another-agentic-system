# orch-signed-request

The signed requests to a tool server ([`signed-tool-requests-v1`](../../../docs/api/signed-tool-requests-v1.md),
[ADR 0067](../../../docs/decisions/0067-requests-to-a-tool-server-are-signed-for-the-person.md)): a `Signer` turns one HTTP request
to a tool server into the value of the header `Tool-Request-Claims`, a tagged `COSE_Sign1` (Ed25519) in the *claim object* profile of
`cratestack-cose` 0.17.2, which `cratestack_cose::open_claims` verifies. The protected header carries the CWT claims `aud`, `exp`, `iat` and
`cti`; the payload binds the request (its method, its path, the SHA-256 of its exact body bytes, the thread and the thread's owner).
**Pure**: no I/O, no async, and no clock (every function that needs the time takes it), so the tests pin the exact bytes.

## Where it sits

Depends on `aws-lc-rs` (Ed25519; already in the tree as rustls' provider), `sha2`, `base64`, `getrandom`, `serde_json`, `uuid` and
`zeroize`; on no other crate of the workspace. Used by [`orch-tools-mcp`](../tools-mcp/README.md), which signs each request of a session
with it; the binary reads a signing key with it at startup (feature `tool-relay`) so that a key that is not one stops the process.

It does **not** use `cratestack-cose`, which needs Rust 1.98 where the workspace is on 1.94.1. The bytes it writes are held to that
crate's by a known-answer test (the vector was made by the real `sign_claims`) and by [`interop/`](interop/README.md), a crate with a
toolchain of its own that runs the real `sign_claims` and `open_claims` on them.

## API at a glance

| Item | What |
|---|---|
| `SigningKey::from_pkcs8_pem(&str)`, `SigningKey::from_seed(&[u8; 32])` | an Ed25519 key. The PEM is a PKCS#8 `PRIVATE KEY` block, version 1 or 2, nothing around it (`openssl genpkey -algorithm ed25519`). `KeyError` says which step failed and never shows a byte of the text. `.kid()` (8 bytes: the first 8 of the RFC 9679 thumbprint), `.public_key()`; `Debug` shows the `kid` only |
| `Signer::new(key, audience, ttl_secs)` | one key, the audience the tool server calls itself (1 to 256 bytes of one line) and the lifetime of an object (1 to `MAX_TTL_SECS` = 60 seconds; `DEFAULT_TTL_SECS` = 30). `SignError` for a bound that is not met |
| `signer.sign(&Request, iat)` | the header value for one request at the time `iat` (whole seconds since the Unix epoch), with a fresh random 16-byte `cti`. `Request { method: Method, path, body, thread: Uuid, owner }`; `Method` is the closed `Post` \| `Get` \| `Delete`. `SignError` for a path that is not an ASCII path starting with `/` of at most 512 bytes, an owner that is not 1 to 320 bytes of one line, or an object over `MAX_HEADER_BYTES` (4096); nothing is signed then |
| `signer.sign_with_cti(&Request, iat, cti)` | the same with the `cti` given: for the known-answer test. A `cti` used twice is a replay |
| `Signer::check(&Request)` | whether a request is within the limits, without signing it |
| `HEADER`, `TYP`, `MAX_TTL_SECS`, `DEFAULT_TTL_SECS`, `MAX_HEADER_BYTES`, `MAX_AUDIENCE_BYTES`, `MAX_PATH_BYTES`, `MAX_OWNER_BYTES`, `CTI_LEN`, `KID_LEN`, `kid_of(&public_key)` | the constants of the contract |
| `verify::Verifier::new(audience, skew_secs).pin(public_key)` (feature `verify`) | **the receiving side**, checks 1 to 5 of the contract: the header, the strict object (the profile of `open_claims`: shortest heads, entries in order, an empty unprotected header, no trailing bytes, `alg` -19), the `typ`, a pinned key found by `kid` and the Ed25519 signature, the audience, the time, and the request (method, path, SHA-256 of the body bytes in constant time). At most two keys are pinned. `.verify(&[header values], &Incoming, now)` returns `Verified { kid, cti, iat, exp, keep_until, thread, owner }` or the first `VerifyError`, whose `.check()` is the number of the contract's check. **It leaves to the caller what needs state or policy:** check 6 (record `(kid, cti)` until `keep_until`, refuse a second sight) and check 7 (authorise the owner). It is the contract's reference for the orchestrator's tests and for a Rust tool server without cratestack, **not** a replacement for `open_claims` in vaam's endpoint |

## Tests

`cargo test -p orch-signed-request` (the tests run the crate with its feature `verify`):

* `tests/vectors.rs`: **the worked example, byte for byte** (`tests/vectors/v1.json`: a throwaway seed, a request and the 379 bytes the real
  `cratestack-cose` 0.17.2 `sign_claims` wrote for them on 2026-11-02, and the same bytes in
  [the contract page](../../../docs/api/signed-tool-requests-v1.md#worked-example); a test fails when the page and the file disagree);
  the receiving side accepting it, with the skew on both sides of the lifetime; **each check failing in its own place** (a missing, repeated,
  over-long or not-base64url header, a query, a `Content-Encoding`; another key, no key, a flipped bit in the signature, the payload and the
  protected header, trailing bytes, another tag, a cut object, a non-minimal head, a non-empty unprotected header, another algorithm;
  another audience; a time before or after the lifetime and the skew; another method, path or body); the thread and the owner bound by
  the signature (change one in the bytes and the object does not verify, and a thread or owner signed by the real key arrives as what it
  says); a lifetime over 60 seconds or an `exp` before `iat` refused, and 60 accepted; a `GET` and a `DELETE` binding the digest of nothing; a
  `cti` of its own for each of 64 headers; every limit of the signer (lifetime, audience, path, owner, the longest inputs still fitting a header);
  a PKCS#8 PEM read (with CRLF and white space around it) and anything else refused without quoting it; at most two pinned keys, and either
  verifying while both are pinned (a rotation).
* The doctest of `src/lib.rs`: sign and verify a request.
* [`interop/`](interop/README.md): the real `cratestack-cose`, in CI job `signed-request-interop`.

```sh
cargo test -p orch-signed-request
```
