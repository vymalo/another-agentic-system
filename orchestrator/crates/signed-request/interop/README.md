# orch-signed-request-interop

Holds [`orch-signed-request`](../README.md) to the real **`cratestack-cose` 0.17.2**, the library the tool server verifies with
([`signed-tool-requests-v1`](../../../../docs/api/signed-tool-requests-v1.md)):

* the real `open_claims` opens what the orchestrator signs, for requests that stretch the encoding (an empty body, 100 000 bytes, non-ASCII
  in the owner, the longest path and owner, a nil and a maximal thread), and returns the `kid`, the algorithm, the four CWT claims and the five
  payload members the contract says; it refuses another `typ`, another `external_aad`, another key and a bit flipped anywhere;
* the real `sign_claims` writes, for the vector of `../tests/vectors/v1.json`, exactly the committed bytes, and `orch-signed-request` writes
  the same ones from the same inputs;
* what the real `sign_claims` writes for a request of its own, the orchestrator's reference verifier accepts.

It is a crate of its own, **outside the orchestrator's workspace**, because `cratestack-cose` 0.17.2 has `rust-version = "1.98.0"` and the
workspace builds on 1.94.1 (`rust-toolchain.toml` here pins 1.98.1). CI job `signed-request-interop` runs it. When the workspace moves to
Rust 1.98 (a change of its own: the Dockerfile, six workflows and the adam-rs pin), `orch-signed-request` can use the real crate and these tests
become ordinary tests of the workspace.

```sh
cd orchestrator/crates/signed-request/interop
cargo test --locked    # needs a Rust 1.98 toolchain: rustup reads rust-toolchain.toml
```
