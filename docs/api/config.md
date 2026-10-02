# The orchestrator's configuration file

> **Status: built (PR S9 of plan 10, 2026-10-02; the `artifacts` section by S10; `auth.mode`, `auth.jwt` and `server.environment` by S14; `auth.roles` and `auth.defaultRole` by S15; the `models`, `tasks` and `ui` sections by S18; the `toolServers` section by slice 8, [ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md)).** The decision is
> [ADR 0034](../decisions/0034-one-yaml-configuration-secrets-by-reference.md) (the file, secrets by reference,
> validation, migration), [ADR 0035](../decisions/0035-utility-model-tasks.md) (the `models` and `tasks` sections),
> [ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md) (the `artifacts` section) and
> [ADR 0033](../decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md) (the `auth` section, its roles, and `server.environment`).
> The loader builds every key marked **now** (crate [`orch-config`](../../orchestrator/crates/config/README.md), the
> loader in [`orchestrator/bin/orchestrator`](../../orchestrator/bin/orchestrator/README.md#the-configuration-file));
> a key marked **reserved** belongs to the PR named beside it and is refused (exit 78, naming that PR and ADR) until it
> lands. The JSON Schema is [`config.schema.json`](config.schema.json). [`GET /api/config`](#get-apiconfig) serves the
> `ui` section.
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
- **A prompt is read once, at startup.** `tasks.<task>.system` is `{ inline: TEXT }` or `{ file: PATH }`; a file is
  read through the same door as a secret's (UTF-8), and a change needs a restart. With the space around it cut it is 1
  to 4096 bytes: empty or longer is exit 78, naming the key and the file, never the text. A prompt is not a secret,
  and `--print-config` shows it as the reference the file has (`{ file: prompts/title.md }`), never the text.
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
  overrides nothing. The `ORCH_MODEL_*` variables are the endpoint `default` (they add it to the file's other
  endpoints, or override its keys when the file has one of that name), `ORCH_TITLE_MODEL` is `tasks.title.model`
  (with `tasks.title.endpoint: default` when the file has none). A variable of a group whose first key is not set (a registry timeout with no registry URL, the
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
    small:
      baseUrl: https://small.example.com/v1
      timeoutSecs: 10
tasks:
  title: { endpoint: small, model: small-model }
  description:
    endpoint: default
    model: small-model
    system: { inline: "Say in one or two sentences what the person wants and where it stands." }
    maxTokens: 160
    recompute: { minNewMessages: 4 }
ui:
  showDescriptions: true
artifacts:
  store: fs
  fs: { root: /var/lib/orchestrator/artifacts }
threadTools:
  url: http://orchestrator:8080
  secret: { file: /run/secrets/thread-tools }
mcp:
  tokensFile: mcp-tokens.yaml
  allowedHosts: [chat.example.com]
auth:
  mode: jwt
  jwt:
    issuer: https://idp.example/realms/main
    audiences: [oauth2-proxy-client-id]
    rolesClaim: realm_access.roles
  defaultRole: user           # the built-in user and admin (see "Roles and permissions")
webhooks:
  generic:
    secrets: [{ env: WEBHOOK_GENERIC_SECRET }]
  github:
    secrets: [{ env: WEBHOOK_GITHUB_SECRET }]
toolServers:                  # what a person may attach to a conversation (see "toolServers")
  - id: websearch
    name: Web search
    description: Search the web.
    url: https://search.example.com/mcp
    icon: data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnLz4=
    bearer: { env: WEBSEARCH_TOKEN }
    tools: [search]
    agents: [chat, researcher]
```

`orchestrator --print-config` prints the configuration this process would run with (the file, the environment over it,
the defaults filled in), secrets as their references, and exits 0; with errors it lists them and exits 78. It opens no
connection. It reads the secrets' variables and files, as a start does, so it checks a file where it will run (any dummy
value does for a check elsewhere); the notes about variables that override the file go to stderr, the YAML to stdout.

## Every key

**Now** is built (S9; the `artifacts` keys by S10 and S11, the `auth` keys by S14 and S15, the `models`, `tasks` and `ui` keys by S18); **reserved** names the PR and the ADR that bring it. "Replaces" is the environment variable
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
| `models.endpoints.<name>.baseUrl` | `http(s)` URL up to `/chat/completions`; a name is a slug (`a-z`, `0-9`, `-`, 1 to 32 characters) | `ORCH_MODEL_BASE_URL` (endpoint `default`) | now: **several endpoints** |
| `models.endpoints.<name>.apiKey` | **secret**, none | `ORCH_MODEL_API_KEY` (endpoint `default`) | now |
| `models.endpoints.<name>.timeoutSecs` | ≥ 1, `20`; the longest one try of a task at this endpoint may take | `ORCH_MODEL_TIMEOUT_SECS` (endpoint `default`) | now |
| `tasks.title.endpoint` | an endpoint name (required with `tasks.title`) | — (`default` when `ORCH_TITLE_MODEL` is used) | now. A name that is not in `models.endpoints` is exit 78 |
| `tasks.title.model` | the model's name at the endpoint (required with `tasks.title`) | `ORCH_TITLE_MODEL` | now. **No `tasks.title`, no title model: titles are off**, as with `ORCH_TITLE_MODEL` unset |
| `tasks.title.system` | `{ inline: TEXT }` or `{ file: PATH }`, 1 to 4096 bytes; default: the core's guidance ("Reply with a 3 to 6 word title for the conversation, in plain text.") | — | now. Replaces the **guidance** only: the core always adds the form of the answer, the data clause, the fence and the language line ([ADR 0035](../decisions/0035-utility-model-tasks.md#4-what-the-core-always-adds)) |
| `tasks.title.maxTokens` | 1 to 256, `32` | — | now |
| `tasks.title.language` | `conversation` (default) or `english`, `french`, `german`, `spanish`, `portuguese`, `italian`, `chinese`, `japanese`, `korean`, `cyrillic`, `arabic`, `hebrew`, `greek`, `devanagari`, `thai` | — | now. `conversation`: the language of the **person's** messages, found by the core and checked on the answer; a fixed language is named last and the answer is checked against **its** script |
| `tasks.description.endpoint`, `.model`, `.system`, `.maxTokens`, `.language` | as the title's; `maxTokens` 1 to 1024, `160`; default guidance: "Describe in one or two sentences what the conversation is about now: what the person wants and where it stands." | — | now. **No `tasks.description`, no descriptions**: no row, no model call |
| `tasks.description.maxChars` | 40 to 500, `300`; the cleaned answer is cut to it at a word | — | now |
| `tasks.description.recompute.minNewMessages` | ≥ 1, `4`; the messages (the person's, and the agent's final words) since the last description before a new one is asked for; fewer is **no model call** | — | now |
| `tasks.turnSummary`, `tasks.stepLabel` | names kept for later tasks | — | reserved, no PR yet: refused |
| `ui.showDescriptions` | boolean, `true`; whether the web shows a thread's description (the API returns it either way) | — | now, served by [`GET /api/config`](#get-apiconfig); the web reads it (PR S19) |

### `threadTools`, `mcp`, `webhooks`, `auth`, `artifacts`

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `threadTools.url` | `http(s)` base URL; both or neither with `secret` | `THREAD_TOOLS_URL` | now |
| `threadTools.secret` | **secret**, ≥ 32 bytes | `THREAD_TOOLS_SECRET` | now |
| `threadTools.previousSecret` | **secret**, ≥ 32 bytes, not the current one | `THREAD_TOOLS_SECRET_PREVIOUS` | now |
| `threadTools.tokenTtlSecs` | 60 to 86400, `7200` | `THREAD_TOOLS_TOKEN_TTL_SECS` | now |
| `threadTools.allowedHosts` | list of `host[:port]`, the host of `url` | `THREAD_TOOLS_ALLOWED_HOSTS` | now |
| `mcp.tokensFile` | path; required when `mcp` is mounted. Its format is `{user, tokenEnv, role?}`: the optional `role` is one of `auth.roles` (the built-in `user` and `admin` without them); without one the token has the default role | `MCP_TOKENS_FILE` | now; `role`: S15, ADR 0033 |
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
| `auth.jwt.rolesClaim` | a dotted path (`realm_access.roles`, `groups`), none (no roles) | — | now; read into `Principal.roles`, which `auth.roles` maps to permissions (S15) |
| `auth.roles` | map from a role name to `{ permissions, scope?, agents? }` ([Roles and permissions](#roles-and-permissions)); absent: the built-in `user` and `admin`; given: it replaces both, and at least one role | — | now (S15) |
| `auth.roles.<role>.permissions` | list of `agent.read`, `agent.invoke`, `thread.read`, `thread.write`, `artifact.read`, `admin`; required, may be empty (a role that is known and grants nothing) | — | now (S15) |
| `auth.roles.<role>.scope` | `own` \| `any` \| `{ read: own\|any, write: own\|any }`, `own`; only with a role that holds `thread.read`, `thread.write` or `artifact.read` | — | now (S15) |
| `auth.roles.<role>.agents` | list of agent ids and/or `"*"`, `["*"]`; only with a role that holds `agent.read` or `agent.invoke`; not empty | — | now (S15) |
| `auth.defaultRole` | a role of `auth.roles`, or `null` for none. Absent: `user` when `auth.roles` is absent (the built-ins, so a deployment that configures nothing is as it was), `null` when `auth.roles` is given. The one key where `null` is a value | — | now (S15) |
| `artifacts` | the artifact store ([ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md)). Absent: no store, and a file an agent hands over is refused with "no artifact store configured". Present: `store` is required | — | now (S10) |
| `artifacts.store` | `fs` \| `s3`; required with the section. Names only what this build compiled in: `fs` needs the Cargo feature `artifacts-fs`, `s3` needs `artifacts-s3`, else exit 78 naming it. The section of the store chosen is required and the other one is an error | — | now |
| `artifacts.fs.root` | path (relative to this file's directory); required with `store: fs`. Made (mode `0700`) when missing. Every role must see the same directory (one machine, or a shared volume) | — | now |
| `artifacts.s3.bucket` | 3 to 63 characters of `a-z`, `0-9`, `-`, `.`, starting and ending with a letter or a digit; required with `store: s3`; the bucket must exist | — | now |
| `artifacts.s3.region` | slug, `us-east-1` | — | now |
| `artifacts.s3.endpoint` | `http(s)` URL with a host, no credentials, query or fragment; absent: AWS S3. The bucket is then in the path (`<endpoint>/<bucket>/<key>`). An `http` endpoint sends the files in the clear | — | now |
| `artifacts.s3.prefix` | `a-z A-Z 0-9 . _ - /`, no `..`, at most 128 characters; keys go under it, so one bucket serves several deployments | — | now |
| `artifacts.s3.accessKeyId`, `artifacts.s3.secretAccessKey` | **secrets**, required with `store: s3`. Static credentials only: this build does not read `AWS_*` variables or an instance profile | — | now |
| `artifacts.s3.timeoutSecs` | 1 to 3600, `60` (one request) | — | now |
| `artifacts.maxFileBytes` | 1 to 268435456 (256 MiB), `10485760` (10 MiB). Read by the ingest (ADR 0032, S11): a larger file is not kept; the agent's artifact is logged without it, with an error "the file is too large to keep" | — | now |
| `artifacts.maxPerJobBytes` | 1 to 4294967296 (4 GiB), `104857600` (100 MiB). The bytes of files one job (one run of an agent) keeps; a job also keeps at most 50 files (not a key). A file over either is refused like one over `maxFileBytes` | — | now (S11) |
| `artifacts.fetchHosts` | list of hosts (`files.example.com`, `10.0.0.5:8080`: a host name or address with or without a port, which without one is the scheme's default, 80 or 443; no scheme, path, wildcard or credentials), default none. A `url` part of an agent's artifact on one of them is fetched by the worker and kept like a `raw` part; any other `url` stays a link. The list is the SSRF control: a host on it is trusted; the fetch is `http(s)` only, follows no redirect, sends no credential and stops at `maxFileBytes` | — | now (S11) |

### `toolServers`

*Built 2026-10-02 ([ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md), slice 8).* The MCP servers a person may
attach to a conversation, **the deployment's own list**: a person cannot enter a URL. A list of at most 64 servers, in the
order the web shows them; absent or empty, nothing is attachable. The orchestrator will call a server on the agent's behalf
and holds its credentials ([the relay](thread-tools-v1.md#attached-servers-and-the-relay-slice-8), not built yet); the
application, the API and the log hold only the part that is not secret. Replaces nothing: it has no variable.

| Key | Type, default | Replaces | When |
|---|---|---|---|
| `toolServers[].id` | `a-z`, `0-9`, `-`, starting with a letter or a digit, 1 to 31 characters, **unique**; required. What a thread records and the prefix of the server's tools on the thread's endpoint (`<id>__<tool>`): no `_`, so the first `__` is the split | — | now |
| `toolServers[].name` | 1 to 80 characters of one line, not blank; required | — | now |
| `toolServers[].description` | at most 500 characters of one line, none. Shown by the picker, and told to the agent (`attached` of its message) | — | now |
| `toolServers[].url` | `http(s)` URL with a host, **no user name, password, query or fragment** (a credential goes in `bearer` or `headers`); required. An `http` URL with a credential logs a warning at startup, naming the server | — | now |
| `toolServers[].icon` | a `data:` URI, `data:image/(svg+xml\|png\|webp);base64,…`, at most 8 KiB, none. The screen draws it as it is: an icon at a URL is never fetched (open question 38), and the icons the server offers itself are dropped | — | now |
| `toolServers[].bearer` | **secret**, none; sent as `Authorization: Bearer <value>` on the orchestrator's own requests | — | now |
| `toolServers[].headers` | map from a header name to a **secret**, none. A name is letters, digits and `-`; `Authorization` (use `bearer`), `Accept`, `Content-Type`, `Host`, `Mcp-Session-Id`, `Mcp-Protocol-Version` and `Last-Event-ID` are refused, whatever their case, and a name twice that differs only by case is too. A resolved value must be one a header can hold (visible ASCII, no line break) | — | now |
| `toolServers[].tools` | list of the server's own tool names, at least one, none (every tool the relay can expose). Each is `a-z A-Z 0-9 _ -`, not starting with `_`, and `<id>__<tool>` is at most 64 characters | — | now |
| `toolServers[].agents` | list of agent ids, at least one, none (every agent). The agents the server may be attached for: a thread whose agent is not listed cannot attach it (422). With no platform registry the ids must be agents of the agents file (exit 78 otherwise); with one they cannot all be known at startup, and an id that matches no agent is a server nobody is offered | — | now |
| `toolServers[].timeoutSecs` | 1 to 600, `120`; the longest one call may take | — | now |

A server's `bearer` and each of its `headers` are two of the **twelve** secrets of the contract (the ten of ADR 0032 and the
two here): a reference, resolved at startup, with an error that names the key and the variable or path and never a value, and
printed by `--print-config` as the reference. The public part of each server (`id`, `name`, `description`, `icon`, `tools`,
`agents`, the timeout) is what reaches the application; `GET /api/tool-servers` shows the first four and `agents`, never the
URL, a header or a credential. A server the deployment stops listing stays attached to the threads that have it (the thread keeps
its attachments); it is no longer told to their agents, and a person can detach it.

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
- **A worked example** is the dev stack (S16): [`dev/orchestrator.yaml`](../../dev/orchestrator.yaml) runs `jwt` against a mock issuer
  (`issuer: http://mock-oidc:8080`, `audiences: [dev-chat]`, `rolesClaim: roles`, `server.environment: development` because the issuer is plain http) behind a
  real oauth2-proxy, with the roles `user`, `admin` and `chat-only` ([`dev/README.md`](../../dev/README.md#sign-in-a-mock-issuer-and-oauth2-proxy)).

### Roles and permissions

([ADR 0033](../decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md#4-roles-and-permissions), built by S15.)
The roles of a request are the ones its credential carries: the token's `auth.jwt.rolesClaim`, or the `role` of an
MCP token's entry. The proxy header carries none. `auth.roles` says what each role grants; a role it does not name
grants nothing (the names are compared exactly: `Admin` is not `admin`). A person none of whose roles is named gets
`auth.defaultRole`, and a person with nothing at all is refused (403) by every operation but
[`GET /api/me`](chat-api.yaml), which says why.

```yaml
auth:
  defaultRole: user
  roles:
    user:  { permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read], scope: own, agents: ["*"] }
    admin: { permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read, admin], scope: { read: any, write: own }, agents: ["*"] }
```

That is what an absent `auth.roles` means (the built-in roles). A deployment that wants Keycloak groups to decide, and
nobody else in:

```yaml
auth:
  jwt: { issuer: https://idp.example/realms/main, audiences: [oauth2-proxy-client-id], rolesClaim: groups }
  roles:
    chat-users:  { permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read], agents: [chat, researcher] }
    chat-admins: { permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read, admin], scope: { read: any }, agents: ["*"] }
  # no defaultRole: a token with neither group is refused (403)
```

| Permission | What it lets the person do |
|---|---|
| `agent.read` | See an agent in `GET /api/agents`, read its AG-UI capabilities, see `GET /api/registry`. Limited to the role's `agents` |
| `agent.invoke` | Start a thread on an agent, send a message to a thread on it (a fork too). Limited to the role's `agents` |
| `thread.read` | Read a thread: `GET /api/threads/{id}`, its export, its branches, its AG-UI stream. `scope.read` says whose |
| `thread.write` | Start a thread, send, answer, cancel, rename, describe, fork. `scope.write` says whose |
| `artifact.read` | Download the files of a thread. `scope.read` says whose (it is its own permission: `thread.read` alone does not give files) |
| `admin` | `GET /api/threads?owner=<e-mail>` and `?owner=*`: other people's threads, or everyone's, which also takes `thread.read` of scope `any` |

- **Own and any.** `own` is the threads the person owns (the owner is the e-mail, [ADR 0033](../decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md#2-the-jwt-authenticator-orch-auth-jwt-authmode-jwt)),
  `any` is every thread. **Reading is not acting**: the administrator has `scope: { read: any, write: own }`, so
  they read every thread and change only their own (owner decision 4 of plan 10). `scope: any` writes everywhere,
  which no built-in role does.
- **What a person gets** when a request is not theirs to make: **404** for a thread they may not read (the answer for one
  that does not exist, so existence never leaks), **403 `read_only`** for a thread they may read and not change, **403
  `forbidden`** for a permission their roles lack (the same for every id, so it says nothing of what exists) and for an
  agent their roles do not name, **403 `no_access`** when their roles grant nothing.
- **Several roles** are unioned, each judged alone: a person with a role that holds `thread.read` and another that holds
  `agent.invoke` for `coder` has both, and may invoke `coder` and no other agent (a role's `agents` limit only the agent
  permissions that role holds).
- **Rules** (exit 78, each naming its key): `auth.defaultRole` is one of the roles; a role name has no space around
  it; no permission is listed twice; a `scope` or `agents` that its role would ignore is an error, and so is an empty
  `agents`; `auth.roles: {}` is an error (leave the key out for the built-ins). An agent id that no agent has matches
  nothing (the roles are read before the agents are). A `role` of the MCP tokens file that `auth.roles` does not define
  is exit 78 too: a typo would otherwise hand the token the default role.
- **A stream lasts as long as the token it was opened with**: an AG-UI connect or run stream is ended at the token's
  `exp` plus 60 s, and after an hour at most; the client reconnects with `Last-Event-ID`. A credential that does not run
  out (the proxy header, an MCP token) does not bound a stream.
- **The proxy header** carries no roles, so every request of `auth.mode: proxy_header` has the default role: with the
  built-ins, everybody is a `user`, as before roles existed. To have an administrator in that mode, name `admin` the
  default role (everybody is one: for one person on a local machine) or use `jwt`.

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

**Built (PR S18, 2026-10-02); the web reads it (PR S19, 2026-10-02).** The public subset of the configuration, for the web.

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
- It is `getConfig` in [`chat-api.yaml`](chat-api.yaml), served by the resource API of every role that serves routes, and
  the contract test of the API checks it (200 with exactly `{ "ui": … }`, 401 without an identity). A deployment with no
  `ui` section serves the defaults.
</content>
</invoke>
