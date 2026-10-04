# orch-model-openai

`ChatModel` over OpenAI-compatible chat completions endpoints: one JSON
`POST {base}/chat/completions` with `stream: false`, and the text of the first
choice as the answer. It holds **one client configuration per endpoint name**
(`models.endpoints.<name>` of the configuration file) and sends each request to the
endpoint it names.

## Where it sits

An **adapter** of the `ChatModel` port in [`orch-ports`](../ports/README.md)
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)). It is the one
protocol every model endpoint speaks (a hosted model, a gateway, a local server), so nothing
here is specific to a vendor: no host SDK, no gateway product
([ADR 0005](../../../docs/decisions/0005-openai-compatible-model-endpoint.md),
[ADR 0007](../../../docs/decisions/0007-protocol-only-dependencies.md)). Only the binary
([`orchestrator`](../../bin/orchestrator/README.md)) depends on it, and only to build the model
the dispatcher asks for the utility tasks (a thread's title and description,
[ADR 0035](../../../docs/decisions/0035-utility-model-tasks.md)).

## API at a glance

| Item | What |
|---|---|
| `OpenAiChat::new(impl IntoIterator<Item = (String, OpenAiConfig)>) -> Result<_, BuildError>` | implements `ChatModel` over the endpoints it is given, by name; installs the `rustls` crypto provider if none is installed. **A request names its endpoint** (`ChatRequest.endpoint`); a name the adapter does not hold is `ModelError::NotConfigured` and nothing is sent anywhere (the configuration checks that every task names an endpoint, so that is a bug, never a configuration outcome). No endpoints is a model that holds none. `endpoint_names()` lists them. Cheap to clone (the endpoints, each with its own HTTP client, key and timeout, are shared) |
| `OpenAiConfig::new(base_url)`, `.with_api_key(SecretString)`, `.with_timeout(Duration)` | `base_url` is up to and not including `/chat/completions` (`https://api.openai.com/v1`; a trailing slash is fine); `api_key` is the bearer token, none by default (an empty one is none); `timeout` covers connecting and answering together (20 s); `use_system_proxy` is off by default |
| `BuildError` | `BadBaseUrl` (not `http://` or `https://`), `BadApiKey` (not a header value), `Http` (the TLS backend did not start); each names the endpoint, by its name, never a URL or a key |

The request is `{"model", "messages": [{"role": "system", ...}, {"role": "user", ...}], "max_tokens",
"stream": false}`; `max_tokens` is the member every compatible server reads (a hosted model that
asks for `max_completion_tokens` instead is not supported yet).

How an answer maps to the port's errors (`Classify`):

| The endpoint | `ModelError` | Class |
|---|---|---|
| 2xx with a choice that has text | the text | |
| 2xx that is not a chat completion, has no text, or is over 256 KiB | `Protocol` | transient |
| 429 | `RateLimited` (its `Retry-After` in seconds, at most an hour) | rate limited |
| 401, 403 | `Unauthenticated` | permanent |
| 408, 5xx, a timeout, a connection that failed | `Unreachable` (no URL in the text) | transient |
| a redirect | `Rejected` ("configure the final URL") | permanent |
| any other 4xx | `Rejected` (the endpoint's own `error.message`, cut at 300 characters, never the request) | permanent |

The credential is sent as a **sensitive** `Authorization: Bearer` header, is never part of an error,
and `OpenAiConfig` and `OpenAiChat` print `<redacted>` for it in `Debug`, and for the endpoint's address too (a deployment may keep it in a secret store, `baseUrl: { file }`: [ADR 0035](../../../docs/decisions/0035-utility-model-tasks.md)). A redirect is never
followed, so the token goes to the configured endpoint and nowhere else. Nothing is retried here:
the caller decides by the error's class.

## Tests

Offline: an in-process `axum` stub of the endpoint, over real HTTP. No environment variables.

* `tests/openai.rs`: the `ChatModel` conformance suite of `orch-ports` (`chat_model_conformance!`:
  the answer is the model's text and the endpoint was asked as put, a failing endpoint and nonsense
  are transient, a rate limit is rate limited, a refusal is permanent, a refused credential is
  unauthenticated, and the key is in no error, an endpoint the adapter does not hold is `NotConfigured`), and what only this adapter says: the exact request
  body and bearer token, an endpoint that wants no key is sent none, the base URL with a trailing
  slash or a path, an answer with no text (six shapes) is a retryable protocol error, the wait asked
  for passed on and bounded, a refusal's words cut and never the request, a redirect not followed, a
  timeout and a port nobody listens on (unreachable, no address in what is logged), an answer over
  the bound not read, a bad base URL or key refused when the adapter is built (naming the endpoint), `Debug` redacting
  the key and the address, and several endpoints (each request goes to the endpoint it names, with that endpoint's key and
  timeout; a name that is not held is `NotConfigured` and nothing is sent).
