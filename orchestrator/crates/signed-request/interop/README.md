# orch-signed-request-interop

Holds [`orch-signed-request`](../README.md) to the real **`cratestack-cose` 0.17.2**, the library the tool server verifies with
([`signed-tool-requests-v1`](../../../../docs/api/signed-tool-requests-v1.md)):

* the real `open_claims` opens what the orchestrator signs, for requests that stretch the encoding (an empty body, 100 000 bytes, non-ASCII
  in the owner, the longest path and owner, a nil and a maximal thread), and returns the `kid`, the algorithm, the four CWT claims and the five
  payload members the contract says; it refuses another `typ`, another `external_aad`, another key and a bit flipped anywhere;
* the real `sign_claims` writes, for the vector of `../tests/vectors/v1.json`, exactly the committed bytes, and `orch-signed-request` writes
  the same ones from the same inputs;
* what the real `sign_claims` writes for a request of its own, the orchestrator's reference verifier accepts.

It is a crate of its own, **outside the orchestrator's workspace**: it keeps its own `Cargo.lock` and pins its own toolchain file
(1.98.1), and CI job `signed-request-interop` runs it. The workspace moved to Rust 1.98 on 2026-11-27 (ANR-1328), so
`orch-signed-request` can now use the real crate and these tests can become ordinary tests of the workspace; that swap is its own change.

```sh
cd orchestrator/crates/signed-request/interop
cargo test --locked    # needs a Rust 1.98 toolchain: rustup reads rust-toolchain.toml
```
