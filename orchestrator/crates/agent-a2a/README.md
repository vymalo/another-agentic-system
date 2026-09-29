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
(HTTP, streaming, resubscribe, errors, release channels).

## API at a glance

| Item | What |
|---|---|
| `A2aAgentClient::new(A2aConfig) -> Result<_, BuildError>` | implements `AgentClient`; installs the `rustls` crypto provider if none is installed |
| `A2aConfig` | `card_timeout` (5 s), `connect_timeout` (10 s), `read_timeout` (90 s), `call_timeout`, `use_system_proxy` |
| `releases_from_card(&AgentCard) -> Option<Releases>`, `RELEASE_CHANNELS_URI` | reads the optional release-channels extension from the live card (the URI is defined in [`orch-a2a-mapping`](../a2a-mapping/README.md) and re-exported) |
| `ui_from_card(&AgentCard) -> Option<UiSupport>` | reads the optional A2UI extension ([ADR 0013](../../../docs/decisions/0013-a2ui-generative-ui.md)) from the live card: an `extensions` entry whose `uri` is exactly `https://a2ui.org/a2a-extension/a2ui/v0.9.1` or `…/v1.0` (both detected, open question 22; `v0.9.1` preferred when both are listed). Anything else is not A2UI |
| `client_capabilities(UiVersion)`, `action_part(&UiActionData, Timestamp)` | the renderer capabilities a message carries (`a2uiClientCapabilities` `{"v0.9.1": {supportedCatalogIds}}`, or `a2uiRendererCapabilities` `{"v1.0": …}`), and the `application/a2ui+json` data part that carries a user's action back (`[{"version", "action": {name, surfaceId, sourceComponentId, timestamp, context}}]`) |
| `install_crypto_provider()` | idempotent `rustls` provider setup |

A selected release is sent as the `A2A-Extensions` header plus namespaced
message metadata. **A2UI is sent only when the card read for that very call lists it** (capabilities in the
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
at the agent as a data part of the same task. `tests/conformance.rs`: the
`AgentClient` conformance testkit of `orch-ports` (`agent_client_conformance!`)
run against the same fake agent. Offline, no environment
variables. The WireMock stand-ins of `compose.yaml` are exercised by
`orch-e2e`'s `wiremock_agent` test (see [`orch-e2e`](../e2e/README.md)).

## See also

[`orch-ports`](../ports/README.md),
[`orch-a2a-mapping`](../a2a-mapping/README.md),
[`orch-testsupport`](../testsupport/README.md),
[`orch-e2e`](../e2e/README.md).
