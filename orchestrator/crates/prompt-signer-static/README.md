# orch-prompt-signer-static

A `PromptSigner` for tests ([ADR 0087](../../../docs/decisions/0087-a-signing-app-signs-each-prompt-and-the-orchestrator-refuses-an-unsigned-one.md),
the note of 2026-11-25): an Ed25519 key and the certificate chain of its leaf, read from PEM files, never renewed. It writes
the statement as every signer must, so the orchestrator's tests can sign with a self-made chain. **Not for a deployment**: its
key sits in a file and its certificate is never renewed or revoked; a deployment's signer is certified by a PKI.

## Where it sits

Implements the `PromptSigner` port of [`orch-ports`](../ports/README.md) with the JWS of
[`orch-prompt-statement`](../prompt-statement/README.md); Ed25519 is `aws-lc-rs`, already in the tree. No composition root
uses it yet.

## API at a glance

| Item | What |
|---|---|
| `StaticPromptSigner::from_pem(key_pem, chain_pem)` | a PKCS#8 Ed25519 key (`-----BEGIN PRIVATE KEY-----`, exactly one block) and a chain of `CERTIFICATE` blocks, the leaf first and the root left out; text outside the blocks is ignored. Refused, with a `StaticSignerError` that quotes nothing: not one key, not base64, not Ed25519, no certificate, a leaf with no Ed25519 key, a leaf that certifies another key |
| `StaticPromptSigner::from_files(&key, &chain)` | the same from two files; `Unreadable("key" \| "chain")` names no path |
| `sign(&AttestedPayload)`, `state()` | the port: the compact JWS with `x5c` and the payload attached; `Ready`, or `Unavailable` (then `sign` is `SignerError::Unavailable`) |
| `set_unavailable(bool)` | the stand-in of a certificate that expired and was not renewed |
| `signatures()` | how many statements it signed: for a test that asserts nothing was |
| `leaf_serial()` | the leaf's serial in hex; `Debug` shows it and nothing of the key |

It does not judge the chain (a root, a SAN, a validity, a CRL): that is the verifier's.

## Tests

`cargo test -p orch-prompt-signer-static` (the fixtures are in [`tests/vectors/`](tests/vectors/README.md)):

* `tests/conformance.rs`: `orch_ports::prompt_signer_conformance!`, with `set_unavailable(true)` as its `unavailable` hook.
* `tests/vector_v2.rs`: **V2 byte for byte**: the statement for V2's input, signed with V1's leaf key and chain, is V2's JWS.
* `tests/integrity.rs`: attest then sign, as the run route will: the request the person signed for is signed once; a changed
  content, content hash or props hash is refused with ADR 0087's code and `signatures()` stays 0.
* `tests/kit_catches.rs`: six signers wrapping this one, each breaking one rule (a detached payload, another `typ`, a flipped
  signature bit, the first statement replayed, `Unavailable` while it signs, signing while unavailable), each failing the
  testkit's case for that rule; the wrapper with nothing broken passes every case.
* `tests/load.rs`: the files load as the text does; each refusal; no path, key or key text in an error or `Debug`.
* Unit tests in `src/der.rs`: the serial after a version, and cut input.
