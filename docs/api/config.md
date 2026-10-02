# The orchestrator's configuration file

> **Status: built (PR S9 of plan 10, 2026-10-02; the `artifacts` section by S10; `auth.mode`, `auth.jwt` and `server.environment` by S14).** The decision is
> [ADR 0034](../decisions/0034-one-yaml-configuration-secrets-by-reference.md) (the file, secrets by reference,
> validation, migration), [ADR 0035](../decisions/0035-utility-model-tasks.md) (the `models` and `tasks` sections),
> [ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md) (the `artifacts` section) and
> [ADR 0033](../decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md) (the `auth` section and `server.environment`).
> The loader builds every key marked **now** (crate [`orch-config`](../../orchestrator/crates/config/README.md), the
> loader in [`orchestrator/bin/orchestrator`](../../orchestrator/bin/orchestrator/README.md#the-configuration-file));
> a key marked **reserved** belongs to the PR named beside it and is refused (exit 78, naming that PR and ADR) until it
> lands. The JSON Schema is [`config.schema.json`](config.schema.json). `GET /api/config` is not served yet (S18).
> The environment variables of
> [`orchestrator/bin/orchestrator/README.md`](../../orchestrator/bin/orchestrator/README.md#environment) still work in
> this release, over the file.

One YAML file, named by `ORCH_CONFIG_FILE` (or `--config <path>`), read once at startup. It configures the
orchestrator only: the web has no configuration and no secrets ([architecture](../architecture.md)), and reads the
public `ui` subset of this file from [`GET /api/config`](#get-apiconfig).

## Rules

- **`version: 1`** is required. Another value, or none, is exit 78 ("this build reads version 1").
- **Unknown keys are errors** (`deny_unknown_fields` on every struct; the schema says `additionalProperties: false`).
- **A secret is a reference**, never a value: `{ env: NAME }` (the variable must be set and not empty) or
  `{ file: /run/secrets/name }` (read at startup, at most 64 KiB; one trailing `\n` or `\r\n` is cut; an `{ env }`
  value is trimmed, as a `tokenEnv` value is today). A plain string where a secret goes is exit 78: `database.url: a secret is a reference: { env: NAME }
  or { file: PATH }`. The message never carries a value.
- **Relative paths** (`agents.file`, `mcp.tokensFile`, a secret's or a prompt's `file`) are relative to the directory
  of the configuration file.
- **No value in an error.** A message names the key path, what is wrong and what is allowed, never a value of the
  file, the environment or a secret file.
- **A key repeated in one mapping is an error**, never "the last one wins".
- **Every error at once.** A YAML syntax error is reported alone (nothing after it can be read). Otherwise each pass
  lists all of its errors, each with its key path, then the process exits 78: the shape errors (unknown or reserved key,
  wrong type, missing required key, a value out of range or not allowed, a plain string as a secret), and, when there
  are none, the rule errors (a URL, a cross-key rule such as "both or neither of `threadTools.url` and `secret`", a
  secret that cannot be read) and then the rules of the gate against the agents, the surfaces and features this build
  has, the agents file and the MCP tokens, which are checked one at a time. A pass runs only when the one before it found
  none: a rule cannot be checked on a value that is not there.
- **Anchors and aliases** are resolved by the parser and merge keys (`<<:`) are applied before validation; a YAML tag
  (`!something`) is an error.
- **A key that selects an implementation** (`artifacts.store`, `auth.mode`, a surface in `server.surfaces`) names only
  what this build compiled in; anything else is exit 78 naming the Cargo feature, as `ORCH_SURFACES` does today.
- The JSON Schema of the file is generated from the Rust types (`schemars`) and committed at
  [`docs/api/config.schema.json`](config.schema.json); a test fails when the types and the committed file differ
  (regenerate with `UPDATE_SCHEMA=1 cargo test -p orch-config --test schema`, as the goldens do with `UPDATE_GOLDEN=1`).
  Editors can validate against it. An optional key is simply optional in it: a key written with nothing after it is a
  type error.
- **The variables over the file** (this release). A flag or its variable that is set wins over the file, which wins
  over the default, and the process logs one warning per variable naming the variable and the key (never the value).
  Each is read with its own parser first (`ORCH_STEPS_RECORD_IO=yes` is `true`, `ORCH_SURFACES` a comma list), and a
  value that does not parse is an error naming the variable. `ORCH_ROLE`, `ORCH_INSTANCE_ID`, `RUST_LOG` and
  `HOSTNAME` are logged at info. A variable that says what the file says, or that the file names as `{ env: NAME }`,
  overrides nothing. The `ORCH_MODEL_*` variables are the endpoint `default`, `ORCH_TITLE_MODEL` is `tasks.title.model`
  (with `tasks.title.endpoint: default` when the file has none); a file that names another endpoint beside them is an
  error naming both. A variable of a group whose first key is not set (a registry timeout with no registry URL, the
  model key or timeout with no base URL, a webhook age with no secrets) is read, and left out, as it always was.
- **A process that serves no routes** (`server.role: worker`) is not asked for the secrets of the routes it would not
  mount (`webhooks.*`), so one file serves a control plane and its workers; the secrets every role uses
  (`database.url`, the registry's, `threadTools`, a model endpoint's) are read by all.

## An example

```yaml
version: 1
server:
  listen: 0.0.0.0:8080
  surfaces: [agui, mcp, thread-tools, webhook-generic, webhook-github]
  publicUrl: https://chat.example.com
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
  registry:
    url: https://platform.example.com/registry/v1/agents
    agentToken: { file: /run/secrets/registry-agent-token }
gate:
  require: [agent-checks, ci]
  ci: { required: [build] }
steps:
  recordToolIo: true
models:
  endpoints:
    default:
      baseUrl: https://models.example.com/v1
      apiKey: { env: ORCH_MODEL_API_KEY }
tasks:
  title: { endpoint: default, model: small-model }
artifacts:
  store: fs
  fs: { root: /var/lib/orchestrator/artifacts }
threadTools:
  url: http://orchestrator:8080
  secret: { file: /run/secrets/thread-tools }
mcp:
  tokensFile: mcp-tokens.yaml
  allowedHosts: [chat.example.com]
webhooks:
  generic:
    secrets: [{ env: WEBHOOK_GENERIC_SECRET }]
  github:
    secrets: [{ env: WEBHOOK_GITHUB_SECRET }]
```

`orchestrator --print-config` prints the configuration this process would run with (the file, the environment over it,
the defaults filled in), secrets as their references, and exits 0; with errors it lists them and exits 78. It opens no
connection. It reads the secrets' variables and files, as a start does, so it checks a file where it will run (any dummy
value does for a check elsewhere); the notes about variables that override the file go to stderr, the YAML to stdout.

## Every key

**Now** is built by S9 (the `artifacts` keys by S10); **reserved** names the PR and the ADR that bring it. "Replaces" is the environment variable
(and flag) of today; during the transition release it still works and wins over the file, with a warning naming the
variable and the key ([ADR 0034](../decisions/0034-one-yaml-configuration-secrets-by-reference.md#migration)). A
secret variable of today stands for a reference to itself: `ORCH_MODEL_API_KEY` set means
`models.endpoints.default.apiKey: { env: ORCH_MODEL_API_KEY }`.

### `server`, `log`, `database`, `dispatcher`, `inbox`

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `server.listen` | socket address, `0.0.0.0:8080` | `LISTEN_ADDR` | now |
| `server.role` | `all` \| `control-plane` \| `worker`, `all` | `ORCH_ROLE` (**kept**: a process override, see below) | now |
| `server.instanceId` | string, `$HOSTNAME-<uuid>` | `ORCH_INSTANCE_ID` (**kept**: a process override) | now |
| `server.surfaces` | list of `agui`, `mcp`, `thread-tools`, `webhook-generic`, `webhook-github`; `[agui]` | `ORCH_SURFACES` (a comma list) | now |
| `server.publicUrl` | origin, none | `ORCH_PUBLIC_URL` | now |
| `server.shutdownGraceSecs` | ≥ 1, `15` | `SHUTDOWN_GRACE_SECS` | now |
| `server.environment` | `development` \| `production`, `development` | — (plan 10 §3.4 called it `ORCH_ENV`; it is a key, not a variable) | now. A `production` process refuses `auth.mode: proxy_header`, and an `http://` `auth.jwt.issuer` or `jwksUrl` ([Authentication](#authentication)) |
| `log.format` | `json` \| `text`, `json` | `LOG_FORMAT` | now |
| `database.url` | **secret** (required) | `DATABASE_URL` | now |
| `database.maxConnections` | ≥ 2, `10` | `DATABASE_MAX_CONNECTIONS` | now |
| `dispatcher.concurrency` | ≥ 1, `32` | `DISPATCHER_CONCURRENCY` | now |
| `dispatcher.outboxLeaseSecs` | ≥ 3, `30` | `OUTBOX_LEASE_SECS` | now |
| `inbox.leaseSecs` | ≥ 3, `30` | `INBOX_LEASE_SECS` | now |
| `inbox.pollSecs` | ≥ 1, `2` | `INBOX_POLL_SECS` | now |
| `inbox.parkedTtlSecs` | ≥ 1, `86400` | `INBOX_PARKED_TTL_SECS` | now |
| `inbox.maxAttempts` | ≥ 1, `10` | `INBOX_MAX_ATTEMPTS` | now |

### `agents`

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `agents.file` | path; required unless `agents.registry.url` is set | `AGENTS_FILE` | now. The file's format (`{id, name, transport?, cardUrl?, tokenEnv?, agent?, gate?}`) does not change |
| `agents.localConcurrency` | ≥ 1, `4`; a build without the feature `agent-local` refuses the key (78) | `AGENT_LOCAL_CONCURRENCY` | now |
| `agents.registry.url` | `http(s)` URL without credentials; a build without `registry-platform` refuses it (78) | `AGENT_REGISTRY_URL` | now |
| `agents.registry.token` | **secret**, none | `AGENT_REGISTRY_TOKEN` | now |
| `agents.registry.agentToken` | **secret**, none | `AGENT_REGISTRY_AGENT_TOKEN` | now |
| `agents.registry.timeoutSecs` | 1 to 60, `3` | `AGENT_REGISTRY_TIMEOUT_SECS` | now |
| `agents.registry.maxAgeSecs` | 1 to 3600, `60` | `AGENT_REGISTRY_MAX_AGE_SECS` | now |

### `gate`

The deployment's layer of the verification gate ([ADR 0018](../decisions/0018-verification-gate-and-rework-loop.md)),
with the same member names as an agent entry's `gate` in the agents file, plus the keys only a deployment has.

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `gate.require` | list of `ci`, `agent-checks`, `verifier`; `[]` | `ORCH_GATE` | now |
| `gate.maxAttempts` | 1 to the cap, `3` | `ORCH_MAX_ATTEMPTS` | now |
| `gate.maxAttemptsCap` | 1 to 100, `10` | `ORCH_MAX_ATTEMPTS_CAP` | now |
| `gate.verifier` | an agent id, none | `ORCH_VERIFIER` | now |
| `gate.verifierTimeoutSecs` | ≥ 1, `1800` | `ORCH_VERIFIER_TIMEOUT_SECS` | now |
| `gate.verifierWatchSecs` | ≥ 1, `5` | `ORCH_VERIFIER_WATCH_SECS` | now |
| `gate.ci.required` | list of check names, `[]` | `ORCH_CI_REQUIRED` | now |
| `gate.ci.timeoutSecs` | ≥ 1, `3600` | `ORCH_CI_TIMEOUT_SECS` | now |

### `steps`, `models`, `tasks`, `ui`

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `steps.recordToolIo` | boolean, `true` | `ORCH_STEPS_RECORD_IO` (which also takes `1`/`0`, `yes`/`no`, `on`/`off`; the file takes YAML's `true`/`false` only) | now. The bounds (4 KiB, 8 KiB, 2 MiB per job) stay the core's constants ([ADR 0030](../decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)); they are not keys |
| `models.endpoints.<name>.baseUrl` | `http(s)` URL up to `/chat/completions` | `ORCH_MODEL_BASE_URL` (endpoint `default`) | now, **one** endpoint; several: S18 |
| `models.endpoints.<name>.apiKey` | **secret**, none | `ORCH_MODEL_API_KEY` (endpoint `default`) | now |
| `models.endpoints.<name>.timeoutSecs` | ≥ 1, `20` | `ORCH_MODEL_TIMEOUT_SECS` (endpoint `default`) | now |
| `tasks.title.endpoint` | an endpoint name (required with `tasks.title`) | — (`default` when `ORCH_TITLE_MODEL` is used) | now |
| `tasks.title.model` | the model's name at the endpoint (required with `tasks.title`) | `ORCH_TITLE_MODEL` | now. **No `tasks.title`, no title model: titles are off**, as with `ORCH_TITLE_MODEL` unset |
| `tasks.title.system`, `.maxTokens`, `.language` | see [ADR 0035](../decisions/0035-utility-model-tasks.md#per-task-settings) | — | reserved: S18, ADR 0035 |
| `tasks.description.*` | see ADR 0035 | — | reserved: S18, ADR 0035 |
| `tasks.turnSummary`, `tasks.stepLabel` | names kept for later tasks | — | reserved, no PR yet: refused |
| `ui.showDescriptions` | boolean, `true` | — | reserved: S18 (served), S19 (read by the web) |

### `threadTools`, `mcp`, `webhooks`, `auth`, `artifacts`

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `threadTools.url` | `http(s)` base URL; both or neither with `secret` | `THREAD_TOOLS_URL` | now |
| `threadTools.secret` | **secret**, ≥ 32 bytes | `THREAD_TOOLS_SECRET` | now |
| `threadTools.previousSecret` | **secret**, ≥ 32 bytes, not the current one | `THREAD_TOOLS_SECRET_PREVIOUS` | now |
| `threadTools.tokenTtlSecs` | 60 to 86400, `7200` | `THREAD_TOOLS_TOKEN_TTL_SECS` | now |
| `threadTools.allowedHosts` | list of `host[:port]`, the host of `url` | `THREAD_TOOLS_ALLOWED_HOSTS` | now |
| `mcp.tokensFile` | path; required when `mcp` is mounted. Its format (`{user, tokenEnv}`) does not change | `MCP_TOKENS_FILE` | now; a `role` per token: S15, ADR 0033 |
| `mcp.allowedHosts` | list of `host[:port]`; required when `mcp` is mounted | `MCP_ALLOWED_HOSTS` | now |
| `mcp.allowedOrigins` | list of origins, `[]` | `MCP_ALLOWED_ORIGINS` | now |
| `mcp.waitMaxSecs` | 1 to 86400, `3600` | `MCP_WAIT_MAX_SECS` | now |
| `mcp.waitMaxConcurrent` | ≥ 1, `256` | `MCP_WAIT_MAX_CONCURRENT` | now |
| `mcp.waitMaxPerUser` | ≥ 1, `16` | `MCP_WAIT_MAX_PER_USER` | now |
| `webhooks.generic.secrets` | list of 1 or 2 **secrets**, each ≥ 32 bytes; required when `webhook-generic` is mounted | `WEBHOOK_GENERIC_SECRETS` (one variable, comma rule kept) | now |
| `webhooks.generic.maxSkewSecs` | ≥ 1, `300` | `WEBHOOK_GENERIC_MAX_SKEW_SECS` | now |
| `webhooks.github.secrets` | list of 1 or 2 **secrets**; required when `webhook-github` is mounted | `WEBHOOK_GITHUB_SECRETS` (comma rule kept) | now |
| `webhooks.github.maxAgeSecs` | ≥ 1, `86400` | `WEBHOOK_GITHUB_MAX_AGE_SECS` | now |
| `auth.devUser` | an e-mail, none; development only (warns); only with `auth.mode: proxy_header` | `AUTH_DEV_USER` | now |
| `auth.mode` | `proxy_header` \| `jwt` \| `jwt_or_proxy_header`, `proxy_header` (nothing changes); a mode whose Cargo feature (`auth-jwt`, `auth-header`) is not in the build is refused (78) | — | now |
| `auth.jwt.issuer` | `http(s)` URL without credentials, query or fragment; required with a mode that reads tokens | — | now |
| `auth.jwt.audiences` | list of at least one non-empty text; required with `issuer` | — | now |
| `auth.jwt.jwksUrl` | `http(s)` URL without credentials, none (the keys are found from `<issuer>/.well-known/openid-configuration`) | — | now |
| `auth.jwt.userClaim` | the claim whose value is the user, `email` | — | now |
| `auth.jwt.rolesClaim` | a dotted path (`realm_access.roles`, `groups`), none (no roles) | — | now; read into `Principal.roles`, nothing reads them until S15 |
| `auth.defaultRole`, `auth.roles` | plan 10 §3.4 | — | reserved: S15, ADR 0033 |
| `artifacts` | the artifact store ([ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md)). Absent: no store, and a file an agent hands over is refused with "no artifact store configured". Present: `store` is required | — | now (S10) |
| `artifacts.store` | `fs` \| `s3`; required with the section. Names only what this build compiled in: `fs` needs the Cargo feature `artifacts-fs`, `s3` needs `artifacts-s3`, else exit 78 naming it. The section of the store chosen is required and the other one is an error | — | now |
| `artifacts.fs.root` | path (relative to this file's directory); required with `store: fs`. Made (mode `0700`) when missing. Every role must see the same directory (one machine, or a shared volume) | — | now |
| `artifacts.s3.bucket` | 3 to 63 characters of `a-z`, `0-9`, `-`, `.`, starting and ending with a letter or a digit; required with `store: s3`; the bucket must exist | — | now |
| `artifacts.s3.region` | slug, `us-east-1` | — | now |
| `artifacts.s3.endpoint` | `http(s)` URL with a host, no credentials, query or fragment; absent: AWS S3. The bucket is then in the path (`<endpoint>/<bucket>/<key>`). An `http` endpoint sends the files in the clear | — | now |
| `artifacts.s3.prefix` | `a-z A-Z 0-9 . _ - /`, no `..`, at most 128 characters; keys go under it, so one bucket serves several deployments | — | now |
| `artifacts.s3.accessKeyId`, `artifacts.s3.secretAccessKey` | **secrets**, required with `store: s3`. Static credentials only: this build does not read `AWS_*` variables or an instance profile | — | now |
| `artifacts.s3.timeoutSecs` | 1 to 3600, `60` (one request) | — | now |
| `artifacts.maxFileBytes` | 1 to 268435456 (256 MiB), `10485760` (10 MiB). Read by the ingest of S11: a larger file is not kept | — | now (read: S11) |
| `artifacts.maxPerJobBytes`, `artifacts.fetchHosts` | plan 10 §3.3: the bytes a job may keep (100 MiB) and the hosts a `url` part may be fetched from (none) | — | reserved: S11, ADR 0032 |

### Authentication

```yaml
server:
  environment: production
auth:
  mode: jwt
  jwt:
    issuer: https://idp.example/realms/main
    audiences: [oauth2-proxy-client-id]
    userClaim: email                    # the default: keeps the threads that exist
    rolesClaim: realm_access.roles      # optional
```

- **`proxy_header`** (the default) trusts `X-Auth-Request-Email` and `auth.devUser`: safe only behind a proxy that strips
  client-supplied copies. **`jwt`** validates the `Authorization: Bearer` token oauth2-proxy forwards (the ID token by
  default; its `aud` is oauth2-proxy's client id) or a client's own token, and reads no header. **`jwt_or_proxy_header`**
  takes the token when the request has one (and a refused token is never reconsidered as the header) and the header when
  it has not: one release of migration.
- **Rules** (exit 78, each naming its key): `auth.jwt` is required by `jwt` and `jwt_or_proxy_header` and refused by
  `proxy_header` (a key that does nothing is an error); `auth.devUser` only with `proxy_header`; **`proxy_header` is
  refused when `server.environment` is `production`**; a mode needs its Cargo feature. A process that serves no routes
  (`server.role: worker`) needs no authenticator.
- **The token** must be signed with RS256, RS384, ES256 or EdDSA by a key of the issuer's JWKS, and carry `iss` equal to
  `issuer`, one of `audiences` in `aud`, `exp` and `iat`; 60 seconds of leeway; `email_verified` must not be `false`. The
  user is `userClaim`, trimmed and lower-cased. See [ADR 0033](../decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md#2-the-jwt-authenticator-orch-auth-jwt-authmode-jwt)
  for the key cache (10 minutes; an unknown `kid` at most once in 30 seconds; an hour of grace) and what is bounded.
- **Responses:** 401 with `WWW-Authenticate: Bearer` for no token or a refused one; **503** with `Retry-After` while the
  issuer's keys cannot be fetched; `/readyz` is 503 until they have been fetched.

### Variables that are not keys

| Variable | Why it has no key |
|---|---|
| `ORCH_CONFIG_FILE` | Names the file (also `--config`). Kept for good. Unset in the transition release: the environment alone configures the process, as today |
| `RUST_LOG` | The log filter of `tracing-subscriber` (`EnvFilter`), read before anything else; the standard way to change verbosity of one process. Kept for good |
| `HOSTNAME` | Set by the container runtime; only the first part of `server.instanceId`'s default. Kept |
| the `tokenEnv` names of the agents file and of the MCP tokens file (`CODER_A2A_TOKEN`, `MCP_TOKEN_<NAME>`, …) | Not the orchestrator's variables: those files name them, and already hold references, not values. Unchanged |

**Process overrides.** `ORCH_ROLE` and `ORCH_INSTANCE_ID` (and their flags `--role`, `--instance-id`) stay after the
deprecation: they describe one process, not the deployment, and a control plane and its workers share one file. They
win over the file and log at `info`, not `warn`.

**Inventory** (verified 2026-10-02 against `orchestrator/bin/orchestrator/src/config.rs` at `9dddfc1`, every
`#[arg(env = …)]`, and `logging.rs`): 52 variables are declared as flags with an environment fallback (one of them,
`HOSTNAME`, hidden), and `RUST_LOG` is read by `EnvFilter`: 53 names. 51 map to a key above; `HOSTNAME` and `RUST_LOG`
do not. No other crate of `orchestrator/` reads the environment outside tests and test helpers.

## `GET /api/config`

**Reserved: S18** (served) **and S19** (read by the web). The public subset of the configuration, for the web.

```http
GET /api/config
→ 200 application/json
{ "ui": { "showDescriptions": true } }
```

- The body is exactly `{ "ui": { … } }`: every key of the `ui` section, with its effective value (defaults filled in).
  Nothing else of the file is ever in it. The `ui` section's Rust type has no secret field, and a test asserts that its
  schema has no secret reference, so a secret cannot be added to it by mistake.
- **Behind the identity layer**, like every `/api/*` route (401 without an identity, fail closed). The web asks for it
  after the edge has let the browser in, and nothing in it is needed before that.
- It changes only when the process restarts. The web treats a missing key as its default and ignores a key it does not
  know, so the section grows additively; a key is never renamed or retyped in `version: 1`.
</content>
</invoke>
