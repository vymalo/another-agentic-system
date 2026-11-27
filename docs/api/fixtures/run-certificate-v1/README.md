# Run-certificate contract v1 — wire fixtures

The bodies of `POST /internal/runs/{run}/certificate`, the route the credential broker serves on its internal
listener ([ADR 0101](../../../decisions/0101-a-cloud-runs-model-calls-are-signed-for-the-run-and-the-injector-stays-for-custom-cases.md),
amended 2026-11-27). Both sides read these files: adam's run-certificate issuer tests and the broker's route tests,
so a shape cannot drift between them.

The route is internal: no edge route, and the caller presents a projected service-account token for the broker's
audience, once per run.

## The request — `request.v1.json`

```json
{"v": 1, "csr": "<base64 DER of a PKCS#10 CSR>"}
```

- `v` is the contract's version, in the **body**. It is `1`; a later shape adds a version rather than a second route.
- `csr` is the DER of a PKCS#10 certification request, base64 (standard alphabet, padded), whose key is **Ed25519**.
  adam makes the key and the CSR **inside the run**; the private key never leaves it.
- The broker ignores the CSR's subject and extensions and builds the request itself.

## The answer — `answer.v1.json`

```json
{"v": 1, "chain": ["<base64 DER>", "<base64 DER>", …]}
```

- `chain` is the leaf **first**, then its intermediates; the root is omitted, as a chain that ends in a configured
  anchor does. At most 8 certificates.
- There is no `kid` and no `notAfter`: both are read from the leaf, which the caller now has. This is what supersedes
  section 3's `{chain, kid, notAfter}`.
- The leaf's URI SAN names the run, its extended key usage is client authentication, and it signs **model requests
  only** — run history keeps adam's process turn key (ADR 0068).

## The refusals

Every refusal is `application/problem+json` with a `code` member, as everywhere else in this API.

| File | Status | `code` | When |
|---|---|---|---|
| `error-unauthenticated.v1.json` | 401 | `unauthenticated` | no acceptable service-account token |
| `error-run-not-bound.v1.json` | 403 | `run_not_bound` | no live binding names the run, or the caller is not its agent (the answer never says whether the run exists) |
| `error-lease-expired.v1.json` | 403 | `lease_expired` | the binding exists and its lease is over |
| `error-not-signed.v1.json` | 403 | `not_signed` | the binding's path is `injected` |
| `error-bad-csr.v1.json` | 400 | `bad_csr` | the CSR does not parse, or its key is not Ed25519 |
| `error-unavailable.v1.json` | 503 | `unavailable` | the issuing port did not answer; the caller retries with the same CSR |

The statuses are the amendment's proposal: a slice that disagrees changes them **and** these files in one change.

## The certificates in `answer.v1.json`

They are a real chain, made once with `openssl` and committed for their shape: a root (not in the file), an
intermediate, and a leaf whose CSR is the one in `request.v1.json` — so a test can verify that the answer's leaf
belongs to the request's key. Ed25519 throughout, the leaf's EKU is client authentication, and its URI SAN is
`spiffe://another/ns/agents/sa/run-1`. They expire a day after they were made: a test that needs a valid leaf mints
its own, and reads these files for the **shape**, not for the validity.
