# orch-agent-a2a

`AgentClient` over the A2A protocol (`a2a-client-lf`): live card and
release-channels discovery, streaming delegation, resubscribe, polling and
cancel.

## Where it sits

An **adapter** of the `AgentClient` port in [`orch-ports`](../ports/README.md).
An agent is an A2A agent-card URL and nothing else: no agent host, gateway or
host SDK is a dependency
([ADR 0007](../../../docs/decisions/0007-protocol-only-dependencies.md)).
Host conveniences are an optional extension, read live from the card, never
cached and failing closed
([ADR 0008](../../../docs/decisions/0008-platform-integration-via-a2a-extension.md)).
The release-channels extension itself is specified in the
`vymalo/another-agentic-platform` repository
(`docs/extensions/release-channels-v1.md` there).
Only the binary ([`orchestrator`](../../bin/orchestrator/README.md)) depends on it.
The pure mapping from A2A values to envelopes and idempotency keys lives in
[`orch-a2a-mapping`](../a2a-mapping/README.md); this crate is the client around it
(HTTP, streaming, resubscribe, errors, release channels, the thread-tools grant). It depends on
[`orch-thread-token`](../thread-token/README.md) to mint the token; nothing else of the orchestrator's surfaces.

## API at a glance

| Item | What |
|---|---|
| `A2aAgentClient::new(A2aConfig) -> Result<_, BuildError>` | implements `AgentClient`; installs the `rustls` crypto provider if none is installed |
| `A2aConfig` | `card_timeout` (5 s), `connect_timeout` (10 s), `read_timeout` (90 s), `call_timeout`, `use_system_proxy`, `thread_tools` (the issuer of the thread-tools grants, `None` by default) |
| `releases_from_card(&AgentCard) -> Option<Releases>`, `RELEASE_CHANNELS_URI` | reads the optional release-channels extension from the live card (the URI is defined in [`orch-a2a-mapping`](../a2a-mapping/README.md) and re-exported) |
| `ui_from_card(&AgentCard) -> Option<UiSupport>` | reads the optional A2UI extension ([ADR 0013](../../../docs/decisions/0013-a2ui-generative-ui.md)) from the live card: an `extensions` entry whose `uri` is exactly `https://a2ui.org/a2a-extension/a2ui/v0.9.1` or `…/v1.0` (both detected, open question 22; `v0.9.1` preferred when both are listed). Anything else is not A2UI |
| `extensions_from_card(&AgentCard) -> BTreeSet<KnownExtension>` | which extensions of the orchestrator's own (`ui-catalog/v1`, `thread-tools/v1`, `steps/v1`, `mentions/v1`: `orch_core::KnownExtension`) the live card lists, by exact URI (a near miss is not listed); `read_card` returns them in `AgentCardInfo.extensions` |
| `A2aConfig.thread_tools: Option<Arc<ThreadToolsIssuer>>`, `thread_tools_metadata(&ThreadToolsGrant) -> Value` | what mints the `thread-tools/v1` grants (the keys, the URL agents reach the orchestrator at and the token lifetime, from [`orch-thread-token`](../thread-token/README.md); `None` by default, so no agent is given a grant) and the value of the extension's key in the message metadata, `{url, token, expiresAt}` |
| `steps_from_card(&AgentCard) -> bool` | whether the live card lists `steps/v1` by exact URI ([ADR 0025](../../../docs/decisions/0025-nested-steps-events-carry-their-source-path.md), [`steps-v1.md`](../../../docs/api/steps-v1.md)) |
| `client_capabilities(UiVersion, Option<&UiDelivery>, accepts_inline)`, `inline_catalog`, `ui_catalog_metadata(&UiDelivery, inline)`, `action_part(&UiActionData, Timestamp)` | the renderer capabilities a message carries (`a2uiClientCapabilities` `{"v0.9.1": {supportedCatalogIds, inlineCatalogs?}}`, or `a2uiRendererCapabilities` `{"v1.0": …}`; with a UI catalog the screen's own id is listed **first** in `supportedCatalogIds` and the catalog is in `inlineCatalogs` only when `inline_catalog` says so), the `ui-catalog/v1` metadata (`{catalogId, version, digest, inline}`), and the `application/a2ui+json` data part that carries a user's action back (`[{"version", "action": {name, surfaceId, sourceComponentId, timestamp, context}}]`) |
| `install_crypto_provider()` | idempotent `rustls` provider setup |

`ui_from_card` also reads whether the entry of the version spoken says `acceptsInlineCatalogs: true` (`UiSupport.accepts_inline_catalogs`; only a boolean `true` counts).

**Steps** ([ADR 0025](../../../docs/decisions/0025-nested-steps-events-carry-their-source-path.md)): an agent whose live card lists `steps/v1` is asked for steps: the URI is activated in `A2A-Extensions` **and** in `message.extensions` on `SendStreamingMessage`, and in `A2A-Extensions` on `SubscribeToTask` (a resubscribe reads the card too, which it did not before). A card without the exact URI is asked for nothing, and the card is read for every call, never remembered. What the agent reports is read by [`orch-a2a-mapping`](../a2a-mapping/README.md) as data whether or not the request asked. `tests/steps.rs` runs the activation (the header, the message's `extensions`, no activation for a card without it or with a near miss, the card read for every message, a resubscribe asking again, none when the card dropped it) and the steps of the fake agent arriving nested, and the same chatty step arriving as often as the agent says it (coalescing is the core's).

**The UI catalog** ([ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)): `SendRequest.ui_catalog` (the delivery the core decided: the catalog inline, or a reference to the current one) is sent **only to an agent whose live card lists `ui-catalog/v1`**. The message then carries `metadata["https://agents.vymalo.com/a2a/extensions/ui-catalog/v1"] = {catalogId, version, digest, inline}` on every message, our catalog id first in the A2UI `supportedCatalogIds`, and the catalog itself in `inlineCatalogs` when it is an inline delivery **and** the A2UI entry of the card says `acceptsInlineCatalogs: true` (otherwise `inline: false` and the agent asks for the catalog again); the URI is activated in `A2A-Extensions` and `message.extensions`. A card without the URI gets exactly the message it got before. *Verified 2026-10-01* (the end-to-end test `two_screens_of_different_versions_reach_the_agent_as_the_extension_says`): a receiver built on `a2a-server-lf` reads every number of the message's metadata as a double (`maxLength: 256` arrives as `256.0`, `version: 2` as `2.0`), so an agent that recomputes the digest of an inline catalog must write a whole number as an integer first (RFC 8785 does). 

**The thread tools** ([`thread-tools/v1`](../../../docs/api/thread-tools-v1.md), [ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)): `SendRequest.thread_tools` (the non-secret grant the dispatcher builds: the thread, the job, the agent, `main`, depth 0; `None` for the verifier) becomes `{url, token, expiresAt}` under the extension's URI in the message metadata, and the URI is activated in `A2A-Extensions` and `message.extensions`, **only** when the card read for this very call lists `thread-tools/v1` (exact URI), the request has a grant and `A2aConfig.thread_tools` has an issuer. The token is minted when the message is sent (one per message, `jti` is the message id, wall-clock `iat`), from the issuer's keys; a grant that cannot be minted is logged without a secret and leaves the agent without tools, never failing the message. Nothing else sees the token: it is not in the request (which holds only the grant), the outbox, the log, or a log line (`ThreadToolsGrant` and the issuer print no secret, and nothing here logs the metadata); a test captures every log line of a send at the most verbose level and finds neither the token, a segment of it, nor the key. A card without the URI, an adapter without keys and a request without a grant get exactly the message they got before.

A selected release is sent as the `A2A-Extensions` header plus namespaced
message metadata; `SendRequest.reference_task_ids` becomes the message's `referenceTaskIds` ([ADR 0021](../../../docs/decisions/0021-context-across-a2a-tasks.md)). **A2UI is sent only when the card read for that very call lists it** (capabilities in the
metadata, the URI in the header and in `message.extensions`); a card that lost it makes the next message plain
A2A again. A surface from an agent is relayed whether or not its card lists the extension, and an action goes
back in the version its surface spoke (ADR 0013). Methods used: `SendStreamingMessage`, `SubscribeToTask`
(the port's `resubscribe`), `GetTask`, `CancelTask`, `ListTasks`.
`SubscribeToTask` only works while the task executes in the answering
process; otherwise `TASK_NOT_FOUND`, which the dispatcher answers by polling
`GetTask`. The SDK loses the HTTP status of failed calls, so errors are
classified by JSON-RPC code and by the SDK's message prefixes.

```rust
use orch_agent_a2a::{A2aAgentClient, A2aConfig};

let agents = A2aAgentClient::new(A2aConfig::default())?;   // then PortSet { agents, .. }
```

Protocol notes above are as recorded in `src/lib.rs`: *verified* against the
SDK sources on 2026-09-29 by that crate's author; not re-checked for this
README.

## Features and environment

No Cargo features. The crate reads no environment variables; bearer tokens
arrive in the endpoint's `AgentTransport::A2a { card_url, bearer }` (the binary
resolves `tokenEnv` from `AGENTS_FILE`); the adapter serves that transport only.
An `AgentTransport::Local` endpoint (an agent hosted in-process) is answered with
`AgentError::Unsupported` by every operation, never dereferenced as a card URL.

## Tests

`tests/against_fake_agent.rs`: the adapter against an in-process A2A 1.0 agent
(`orch-testsupport`'s `FakeAgent`) over real HTTP. `tests/a2ui.rs`: capability detection on and off (no extension, each URI, both, near-miss URIs), a card that
changes between calls, the capabilities and activation per version, surfaces from an artifact, a message and a
status message, refusal of malformed and oversized parts, a poll keyed like the stream, and an action arriving
at the agent as a data part of the same task. `tests/ui_catalog.rs`: the UI catalog against the in-process agent: inline to an agent that lists the extension and takes it (and its digest checks out), only a reference (and our id first) to one that does not take it inline or for a reference delivery, plain A2A for a card without the URI (near misses included) or for a message with no delivery, the card read for every message, an agent that lists the extension but not A2UI, the other dialect, and `read_card` listing the extensions. `tests/thread_tools.rs`: the thread tools against the in-process agent: the endpoint and a token that verifies with the keys (thread, job, agent, caller, depth, the message id as `jti`, a two-hour lifetime, `expiresAt` the token's `exp`) for a card that lists the extension, a token of its own for every message, plain A2A for a card without the exact URI (five near misses included), without keys, or for a request with no grant, the card read for every message, an asked agent's caller and depth; `tests/thread_tools_logs.rs` (a process of its own, since a `tracing` subscriber has to see the callsites first): nothing logged during a send, at the most verbose level, holds the token, a segment of it or the key, and `Debug` of the request, the client, the issuer and the grant shows none. `src/a2ui.rs` and `src/extensions.rs` (unit): the capabilities in both dialects, the metadata, `acceptsInlineCatalogs` (only `true` counts, only the entry of the version spoken) and the exact-URI detection. `tests/conformance.rs`: the
`AgentClient` conformance testkit of `orch-ports` (`agent_client_conformance!`)
run against the same fake agent. Offline, no environment
variables. The WireMock stand-ins of `compose.yaml` are exercised by
`orch-e2e`'s `wiremock_agent` test (see [`orch-e2e`](../e2e/README.md)).

## See also

[`orch-ports`](../ports/README.md),
[`orch-a2a-mapping`](../a2a-mapping/README.md),
[`orch-testsupport`](../testsupport/README.md),
[`orch-e2e`](../e2e/README.md).
