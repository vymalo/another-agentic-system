# orch-signed-request

The signed requests to a tool server ([`signed-tool-requests-v1`](../../../docs/api/signed-tool-requests-v1.md),
[ADR 0080](../../../docs/decisions/0080-requests-to-a-tool-server-are-signed-for-the-person.md)): a `Signer` turns one HTTP request
to a tool server into the value of the header `Tool-Request-Claims`, a tagged `COSE_Sign1` (Ed25519) in the *claim object* profile of
`cratestack-cose` 0.17.2, which `cratestack_cose::open_claims` verifies. The protected header carries the CWT claims `aud`, `exp`, `iat` and
`cti`; the payload binds the request (its method, its path, the SHA-256 of its exact body bytes, the thread and the thread's owner).
**Pure**: no I/O and no clock (every function that needs the time takes it), so the tests pin the exact bytes. Signing and verifying are
`async` because `cratestack-cose`'s functions are; here they wait for nothing and need no particular runtime.

## Where it sits

Depends on `cratestack-cose` (the claim object: `sign_claims`, `open_claims`, the thumbprint; it brings `cratestack-core`), `aws-lc-rs` (the
key: the PEM and the Ed25519 signature; already in the tree as rustls' provider), `sha2`, `base64`, `getrandom`, `serde_json`, `uuid` and
`zeroize`; on no other crate of the workspace. Used by [`orch-tools-mcp`](../tools-mcp/README.md), which signs each request of a session
with it; the binary reads a signing key with it at startup (feature `tool-relay`) so that a key that is not one stops the process.

The claim object is written by `cratestack_cose::sign_claims` and, in the reference verifier, opened by `open_claims`; the library gets
the key through `ExternalSigner::ed25519`, so the private key stays in `aws-lc-rs` (and the library checks each signature against the
public key before it uses it). This crate owns the payload, the limits of the contract and the checks around the two calls. The bytes are
pinned by a known-answer test, so a version of the library that writes others fails it. Until 2026-11-28 (ANR-1342) the crate had a writer
of its own ([ADR 0080, amendment](../../../docs/decisions/0080-requests-to-a-tool-server-are-signed-for-the-person.md#amendment-2026-11-28-anr-1342)).

## API at a glance

| Item | What |
|---|---|
| `SigningKey::from_pkcs8_pem(&str)`, `SigningKey::from_seed(&[u8; 32])` | an Ed25519 key. The PEM is a PKCS#8 `PRIVATE KEY` block, version 1 or 2, nothing around it (`openssl genpkey -algorithm ed25519`). `KeyError` says which step failed and never shows a byte of the text. `.kid()` (8 bytes: the first 8 of the RFC 9679 thumbprint), `.public_key()`; `Debug` shows the `kid` only |
| `Signer::new(key, audience, ttl_secs)` | one key, the audience the tool server calls itself (1 to 256 bytes of one line) and the lifetime of an object (1 to `MAX_TTL_SECS` = 60 seconds; `DEFAULT_TTL_SECS` = 30). `SignError` for a bound that is not met |
| `signer.sign(&Request, iat).await` | the header value for one request at the time `iat` (whole seconds since the Unix epoch), with a fresh random 16-byte `cti`. `Request { method: Method, path, body, thread: Uuid, owner }`; `Method` is the closed `Post` \| `Get` \| `Delete`. `SignError` for a path that is not an ASCII path starting with `/` of at most 512 bytes, an owner that is not 1 to 320 bytes of one line, or an object over `MAX_HEADER_BYTES` (4096); nothing is signed then. `SignError::Cose` if `sign_claims` or the signature fails |
| `signer.sign_with_cti(&Request, iat, cti).await` | the same with the `cti` given: for the known-answer test. A `cti` used twice is a replay |
| `Signer::check(&Request)` | whether a request is within the limits, without signing it |
| `HEADER`, `TYP`, `MAX_TTL_SECS`, `DEFAULT_TTL_SECS`, `MAX_HEADER_BYTES`, `MAX_AUDIENCE_BYTES`, `MAX_PATH_BYTES`, `MAX_OWNER_BYTES`, `CTI_LEN`, `KID_LEN`, `kid_of(&public_key)` | the constants of the contract |
| `verify::Verifier::new(audience, skew_secs).pin(public_key)` (feature `verify`) | **the receiving side**, checks 1 to 5 of the contract: the header, check 2 by `open_claims` itself (the strict object, the `typ`, a pinned key found by `kid`, the signature; the pinned keys are Ed25519, so an object that names another algorithm does not verify), the audience, the time, and the request (method, path, SHA-256 of the body bytes in constant time). At most two keys are pinned; `pin` refuses bytes that are not an Ed25519 key (`VerifyError::InvalidKey`). `.verify(&[header values], &Incoming, now).await` returns `Verified { kid, cti, iat, exp, keep_until, thread, owner }` or the first `VerifyError`, whose `.check()` is the number of the contract's check. **It leaves to the caller what needs state or policy:** check 6 (record `(kid, cti)` until `keep_until`, refuse a second sight) and check 7 (authorise the owner). It is the contract's reference for the orchestrator's tests and for a Rust tool server |

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
* `tests/cratestack.rs`: `cratestack-cose` used directly, as a tool server would. The real `open_claims`, with the library's own resolver,
  opens what the orchestrator signs for requests that stretch the encoding (an empty body, 100 000 bytes, non-ASCII in the owner, the longest
  path and owner, a nil and a maximal thread) and returns the `kid`, the algorithm, the four claims and the five members in order; another
  `typ` and another `external_aad` are refused; the library's own `Ed25519Signer` writes the bytes of the vector; the reference verifier
  accepts what that signer signs and answers `WrongType` for another `typ`.

```sh
cargo test -p orch-signed-request
```
