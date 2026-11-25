# Prompt-signature vectors (TEST ONLY)

`prompt-signatures.json` is a copy of part of the **shared prompt-signature test vectors** of the signature contract's
cloud section (its ADR-0033, not this repository's ADR 0033), taken on 2026-11-25: the input and the exact bytes of
**V2** (the orchestrator's statement), the input of **V3a** (a string, no `forwardedProps`) that V2 attests, the content and `forwardedProps` of **V3c**, and the
content of **V3b** (two parts, non-ASCII text). Nothing else of those vectors is here. Every value is a test value; the
origins end in `.test.invalid`.

The key and the chain V2 was signed with are in
[`orch-prompt-signer-static`'s vectors](../../../prompt-signer-static/tests/vectors/README.md).

Do not edit these files: when the shared vectors change, copy them again and say from when.
