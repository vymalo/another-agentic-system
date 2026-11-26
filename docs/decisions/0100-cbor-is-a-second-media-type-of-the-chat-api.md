# ADR 0100 — CBOR is a second media type of every JSON body of the chat API

- **Status:** **Accepted (2026-11-26).** The owner on 2026-11-26: *"Yes, I'm OK with CBOR. The question was mostly for the
  -system and -platform, which will use OpenAPI to communicate and not CrateStack."* Built in the orchestrator's `orch-api`
  crate (`orchestrator/crates/api/src/cbor.rs`). The platform's Platform API follows the same rules (its own card).

## Context

A client that cares about size or parsing cost (a terminal client, a native app, a program) wants a binary encoding of the
same API. The API is axum plus one OpenAPI document, `docs/api/chat-api.yaml`, and stays so.

## Decision

1. **CBOR, not protobuf.** CBOR (RFC 8949) is a binary form of the JSON data model, so the schemas, the generated clients and
   the problem envelope stay what they are: one schema, two encodings. Protobuf would be a second schema to keep equal.
   `ciborium` `=0.2.2` (with `ciborium-ll`) reads and writes it through serde; no encoder is written by hand.
2. **A second media type, `application/cbor`, on the same schemas.** JSON is the default for a client that says nothing.
3. **Negotiation is one layer** (`cbor::negotiate`), outside the identity layer, not code in a handler:
   - a request with `Content-Type: application/cbor` is decoded into the value the JSON path reads and handed on as JSON, so
     every handler, every validation and every event in the log are the ones JSON makes;
   - an answer whose body is `application/json` is encoded as CBOR when `Accept` ranks `application/cbor` above JSON (by `q`,
     an exact type beating `application/*` and `*/*`; JSON wins a tie); `application/problem+json` becomes
     `application/problem+cbor`;
   - `Accept` absent, `*/*` or `application/json` gives JSON; one that lists neither is **406** (a write is refused before it
     runs; a read is judged on its answer, because only an answer that is JSON is subject to it);
   - `Vary: Accept` is on every answer that is JSON or CBOR;
   - an answer that is not JSON (the SSE streams, files, metrics) is never touched, and a request body of another type is left
     to the route, which answers 415 where it cares.
4. **Limits.** The body limit is the JSON one (1 MiB, then 413). CBOR nests at most 64 levels. An indefinite-length string,
   array or map, a map key that is not text and a body that does not decode are **400** with the problem envelope.
5. **Base64 stays text.** A field JSON carries as base64 is a CBOR text string with the same characters; no field becomes a
   CBOR byte string, so both encodings hold equal values. CBOR byte strings in a request are refused (400).
6. **SSE and AG-UI stay JSON.**
7. **The document lists both.** Every request body and answer that lists `application/json` (or `application/problem+json`)
   lists the CBOR twin on the same schema; `tools/api-breaking-check` fails a document where one is missing or the schemas
   differ.

## Consequences

- An `ETag` on a CBOR answer is dropped (it would name the JSON bytes); no route's contract uses one on a JSON body.
- A CBOR client gets the JSON number model (integers and floats as serde reads them), not CBOR tags.
- Equal values are proven by `orchestrator/crates/api/tests/cbor.rs`: the same request in both encodings answers equal values.
