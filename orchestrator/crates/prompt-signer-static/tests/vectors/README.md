# Signer vectors (TEST ONLY)

A copy, taken on 2026-11-25, of part of the **shared prompt-signature test vectors** of the signature contract's cloud
section (its ADR-0033, not this repository's ADR 0033), vector **V1**:

| File | What |
|---|---|
| `orchestrator.test-only.key.pem` | the orchestrator leaf's Ed25519 key, from a fixed seed: public and test-only |
| `chain.test-only.pem` | the orchestrator's leaf (serial `1001`) and the intermediate that issued it, as `x5c` carries them; the root is left out |

With them this adapter signs V2's input into V2's exact bytes (`tests/vector_v2.rs`; the input is in
[`orch-prompt-statement`'s vectors](../../../prompt-statement/tests/vectors/README.md)). **Never use this key outside tests.**
Do not edit these files: when the shared vectors change, copy them again and say from when.
