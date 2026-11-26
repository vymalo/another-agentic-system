# ADR 0102 — CBOR is a second media type of the chat API's JSON bodies

- **Status:** **Accepted (2026-11-26)** by the owner: *"Yes, I'm OK with CBOR. The question was mostly for the -system and -platform, which will use OpenAPI to communicate and not CrateStack."* Built in `orchestrator/crates/api/src/cbor.rs`.

## Context

Every body of the chat API is JSON, described by `docs/api/chat-api.yaml`. A client that wants a smaller, faster-to-parse
encoding has none. The owner chose CBOR, not protobuf, and asked that the API keep its axum routes and its OpenAPI document.

## Decision

1. **A second media type, `application/cbor`, on the same schemas.** No route, type or schema is added or changed.
   JSON stays the default for a client that says nothing.
2. **Why not protobuf.** Protobuf needs a second schema language and generated code beside OpenAPI, and the schemas here are
   JSON Schema. CBOR is the same data model as JSON, so the one OpenAPI schema describes both and serde is the only interface.
3. **Negotiated in one place**, a middleware over the resource API (`cbor::negotiate`), not in each handler:
   - `Content-Type: application/cbor` on a request: the body is checked, decoded by `ciborium` `=0.2.2` into the JSON value
     the handler already reads, and handed on as `application/json`.
   - `Accept: application/cbor` (with `q` values; a tie goes to JSON) encodes an `application/json` answer as CBOR. `Accept`
     absent, `*/*`, `application/*` or `application/json` gives JSON. An `Accept` that lists neither (and not
     `text/event-stream`) is **406** `not_acceptable`, before any handler runs. A body that names another media type on a
     route that reads JSON is **415** `unsupported_media_type`; a body with no media type stays 400.
   - `Vary: Accept` is on every answer the layer converts or could have converted.
4. **Limits.** The request limit is the JSON one, 1 MiB. Before decoding, a walker refuses an **indefinite-length item**, a
   reserved header, a length that runs past the input, nesting beyond **64** and a second item after the first; decoding
   then runs under the same recursion limit. Each is a 400 `bad_cbor`, never a panic.
5. **Same values.** Fields JSON carries as base64 stay base64 **text** in CBOR; a byte string, a non-text map key and an
   integer beyond 64 bits are 400. A map that repeats a key keeps the last one, as serde_json does (a route that checks for a
   repeated member in the raw JSON sees the already converted body).
6. **Errors stay `application/problem+json`** whatever `Accept` says; they are small and the envelope is one shape.
7. **SSE and the AG-UI frames stay JSON/event-stream**: the layer converts only `application/json` answers.
8. **The contract lists both.** Every `content` map with `application/json` lists `application/cbor` with the same schema
   (`tests/contract.rs`, `every_json_body_is_also_offered_as_cbor`).

## Consequences

- The conversion reads the body before the route's own checks: the identity layer is outside it, but a route that checks a
  permission or a rate limit before reading its body (search) now decodes a CBOR body first. The cost is bounded by the 1 MiB limit.
- Large answers are decoded to a value and re-encoded (at most 64 MiB), once, in memory.
- The platform's API takes the same rules (sibling card), so a client treats both the same.

## Verification

`tests/edge.rs`: the same request as JSON and CBOR answers equal values on a list, a read and a write; a CBOR write makes the
same change; 400, 406 and 415 are the documented errors. `cbor.rs` unit tests: `Accept` parsing and the refused items.
The web's generated types (`openapi-typescript`) with two media types per body are *unverified* here (no `node_modules`).
