# orchestrator (binary)

The orchestrator service: the composition root that wires the Postgres store,
the A2A adapter (and, with the feature `agent-local`, the local agents), the dispatcher, the inbox worker, the resource API and the interaction surfaces
chosen by `ORCH_SURFACES` into one stateless process. `ORCH_ROLE` says which
halves the process runs: the control plane, a worker, or both
([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)).

## Where it sits

The only crate that depends on every adapter, and the only place that chooses
implementations
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)):
[`orch-store-postgres`](../../crates/store-postgres/README.md) for
`ThreadStore` and `Wakeup`, [`orch-agent-a2a`](../../crates/agent-a2a/README.md)
for `AgentClient` (and, with the feature `agent-local`, [`orch-agent-adam`](../../crates/agent-adam/README.md) for
in-process agents, routed by transport with `ByTransport`), [`orch-artifacts-fs`](../../crates/artifacts-fs/README.md) and
[`orch-artifacts-s3`](../../crates/artifacts-s3/README.md) for `ArtifactStore` (the files agents hand over; chosen by `artifacts.store`,
[ADR 0032](../../../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md)), the system clock and UUIDv7 ids. It has no logic of its own:
what the service does lives in [`orch-app`](../../crates/app/README.md) and
[`orch-api`](../../crates/api/README.md) and the surface crates
([`orch-surface-agui`](../../crates/surface-agui/README.md), [`orch-surface-mcp`](../../crates/surface-mcp/README.md)). Processes are stateless; the only
persistence is Postgres
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
The role enum (`Role`) and the supervisor (`Host`) are not ours: they come from
the `adam-host` crate of [adam-rs](https://github.com/vymalo/another-adam-rs),
a git dependency pinned to a full commit sha in `orchestrator/Cargo.toml`
(ADR 0015, decision 4); a PR bumps it. Without the feature `agent-local` it brings two crates into the
dependency tree, `adam-host` and `adam-error`, and nothing else from adam-rs: `cargo tree -p orchestrator -i adam-runtime`
finds nothing.
The configuration file is defined and validated by the pure crate [`orch-config`](../../crates/config/README.md); this
binary reads the file, the environment and the secret files and builds its own `Config` from them
([below](#the-configuration-file)). Configuration is the composition root's input, not a port
([ADR 0034](../../../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md), decision 5).
Running it, the container image, configuration and shutdown are documented in
[`orchestrator/README.md`](../../README.md); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## What is in it

| File | What |
|---|---|
| `src/main.rs` | the command line (clap), `--print-config`, tracing setup, signals (SIGTERM, SIGINT), sysexits-style exit codes (`78` configuration, `69` database unavailable, `71` listen address, `70` a component of the service stopped, ended early or panicked (`adam_host::HostError`), `1` otherwise) |
| `src/model.rs` | `ConfiguredModel`, the one `ChatModel` type of the binary's `PortSet`: `Off(NoModel)` or `OpenAi(OpenAiChat)` (from `orch-model-openai`), built from `Config.models` (`ConfiguredModel::build`): `OpenAiChat` over **every** endpoint of `models.endpoints`, each with its own base URL, key and timeout, and one startup log line per task that is on (the task, its endpoint, URL, model and tokens, and whether its guidance is the core's or configured); `Off` when no task is configured. Static dispatch over two variants, so tasks on and off are the same build |
| `src/artifacts.rs` | the one place that knows the two artifact stores ([ADR 0032](../../../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md)): `ArtifactSettings { store: StoreSettings::{Fs { root }, S3(S3Settings)}, max_file_bytes, max_per_job_bytes, fetch_hosts }` (what `artifacts` of the file says: `config.rs` turns the limits into `AppConfig.files`, the ingest's, and `boot.rs` turns `fetch_hosts` and `max_file_bytes` into `A2aConfig.fetch_files` when the list is not empty; `S3Settings`' `Debug` shows no credential) and `ConfiguredArtifacts`, the one `ArtifactStore` type of the binary's `PortSet`: `Off(NoArtifacts)`, `Fs(FsArtifacts)` with the feature `artifacts-fs`, `S3(S3Artifacts)` with `artifacts-s3`. `ConfiguredArtifacts::build(settings)` makes the directory and checks it can be written (a root that cannot be used stops startup, exit 78, naming `artifacts.fs.root`), builds the S3 store **without connecting** (a control plane does not wait for a bucket), and logs the store (the directory, or the bucket, region and endpoint; never a credential) and a warning for an `http://` endpoint. Static dispatch over the variants, so a deployment with a directory, one with a bucket and one with none are the same build. Every role builds it: a worker keeps the files, the control plane serves them. **Nothing uses the store yet** (the ingest of S11 does): it is only built and handed to the bundle |
| `src/config.rs` | `Args` (clap derive: a flag per setting, falling back to its environment variable), `Config`, `ModelsSettings` (`endpoints`: name to `EndpointSettings` (base URL, the key as a `SecretString`, the timeout); `tasks`: `TaskKind` to `orch_app::TaskSettings`; both empty when no task is configured, redacted in `Debug`; from a file they are the file's `models.endpoints` and `tasks`, with each prompt read, and from the environment alone the endpoint `default` the legacy `ORCH_MODEL_*` variables make with the title task `ORCH_TITLE_MODEL` names; `Config::app_config` carries the tasks and `Config.public`, the `ui` section `GET /api/config` serves, into `AppConfig`), `RegistrySettings` (the platform registry: the URL, the two tokens as `SecretString`s, the timeout and the longest a copy stays fresh; `None` when `AGENT_REGISTRY_URL` is unset, redacted in `Debug`, with the URL's query left out; `AGENTS_FILE` may then be unset or empty), `AuthSettings` (`mode`, `jwt`, and `policy`: the `orch_app::Policy` built from `auth.roles` and `auth.defaultRole`, the built-in `user` and `admin` with `user` the default without a file; `Config::app_config` carries it into `AppConfig.policy`), `McpSettings` (the tokens, each with the roles of its entry, as `SecretString`s read through the `env` and `read` closures, the hosts and the public URL, when `mcp` is mounted on a role that serves HTTP), `Surface`, `LocalAgentKind` (the closed set of in-process agent kinds `transport: local` may name; `Echo` so far; compiled in with the feature `agent-local`; `needs_model()` says whether a kind calls a model), `LogFormat`, `ConfigError`. Clap only collects raw strings; `Config::load` validates them, reading the agent file and the `tokenEnv` variables through closures, so tests build `Args` by hand and never touch the process environment. Every problem names the variable, file or agent at fault, carries no secret and exits 78 |
| `src/config/file.rs` | the configuration file's loader (ADR 0034): `Loaded::from_process` / `Loaded::load` (the environment and the file system passed in as closures, so a test reads neither), the table `SETTINGS` of the 51 legacy variables and the key each became (and how each is read: `Text`, `Uint`, `Bool`, `List`, `Role`, `Surfaces`, `Gate`, `LogFormat`, `Secret`, `SecretList`), the overlay of the variables onto the tree, the projection of the valid file onto `Args` (so [`Config::load`] and its rules are the one source of what is valid), the legacy errors put in the file's words and scrubbed of anything quoted, and `Note`: what startup says about where a setting came from |
| `src/local.rs` | the one place that knows `orch-agent-adam`, in two variants of one surface. With the feature `agent-local`: `Local::start` builds the local agents' own pool on `DATABASE_URL` and migrates their journal (only when `AGENTS_FILE` lists a local agent), `compose` builds `ByTransport<A2aAgentClient, LocalAgentClient>`, `Local::register` adds the agents' worker as a worker component in the roles that run workers, `is_unavailable` maps a transient failure to exit 69. Without it: `Agents` is the A2A client alone and `Local` cannot be built |
| `src/boot.rs` | `run(cfg, shutdown)`: shared `setup` (pool, migrations, wakeup, A2A client, agent directory, `App`), then the components of `cfg.role`, registered with `adam_host::Host`: the HTTP server (`control_plane_router` plus `serve`: health, the resource API, the configured surfaces) as a control-plane component, the dispatcher and the inbox worker (timers and stored reports, `orch_app::InboxWorker`) as worker components, and for a worker-only process the health-only router ([`orch_api::health_router`](../../crates/api/README.md)) on `LISTEN_ADDR`. `Host` starts only what the role asks for, treats the first component to end on its own as fatal (exit `70`), and stops the control plane before the workers, each bounded by `SHUTDOWN_GRACE_SECS` and aborted after that (the dispatcher and the inbox worker release their leases as they stop). Readiness flips first, so probes answer 503 for the whole drain |

## Environment

Each is also a flag (`--database-url`, `--listen-addr`, `--surfaces`, and so on; `orchestrator --help`), and a flag wins over its variable.

**Deprecated in this release** ([ADR 0034](../../../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md)):
each variable below is a key of the [configuration file](#the-configuration-file) now (the key each one became is in
[`docs/api/config.md`](../../../docs/api/config.md#every-key)). It still works, over the file, and logs a warning when it is set; the
next release removes them, except `ORCH_CONFIG_FILE`, `ORCH_ROLE`, `ORCH_INSTANCE_ID`, `RUST_LOG`, `HOSTNAME` and the variables
that hold secrets.

| Variable | Default | |
|---|---|---|
| `ORCH_CONFIG_FILE` | unset | the configuration file (flag `--config`): YAML, `version: 1`, read once at startup ([below](#the-configuration-file)). Unset: the environment alone configures the process, as before, with one warning. Kept for good |
| `DATABASE_URL` | required | Postgres connection string, never logged |
| `AGENTS_FILE` | required, unless `AGENT_REGISTRY_URL` is set | YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?, gate?}` ([`agents.example.yaml`](../../agents.example.yaml)); `gate` is the entry's verification gate, `{require?, maxAttempts?, verifier?, ci?: {required?, timeoutSecs?}}` with `deny_unknown_fields` (see [The gate](#the-verification-gate)); `transport` is `a2a` (the default when absent; `cardUrl` required) or `local` (`agent` required, names a `LocalAgentKind`; `cardUrl` and `tokenEnv` refused). `local` is refused with `LocalAgentsNotCompiled` (78) unless the build has the Cargo feature `agent-local`; any other `transport` is a startup error |
| `AGENT_REGISTRY_URL` | unset | the platform's agent registry, `agent-registry/v1` ([ADR 0022](../../../docs/decisions/0022-platform-provisions-agents-system-discovers-them.md), [`orch-registry-platform`](../../crates/registry-platform/README.md)): the full URL of the document, `http` or `https` with a host and no user name or password (flag `--registry-url`). The agents it lists are read live, beside the `AGENTS_FILE` agents (which come first, win on an id both list and are the default agent); a registry that cannot be read leaves only those, and `GET /api/registry` says so. With it set, `AGENTS_FILE` may be unset or list no agent. Needs the Cargo feature `registry-platform`: without it the URL is refused at startup (78), never ignored. The registry is never read into the database: the copy lives in this process only |
| `AGENT_REGISTRY_TOKEN` | unset | the bearer token sent to the registry (flag `--registry-token`). Never logged; a warning when it goes over `http://` |
| `AGENT_REGISTRY_AGENT_TOKEN` | unset | the bearer token sent to **every agent the registry lists** (flag `--registry-agent-token`): one deployment-wide credential until authentication to the agents is decided (open question 11). Never logged |
| `AGENT_REGISTRY_TIMEOUT_SECS` | `3` | 1 to 60 (flag `--registry-timeout-secs`); how long one read of the registry may take. A slower one is down: its agents are not listed. Checked whether or not a URL is set |
| `AGENT_REGISTRY_MAX_AGE_SECS` | `60` | 1 to 3600 (flag `--registry-max-age-secs`); the longest a copy of the document is kept in this process, whatever the registry's `Cache-Control` allows. Checked whether or not a URL is set |
| `LISTEN_ADDR` | `0.0.0.0:8080` | control plane: the API; worker: the probes only |
| `ORCH_ROLE` | `all` | `all`, `control-plane` or `worker` (`--role`); see [Roles](#roles). Unknown is a startup error (78) |
| `AUTH_DEV_USER` | unset | e-mail served for requests without `X-Auth-Request-Email`; development only, logs a warning; the file refuses it unless `auth.mode` is `proxy_header` |
| `DATABASE_MAX_CONNECTIONS` | `10` | at least 2 |
| `DISPATCHER_CONCURRENCY` | `32` | |
| `OUTBOX_LEASE_SECS` | `30` | also the lease of a local agent's run |
| `ORCH_VERIFIER_WATCH_SECS` | `5` | at least 1 (`--verifier-watch-secs`); how often a verification looks at its thread while it waits for the verifier. When the thread has moved on (the deadline held it, it was cancelled, the user wrote) the verifier is told to stop and the row ends: this is how late |
| `ORCH_STEPS_RECORD_IO` | `true` | `true`/`false` (also `1`/`0`, `yes`/`no`, `on`/`off`; `--steps-record-io`); whether a step's input and output (what a tool was called with and what it returned) are recorded in the log, redacted and capped at 4 KiB in, 8 KiB out and 2 MiB per job ([ADR 0030](../../../docs/decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)). `false` keeps a step to its label and detail; a value that is neither is a startup error (78). It will be the configuration key `steps.recordToolIo` when the YAML configuration exists |
| `ORCH_ASK_MAX_DEPTH` | `2` | 1 to 4 (`--ask-max-depth`; `asks.maxDepth`); how deep a chain of asked agents goes: the addressed agent asks A (depth 1), A may ask B (depth 2), and B cannot ask ([`ask_agent`](../../../docs/api/thread-tools-v1.md#ask_agent)) |
| `ORCH_ASK_MAX_PER_JOB` | `16` | 1 to 64 (`--ask-max-per-job`; `asks.maxPerJob`); the asks one job may make, those that ended included |
| `ORCH_ASK_MAX_RUNNING` | `4` | 1 to 16 (`--ask-max-running`; `asks.maxRunning`); the asks of one thread that may run at once |
| `ORCH_ASK_TIMEOUT_SECS` | `1800` | 10 to 7200 (`--ask-timeout-secs`; `asks.timeoutSecs`); an ask that runs this long ends `timed_out` and the asked agent is told to stop; a call may ask for less, never for more |
| `ORCH_TITLE_MODEL` | unset | the model that writes thread titles (`--title-model`; **deprecated: `tasks.title.model`, [`config.md`](../../../docs/api/config.md#steps-models-tasks-ui), which also has a model, endpoint, guidance and language rule per task and the description task**; [ADR 0005](../../../docs/decisions/0005-openai-compatible-model-endpoint.md)): after the agent's first reply it is asked for a 3 to 6 word title, which replaces the first words of the first message unless a person renamed the thread. **Unset turns titles off** (and `ORCH_MODEL_*` are not read). Set, it needs `ORCH_MODEL_BASE_URL` (exit 78 otherwise) |
| `ORCH_MODEL_BASE_URL` | unset | the OpenAI-compatible endpoint the title model is asked at (`--model-base-url`), up to and not including `/chat/completions` (`https://api.openai.com/v1`; a trailing slash is cut); must start with `http://` or `https://` (exit 78) |
| `ORCH_MODEL_API_KEY` | unset | the bearer token that endpoint wants, when it wants one (`--model-api-key`). A sensitive header, never logged, in no error and not in `Debug` |
| `ORCH_MODEL_TIMEOUT_SECS` | `20` | at least 1 (`--model-timeout-secs`); how long one question to the model may take. A model that is slow, down or says nothing usable costs the thread nothing: it keeps the first message's words |
| `INBOX_LEASE_SECS` | `30` | at least 3; how long a crashed replica's claim on an inbox row blocks others |
| `INBOX_POLL_SECS` | `2` | at least 1; the inbox worker's safety poll, and so the latest a timer fires after its time |
| `INBOX_PARKED_TTL_SECS` | `86400` | at least 1; how long a report that no thread watches yet waits before it expires |
| `INBOX_MAX_ATTEMPTS` | `10` | at least 1; claims of one inbox row before it is dead-lettered (a row claimed more often without being finished is dead-lettered undelivered; claims handed back at shutdown or ended by a park are not counted) |
| `AGENT_LOCAL_CONCURRENCY` | `4` | only with the feature `agent-local`: runs of local agents stepped at once (at least 1); the local agents' pool is this plus 4 connections |
| `SHUTDOWN_GRACE_SECS` | `15` | |
| `ORCH_SURFACES` | `agui` | comma-separated surfaces to mount (`--surfaces`), as far as the build has them: `agui`, `mcp`, `thread-tools`, `webhook-generic`, `webhook-github`; unknown, empty, repeated or not compiled in is a startup error, and so is the removed `chat-api` (see [Surfaces](#surfaces)) |
| `WEBHOOK_GENERIC_SECRETS` | none | one or two comma-separated shared secrets of `POST /webhooks/ci` (`--webhook-generic-secrets`; a signature by either is good, so a secret can be rotated). **Required when a role that serves routes (`all`, `control-plane`) mounts `webhook-generic`** (exit 78); a third secret, or one under 32 bytes, is a startup error; never logged, and hidden in `--help` |
| `WEBHOOK_GITHUB_SECRETS` | none | one or two comma-separated secrets of `POST /webhooks/github` (`--webhook-github-secrets`); **required when a role that serves routes mounts `webhook-github`** (exit 78), a third secret is a startup error; never logged, hidden in `--help` |
| `WEBHOOK_GENERIC_MAX_SKEW_SECS` | `300` | at least 1; how far `X-Vymalo-Timestamp` may be from the clock, either way (`--webhook-generic-max-skew-secs`) |
| `WEBHOOK_GITHUB_MAX_AGE_SECS` | `86400` | at least 1; how old the signed `completed_at` (`updated_at` for a workflow run) of a GitHub event may be; an older event is acknowledged (202) and not stored (`--webhook-github-max-age-secs`) |
| `ORCH_CI_REQUIRED` | none | comma-separated names of the CI checks that must pass, the deployment's `ci.required` (`--ci-required`); **a gate that requires `ci` must name at least one** (here or in an entry's `gate.ci.required`), else exit 78; `ci` is refused (78) when no CI webhook surface is mounted |
| `ORCH_CI_TIMEOUT_SECS` | `3600` | at least 1; how long a job waits for the CI reports its gate needs before it is blocked with `ci_timeout` (no attempt is used); an `AGENTS_FILE` entry's `gate.ci.timeoutSecs` overrides it (`--ci-timeout-secs`) |
| `ORCH_GATE` | none | sources every job must pass before it is `done`, a comma list of `ci`, `agent-checks`, `verifier` (`--gate`). Empty is no gate: an agent that completes is done. **`ci` and `agent-checks` are accepted by this build** (`ci` since slice 6; it needs reports, so mount `webhook-generic`); `verifier` is a startup error (78) naming the slice that enables it |
| `ORCH_GATE` | none | sources every job must pass before it is `done`, a comma list of `ci`, `agent-checks`, `verifier` (`--gate`). Empty is no gate: an agent that completes is done. **All three are accepted by this build** (`ci` since slice 6, it needs reports, so mount `webhook-generic`; `verifier` since slice 10). `verifier` needs a verifier agent (`ORCH_VERIFIER`, or `gate.verifier` in an entry) |
| `ORCH_MAX_ATTEMPTS` | `3` | attempts a gated job's agent gets, the first included (`--max-attempts`); at least 1 and at most the cap |
| `ORCH_MAX_ATTEMPTS_CAP` | `10` | the most an `AGENTS_FILE` entry or a run may set the attempts to (`--max-attempts-cap`); at most `100`. When only the cap is set below `3`, the default attempts are lowered to it; an explicit `ORCH_MAX_ATTEMPTS` above the cap is a startup error |
| `ORCH_VERIFIER` | none | the verifier agent's id (`--verifier`): another configured agent than the ones it verifies (startup error otherwise, naming the agent and how to fix it). Used when the gate requires `verifier` |
| `ORCH_VERIFIER_TIMEOUT_SECS` | `1800` | how long a verification may wait for the verifier's verdict before the thread waits for the user (`--verifier-timeout-secs`); at least 1. Waiting does not use an attempt |
| `THREAD_TOOLS_SECRET` | none | the HMAC key of the thread-tools tokens, at least 32 bytes (`--thread-tools-secret`; `openssl rand -hex 32`). With `THREAD_TOOLS_URL` it makes the A2A adapter give a grant to every agent whose card lists `thread-tools/v1`, and it opens the surface `thread-tools`. **Both or neither** (a half is exit 78), and **in every role** when `thread-tools` is in `ORCH_SURFACES`: a worker mounts no route, but it mints; a process that does not name the surface still reads and checks them when they are set. A `SecretString`, never logged, hidden in `--help` |
| `THREAD_TOOLS_SECRET_PREVIOUS` | none | the previous key, verifying only (`--thread-tools-secret-previous`): for a rotation, move the current key here and set a new one, then remove it once the longest token lifetime has passed. At least 32 bytes, not the current key, and not without a current one (78) |
| `THREAD_TOOLS_URL` | none | the base URL under which agents reach this orchestrator (`--thread-tools-url`), `http` or `https`, a host, optionally a port and a path prefix, no credentials, query or fragment; the grant tells an agent `<URL>/thread-tools/<threadId>/mcp`. With the key, it is also what makes `boot.rs` give the A2A client an issuer (`A2aConfig.thread_tools`), in every role |
| `THREAD_TOOLS_TOKEN_TTL_SECS` | `7200` | how long a token lives (`--thread-tools-token-ttl-secs`), 60 to 86400 |
| `THREAD_TOOLS_ALLOWED_HOSTS` | the host of the URL | comma-separated `Host` values the surface accepts (`--thread-tools-allowed-hosts`), each `host` or `host:port` (a URL, `*` or a bad port is 78); the default is the host of `THREAD_TOOLS_URL`, with its port when the URL names one |
| `MCP_TOKENS_FILE` | required with `mcp` | YAML list of `{user, tokenEnv, role?}` (`--mcp-tokens-file`): who each bearer token is, and the optional `role` it has (one of `auth.roles`, the built-in `user` and `admin` without them: another is `Invalid`, 78; none means the default role; [ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)); read by the roles that serve HTTP only. A `tokenEnv` variable that is unset or empty is `McpTokenEnvMissing` and an unreadable file `McpTokensFileRead` (both 78) |
| `MCP_TOKEN_<NAME>` | required by the file | the variable a `tokenEnv` names: the bearer token (a `SecretString`, never logged), at least 32 bytes (shorter is `Invalid`, 78) |
| `MCP_ALLOWED_HOSTS` | required with `mcp` | comma-separated `Host` values the MCP server accepts (`--mcp-allowed-hosts`), each `host` or `host:port`: a URL, `*` or a port that is not a number is `Invalid` (78) |
| `MCP_WAIT_MAX_SECS` | `3600` | the largest `timeout_secs` of `wait_for_job` (`--mcp-wait-max-secs`), 1 to 86400, larger requests are cut to it |
| `MCP_WAIT_MAX_CONCURRENT`, `MCP_WAIT_MAX_PER_USER` | `256`, `16` | the most `wait_for_job` calls one process, and one user, may hold open (`--mcp-wait-max-concurrent`, `--mcp-wait-max-per-user`), at least 1 |
| `MCP_ALLOWED_ORIGINS` | none | comma-separated browser origins the MCP server lets through (`--mcp-allowed-origins`), each `http(s)://host[:port]`; a request with another `Origin` is 403 |
| `ORCH_PUBLIC_URL` | unset | the chat's public origin (`--public-url`), for the `web_url` of `start_job`; an origin with no path |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | names this replica in leases |
| `RUST_LOG`, `LOG_FORMAT` | `info,rmcp=warn`, `json` | `LOG_FORMAT=text` for humans; `RUST_LOG` replaces the default whole (the MCP library logs a line per request at `info`) |

### The configuration file

One YAML file replaces the variables above ([ADR 0034](../../../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md);
every key, the variable it replaces and the PR that builds it are in [`docs/api/config.md`](../../../docs/api/config.md); the JSON Schema
is [`docs/api/config.schema.json`](../../../docs/api/config.schema.json), which an editor can validate against). It is read once, at
startup: processes are stateless and restart cheaply, so there is no hot reload (open question 43).

```sh
ORCH_CONFIG_FILE=dev/orchestrator.yaml orchestrator            # or --config dev/orchestrator.yaml
orchestrator --config dev/orchestrator.yaml --print-config     # the merged configuration, secrets as references; exit 0 or 78
```

* **Secrets are references**, `{ env: NAME }` or `{ file: PATH }`, never a value; a plain string where a secret goes is an
  error. The variables and files are read at startup, and a reference that does not resolve is an error naming the key and the
  variable or path. A process that serves no routes (`worker`) is not asked for the secrets of the routes it would not mount.
* **Precedence**: a flag or its variable, then the file, then the default. Each variable that is set is read with its own
  parser first (`ORCH_STEPS_RECORD_IO=yes` is `true`), laid over the file, and logged once at startup: a `warn` naming the variable
  and the key (`ORCH_TITLE_MODEL overrides tasks.title.model of the configuration file`), never the value. A secret variable counts
  as a reference to itself, so a file that names `{ env: DATABASE_URL }` is not overridden by `DATABASE_URL`. `ORCH_ROLE` and
  `ORCH_INSTANCE_ID` are process overrides (one file serves a control plane and its workers): they win over `server.role` and
  `server.instanceId` and log at `info`, as do `RUST_LOG` and `HOSTNAME`, which have no key.
* **Three passes, then the rules the binary has always had** ([`orch-config`](../../crates/config/README.md#the-three-passes)):
  syntax (reported alone, by line and column); the variables over the tree; the shape against the JSON Schema (every violation,
  with reserved keys named by the PR and ADR that bring them); the rules between keys and the secrets. The valid file is then
  handed to [`Config::load`](src/config.rs), the same rules the variables go through (a gate against the agents, a surface this build
  compiled in, the agents file, the MCP tokens), so the file and the variables cannot disagree on what is valid. Any error is
  exit **78**, listed on stderr one per line, each naming a key path (a variable that does not parse names the variable) and
  **never a value**: a message of a library or of today's parsers that quotes the value it refused is scrubbed.
* **`--print-config`** prints the configuration this process would run with, the defaults filled in, and opens no connection (it
  reads the agents file and the secrets' variables, as a start does). The notes about variables go to stderr. With no file it
  prints what the environment alone says.
* `agents.file` and `mcp.tokensFile` (and a `{ file }` secret) are relative to the directory of the configuration file, also when a
  legacy variable gave them. The agents file and the MCP tokens file keep their formats.
* A build without a Cargo feature refuses the keys that need it, naming the feature (`server.surfaces` naming a surface that is not
  compiled in, `agents.registry` without `registry-platform`, `agents.localConcurrency` without `agent-local`, `artifacts.store`
  naming `fs` without `artifacts-fs` or `s3` without `artifacts-s3`).
* **`artifacts`** ([ADR 0032](../../../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md)) has no variable: it is a key of
  the file only. Without the section the store is `NoArtifacts` and a file is refused with "no artifact store configured"; the
  environment alone (no file) never has one. `artifacts.fs.root` is relative to the directory of the file; `accessKeyId` and
  `secretAccessKey` of `artifacts.s3` are secret references like every other.
* **`toolServers`** ([ADR 0024](../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md)) has no variable: it is a key of
  the file only, the MCP servers a person may attach to a conversation. What reaches the application (`Config.tool_servers`,
  `AppConfig.tool_servers`) is the public part of each server (id, name, description, icon, the `tools` allow-list, the `agents` it
  is for, the timeout); **the URL and the credentials are in `Config.tool_endpoints` only**, one `orch_ports::ToolServerEndpoint`
  per server (id, URL, timeout, the bearer and the header values resolved, as `ToolSecret`s), which the relay (feature `tool-relay`) reads and nothing else does, and which appear in no `Debug` (the configuration's lists the ids), event, answer or log line. A mistake is exit 78 naming the key, never a value; an `agents` id that is not an
  agent of the agents file is one too (checked only when the file's own list is the whole list, that is, with no platform registry);
  a server that has a credential and a plain `http://` URL logs a warning naming the server.
* The compose stack is configured this way:* The compose stack is configured this way: [`dev/orchestrator.yaml`](../../../dev/orchestrator.yaml) (and
  [`dev/orchestrator.live.yaml`](../../../dev/orchestrator.live.yaml) for `compose.live.yaml`). The `local-agent` profile stays on
  variables, which keeps the old path covered by a stack that runs.

### Authentication

`auth.mode` ([ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md), keys in
[`docs/api/config.md`](../../../docs/api/config.md#authentication)) has no variable: `proxy_header` (the default, nothing changes), `jwt`,
or `jwt_or_proxy_header`. `src/auth.rs` builds the one `ConfiguredAuth` of the binary's `PortSet` (static dispatch over
`RefuseAll`, the header authenticator, the JWT authenticator and `ByCredential` of both); the log says which at startup, and warns
that the header is trusted when the mode reads it. The configuration refuses, before anything connects: `auth.jwt` missing
(or present with `proxy_header`), `auth.devUser` with another mode, `proxy_header` when `server.environment` is `production`, an
issuer that is not an `http(s)` URL without credentials, query or fragment, no audience, and a mode whose feature is not compiled in. In
`jwt` mode `/readyz` is 503 until the issuer's keys have been fetched (the probe is what fetches them), and the first fetch that
fails is not repeated for 5 seconds.

### The verification gate

[ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md). Three layers (sources can only be added, attempts set anywhere
within the cap): the deployment (`ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_MAX_ATTEMPTS_CAP`, `ORCH_VERIFIER`, `ORCH_VERIFIER_TIMEOUT_SECS`), an agent's
`gate:` in `AGENTS_FILE`, and the run that creates a thread (`forwardedProps["vymalo.gate"]`, see
[`docs/api/agui.md`](../../../docs/api/agui.md#verification-the-gate)). The result is copied into the thread's job when
it is created, so a change of configuration never reaches a running job.

```yaml
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  gate:
    require: [agent-checks]   # the agent's own `checks` artifact must pass, or it is sent back
    maxAttempts: 2            # 1..=ORCH_MAX_ATTEMPTS_CAP
- id: builder
  name: Builder
  cardUrl: https://builder.example.com/.well-known/agent-card.json
  gate:
    require: [verifier]       # another agent reviews the pushed commit; its `verdict` artifact decides
    verifier: reviewer        # a configured agent (below), never this one
- id: reviewer
  name: Reviewer
  cardUrl: https://reviewer.example.com/.well-known/agent-card.json
  tokenEnv: REVIEWER_A2A_TOKEN   # its own credentials: it never receives the worker's
```

Startup validates all of it and exits **78** with a message naming the variable or the agent: an unknown source; a
`maxAttempts` outside `1..=cap`; an entry whose `require` leaves out a source the deployment requires; a `verifier`
that is not another configured agent; a gate that requires `verifier` with no verifier configured; an agent whose own
gate requires the verifier and that is the verifier itself (its entry must leave `verifier` out of its `require`, the one
removal a layer may make); an unknown member of `gate`. `ci` is honoured since slice 6 (the inbox and timers, slice 5,
and the CI webhook write and apply its reports): a deployment or an entry may require it and set `ci: {required, timeoutSecs}`; a run may add `ci` to `require`
but not set the `ci` settings. **A gate that requires `ci` names the checks that count** (`ci.required`, `ORCH_CI_REQUIRED` for the deployment): the process
exits 78 for a deployment or an entry that requires `ci` without a name, and a run that adds `ci` on a policy with none is a 400.
A process that serves routes and mounts neither `webhook-generic` nor `webhook-github` refuses `ci` in every layer (78 at
startup, 400 for a run) with "no CI webhook surface is mounted (ORCH_SURFACES)"; a `worker` serves no routes and cannot tell, so it
honours what the control plane decides. A source this build cannot honour would be refused in every layer, naming the slice
that enables it (none is left). A run that asks for the same is a 400.

**The verifier** (slice 10): when the worker completes under a gate that requires it, the dispatcher asks the verifier agent
over A2A, in a context of its own (`<thread>-verify-<attempt>-<verification>`), to review the commit the worker pushed, and
its `verdict` artifact `{passed, findings[]}` decides like any other source: findings send the worker back (quoted as
untrusted data), a pass counts toward done. No verdict is a failed check. A verifier that cannot be used (its task fails, it
cannot be reached, it does not answer within `ORCH_VERIFIER_TIMEOUT_SECS`) leaves the thread waiting for the user, `blocked`,
without spending an attempt. A thread may require the verifier in its run but not choose which agent it is.

### Sharing

A thread can be shared by a revocable link ([ADR 0040](../../../docs/decisions/0040-thread-sharing-by-revocable-link.md)). The configuration file's `sharing` section ([`docs/api/config.md`](../../../docs/api/config.md#sharing)) is the only way to turn it on: there is no environment variable. `mode` is `disabled` (the default: no key is read, no route serves a link, the share routes answer that sharing is off), `internal` or `public`; `secret` (and `previousSecret`, for a rotation) are references to secrets, required unless `disabled`, and the file is refused (exit 78, naming the key) when the secret is the thread tools' secret or a public option or a rate limit is set without `mode: public` or is out of range. `src/config/file.rs` (`sharing_of`) turns the section into `orch_app::SharingSettings` for the application and `orch_api::PublicLimits` for the public routes, and `src/boot.rs` passes them with `ApiConfig::check`, which refuses to start with `public` and no limiter. The roles' table in [`docs/api/config.md`](../../../docs/api/config.md#roles-and-permissions) lists `thread.share` (`user` and `admin`). No secret value reaches a log, an error or `--print-config`.

### Logs and metrics

Every log line carries the process's `role` and `instance` (the lease owner), so lines of several
replicas can be told apart in a collector. In JSON they are the first two keys of the object
(`{"role":"worker","instance":"w1","timestamp":...}`); in text the line starts
`role=worker instance=w1 `. They are added by the event formatter (`src/logging.rs`), not by a
span, so the dispatcher's spawned tasks have them too. The configuration is read before logging
starts: a line about an invalid configuration has neither (there is no role yet). While a
dispatcher processes an outbox row, its lines also sit in an `outbox` span (`id`, `thread`,
`kind`, `attempt`; JSON keys `span` and `spans`).

Every role serves `GET /metrics` on `LISTEN_ADDR` without an identity: the outbox queue as
Prometheus text, for dashboards and for scaling the workers (see
[`orch-api`](../../crates/api/README.md#get-metrics) for the samples, and
[`docs/orchestrator.md`](../../../docs/orchestrator.md#observability-and-scaling) for the KEDA
recipe). The edge proxy of the compose stack does not route it.

### Roles

`ORCH_ROLE` is `adam_host::Role`, a closed enum owned by adam-rs, so the names are the same for every
host (adam-coder reads its own `ROLE`). Every role runs the migrations, needs `DATABASE_URL` and
`AGENTS_FILE`, and binds `LISTEN_ADDR`.

| Role | Starts | Serves on `LISTEN_ADDR` | Ready when |
|---|---|---|---|
| `all` (default) | HTTP server, dispatcher and inbox worker, as before the role existed | health, resource API, surfaces | the database is migrated and answers |
| `control-plane` | HTTP server; **no dispatcher and no inbox worker**, so nothing is delivered to an agent and no timer fires from this process (a report received here is stored, and applied by a worker) | health, resource API, surfaces | the database is migrated and answers |
| `worker` | dispatcher, inbox worker, and a router with only `/healthz` and `/readyz` | health only: every other path is 404, with or without an identity | the database answers and the dispatcher has started |

The two halves share nothing but Postgres (outbox, thread version compare-and-swap, `LISTEN/NOTIFY`;
[ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)), so a control plane
and its workers may be any number of processes on any machines. A thread created through a control
plane stays `queued` until some worker runs; a worker that dies mid-task hands it over through the
outbox lease, as before. On shutdown the server drains before the dispatcher stops; a worker's probe
router outlives its dispatcher, so a draining worker answers 503, not connection refused.

The authoritative table, with the meaning of each variable, is in
[`orchestrator/README.md`](../../README.md#configuration). Keep the two in step.

*Unverified:* the sysexits.h numbers come from memory of the BSD header, as
noted in `src/main.rs`.

## Features

| Feature | Default | Compiles in |
|---|---|---|
| `surface-agui` | yes | [`orch-surface-agui`](../../crates/surface-agui/README.md), the surface name `agui` |
| `surface-mcp` | yes | [`orch-surface-mcp`](../../crates/surface-mcp/README.md), the surface name `mcp` ([ADR 0019](../../../docs/decisions/0019-mcp-server-over-streamable-http.md)). On by default like `surface-agui`, so the image has it; it is mounted only when `ORCH_SURFACES` names it |
| `surface-thread-tools` | yes | [`orch-surface-thread-tools`](../../crates/surface-thread-tools/README.md) (the surface name `thread-tools`) and [`orch-thread-token`](../../crates/thread-token/README.md) ([`docs/api/thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md)). On by default; mounted only when `ORCH_SURFACES` names it, and then `THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL` are required in every role. Without the feature those variables are not read and naming the surface is refused The endpoint also offers **`ask_agent`** ([`AskTools`](../../crates/surface-thread-tools/README.md#ask_agent), limits `asks.*` / `ORCH_ASK_*`), which has no feature of its own: it is part of the surface, and the A2A adapter is told it exists (`coordinate` in `mentions/v1`) in every role that mints grants |
| `tool-relay` | yes | [`orch-tools-mcp`](../../crates/tools-mcp/README.md) and the relay of [`orch-surface-thread-tools`](../../crates/surface-thread-tools/README.md): the tools of the MCP servers attached to a thread, called for the agent with the credentials of `toolServers`, each call a step with the server's icon ([ADR 0024](../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md)). Implies `surface-thread-tools`; on by default because it does nothing without `toolServers` in the configuration and the surface mounted. Without it, listed servers can still be attached and the process warns at startup that they give an agent no tools |
| `surface-webhook` | yes | [`orch-surface-webhook`](../../crates/surface-webhook/README.md), the surface names `webhook-generic` (`POST /webhooks/ci`) and `webhook-github` (`POST /webhooks/github`) |
| `auth-header` | yes | [`orch-auth-header`](../../crates/auth-header/README.md): the proxy-header authenticator, `auth.mode: proxy_header` (the default) and `jwt_or_proxy_header` ([ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)). Without it those modes are refused at startup (78, naming the feature); a process that serves no routes needs none. The tests of `--no-default-features` keep this feature, because the default mode needs it |
| `auth-jwt` | yes | [`orch-auth-jwt`](../../crates/auth-jwt/README.md): the OAuth2 resource server, `auth.mode: jwt` and `jwt_or_proxy_header` (`auth.jwt`: issuer, audiences, `jwksUrl`, `userClaim`, `rolesClaim`). Without it those modes are refused (78) |
| `registry-platform` | yes | [`orch-registry-platform`](../../crates/registry-platform/README.md): the platform's agent registry (`AGENT_REGISTRY_URL` and the variables that go with it). Used only when the URL is set; without the feature a URL is a startup error (78) |
| `artifacts-fs` | yes | [`orch-artifacts-fs`](../../crates/artifacts-fs/README.md): `artifacts.store: fs`, a directory. Without the feature the key is refused at startup (78), never ignored |
| `artifacts-s3` | yes | [`orch-artifacts-s3`](../../crates/artifacts-s3/README.md): `artifacts.store: s3`, a bucket, through the S3 backend of `object_store` (six crates more in the lock file, on the workspace's `reqwest` and `aws-lc-rs`, so still one TLS stack). On by default so that the image can be configured for production with no other build; a build that does not need it drops the feature, and a file that names `s3` is then refused (78) |
| `agent-local` | **no** | [`orch-agent-adam`](../../crates/agent-adam/README.md) and the adam-rs runtime: `transport: local` agents run in this process ([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)); the variable `AGENT_LOCAL_CONCURRENCY` |

The store features are independent of the rest and of each other: the binary builds and lints (`cargo clippy -p orchestrator --bins --locked -- -D warnings`) with both, with neither (`--no-default-features`), and with only one (`--no-default-features --features artifacts-fs`, or `artifacts-s3`); CI runs the default build and `--no-default-features`.

The feature decides what *can* be mounted, `ORCH_SURFACES` what *is*: a surface
named but not compiled in stops startup with an error naming its feature. The
binary is `orchestrator` (`cargo run -p orchestrator`).

**Local agents (`agent-local`).** An `AGENTS_FILE` entry with `transport: local` needs a build with
`cargo run -p orchestrator --features agent-local`; without it the entry is refused at startup (78). With it and at
least one such entry, every role opens a pool of its own on `DATABASE_URL` (`AGENT_LOCAL_CONCURRENCY + 4`
connections, on top of `DATABASE_MAX_CONNECTIONS`) and creates the journal's tables, `orch_agent_runs`,
`orch_agent_journal` and `orch_agent_meta` (prefix `orch_agent_`), after the orchestrator's own migrations; both are
idempotent. The `worker` and `all` roles register the agents' worker (and its `NOTIFY` listener on the channels
`orch_agent_events` and `orch_agent_signals`) as a worker component next to the dispatcher; the `control-plane` role
runs neither and steps nothing. A transient failure of the local agents' database at boot exits `69`.

## Surfaces

The resource API (`GET /api/agents`, `GET /api/threads`, `GET /api/threads/{id}`,
`GET /api/threads/{id}/export`, `POST /api/threads/{id}/cancel`) and health are `orch-api`'s and are mounted whatever
`ORCH_SURFACES` says. The interaction surfaces are chosen by it:

| `ORCH_SURFACES` | Serves |
|---|---|
| unset, or `agui` (the default) | the AG-UI routes: `POST /agui/agents/{agentId}`, `GET /agui/threads/{threadId}/connect`, `GET /agui/agents/{agentId}/capabilities` |
| `agui,mcp` | and the MCP server at `/mcp`: a **machine route** outside the identity layer, guarded by `Authorization: Bearer <token>`; needs `MCP_TOKENS_FILE` and `MCP_ALLOWED_HOSTS` (a missing piece is exit 78 before anything connects) |
| `agui,thread-tools` (with `THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL`) | and the per-thread MCP endpoint `/thread-tools/{threadId}/mcp` ([`docs/api/thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md)): a **machine route** outside the identity layer, guarded by the HMAC token the A2A adapter mints for an agent that lists `thread-tools/v1` (a bad, expired or foreign token is one `401`; no route is served for a thread that does not exist or is another agent's); the tools are `get_ui_catalog` and `turn_output` (an agent announces its answer, [ADR 0031](../../../docs/decisions/0031-working-text-and-the-turns-answer.md)). It is not under `/mcp`, and the edge should not route it from the public side: agents call the orchestrator directly |
| `webhook-generic` (with `WEBHOOK_GENERIC_SECRETS`) | `POST /webhooks/ci`: a signed CI report, a **machine route**: no user identity (it never reads `X-Auth-Request-Email`), guarded by an HMAC-SHA-256 over `"<timestamp>.<body>"`. The edge must pass `/webhooks/*` to the orchestrator without injecting an identity ([`api/webhooks.md`](../../../docs/api/webhooks.md)) |
| `webhook-github` (with `WEBHOOK_GITHUB_SECRETS`) | `POST /webhooks/github`: GitHub's `check_run` and `workflow_run` deliveries (`completed` only; `ping` is 204, every other event is 202 and ignored), a **machine route** guarded by `X-Hub-Signature-256` over the raw body |

That is the whole list. The legacy chat API interaction routes (`POST /api/threads`,
`POST /api/threads/{id}/messages`, `GET /api/threads/{id}/events`, `GET /api/threads/{id}/stream`),
the crate `orch-surface-chat-api` and its feature `surface-chat-api` were **removed on 2026-09-30**
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)). Those routes answer 404
(`POST /api/threads` answers 405, because its path is also the thread list).

**Migrating an existing deployment.** An `ORCH_SURFACES` (or `--surfaces`) that still lists
`chat-api` fails closed: the process exits 78 (`EX_CONFIG`) before it connects to anything,
with

```text
ORCH_SURFACES is invalid: surface "chat-api" was removed on 2026-09-30: the legacy chat API
interaction routes (createThread, postMessage, listEvents, streamEvents; POST /api/threads and
/api/threads/{id}/messages, GET /api/threads/{id}/events and /api/threads/{id}/stream). Use AG-UI
instead: set ORCH_SURFACES=agui (the default) and speak POST /agui/agents/{agentId}, GET
/agui/threads/{threadId}/connect and GET /agui/agents/{agentId}/capabilities (docs/api/agui.md);
the resource API (GET /api/threads, GET /api/threads/{id}, GET /api/agents, cancel) is unchanged
```

Nothing is quietly ignored: drop `chat-api` from the list (or unset the variable) and move the
client. Create a thread and send a message with one `POST /agui/agents/{agentId}` (a UUID you
mint as `threadId`), read the log with `GET /agui/threads/{threadId}/connect`
([`docs/api/agui.md`](../../../docs/api/agui.md)). The web app has run on AG-UI only since
2026-09-29.

## Tests

* Unit tests in `src/config.rs`: no database, no environment (the gate: the defaults, the variables, `ci`
  accepted and `ORCH_CI_TIMEOUT_SECS` read (and overridden by a target's `ci.timeoutSecs`), the webhook: mounted by name with its secrets (both webhooks), a mounted route without secrets refused (a worker need not have them), the count, the 32-byte minimum, the skew and the redaction of the secrets, its feature off; a `ci` gate with no names or no webhook surface refused, and honoured beside a webhook or in a worker; the verifier read from `ORCH_GATE`, `ORCH_VERIFIER` and
  `ORCH_VERIFIER_TIMEOUT_SECS` (a missing or unknown verifier, a self-verifying one, a bad timeout refused), strict parsing of `gate:`, a
  target that weakens the deployment or exceeds the cap; defaults, the
  environment/flag mapping, unknown, empty and repeated surfaces, the removed
  `chat-api` refused with an error naming it and AG-UI (`RemovedSurface`, from the variable and from the
  flag), `--help` naming every variable, the role: default `all`, each
  value, blank, unknown, the flag collected raw). `src/main.rs` maps every
  `HostError` to exit 70. `src/logging.rs`: role and instance first on every JSON and text
  line (an instance with a quote stays valid JSON, no fields means the stock line).
* Unit tests of the thread-tools settings in `src/config.rs`: the key, the URL, the lifetime and the hosts read (the default host from the URL and its port, a rotation, a lifetime from 60 to 86400), read by every role and without the surface when set, refused when the surface is named without them, when only one of the key and the URL is set, when a key is short, repeated as the previous one or the previous one has no current key, and for a URL, lifetime or host that is not one; nothing prints a key, in `Debug` or in an error; a build without the feature refuses the surface.
* Unit tests of the models in `src/config.rs` and `src/config/file.rs` (ADR 0035): a file's several endpoints and both tasks reach the application as the settings they run with (each task's endpoint, model, prompt read from its file or inline, tokens, language rule as the core's, the description's length and `minNewMessages`, the endpoint's timeout as the bound of a try, `ui.showDescriptions`), the legacy variables are the endpoint `default` beside the file's other endpoints (S9 refused them), every language of the file is a rule of the core, and the file and the variables give the same configuration. And of the title model in `src/config.rs`: off unless `ORCH_TITLE_MODEL` is set (the endpoint variables alone turn nothing on), the endpoint required with it (`Missing`), the trailing slash cut, the scheme checked (`Invalid` for `ftp://`, a bare host, `https://`), the timeout at least 1 and refused even when titles are off, the name and timeout reaching `AppConfig`, and no key in `Debug`.
* `src/config/file.rs` tests of `auth`: without a section the mode is `proxy_header` (and without a file too); the section reaches the configuration; `AUTH_DEV_USER` beside `jwt` is refused through the file's rule; a mode whose feature is missing is refused naming it; a worker needs none; `--print-config` shows it. `tests/smoke.rs`: `in_jwt_mode_only_a_valid_token_is_an_identity_and_readiness_follows_the_keys` (a real process against a local issuer: 503 and `/readyz` 503 while the keys cannot be fetched, 200 once they can; no token, a client-supplied header, a wrong audience, garbage, an unpublished key and `alg: none` are 401 with the challenge; no token reaches the log) and `a_production_process_refuses_the_proxy_header_before_anything_connects`.
* Unit tests of the registry settings in `src/config.rs`: no registry unless `AGENT_REGISTRY_URL` is set (the tokens alone turn nothing on), the defaults (3 s, 60 s) and the values read, the URL refused unless it is an absolute `http(s)` URL with a host and no user name or password (and the refusal repeats no password), the two numbers refused out of 1 to 60 and 1 to 3600 whether or not a registry is set, no secret and no query string in any `Debug`, `AGENTS_FILE` optional (unset, `[]`, empty, only comments) when a registry is set and still read and validated when it is, and still required, with agents, without one; and, in a build without the feature (`--no-default-features`), a URL refused with `RegistryNotCompiled` naming `registry-platform`.
* `src/config/file.rs` test of `sharing` (ADR 0040): the section reaches the application as the file says (the mode, the keys, the base URL, the public options and the limits) and the secret does not print; `disabled` drops the keys.
* Unit tests of the roles (S15): the built-in policy without a file and the same policy in `AppConfig`; the file's `auth.roles` and `auth.defaultRole` become the policy (the built-ins, `defaultRole: null`, custom roles that replace them with no default, a named default); the `role` of an MCP token reaches its principal and a role the policy does not define is `Invalid` naming the roles it has.
* Unit tests of the MCP settings in `src/config.rs`: tokens, hosts, public URL and wait bound read and normalised (user lower-cased, token trimmed, hosts split and required to be authorities, origins, the 32-byte token minimum, the wait limits; `MCP_WAIT_MAX_SECS` 1 to 86400), nothing read unless `mcp` is mounted (and not by a `worker`), every missing piece named (`Missing`, `McpTokenEnvMissing`, `McpTokensFileRead`, `Invalid` for a bad file, host list or URL), a rotation allowed and a shared token refused, no token in `Debug`; `src/main.rs` maps the new errors to exit 78.
* Unit tests of the file loader in `src/config/file.rs` (no database, no environment): **a file gives the `Config` the variables
  would have given** (compared whole, instance id apart); without a file, the environment alone and one note; a variable over the
  file is a note naming the variable and the key and never the value; one that says what the file says, or that the file names as
  `{ env }`, overrides nothing; a secret variable is a reference to itself and a flag wins over its variable; the process
  overrides are `Process` notes; each variable read with its own parser (`yes` is `true`, a comma list); one that does not
  parse is an error naming the variable and no value; the rest of a group whose first key is not set is read and left out; the
  `ORCH_MODEL_*` variables are the endpoint `default` and a file that names another is an error naming both; the webhook
  variables keep their comma rule and a file secret is one secret; the rules the binary has always had are put in the file's
  words with no value; every shape error at once; `--print-config` shows references and no value, and what it prints reads back;
  the table covers every variable and each key is a row of `docs/api/config.md`; `agents.localConcurrency` is refused without
  `agent-local` and read with it.
* `tests/local.rs` (`#![cfg(feature = "agent-local")]`, run with `--features agent-local`): the executable hosting
  a local `echo` agent answers an AG-UI run and the journal holds the run (`orch_agent_runs`); a `control-plane`
  process with a local agent starts, accepts a run and leaves it `queued` with an empty journal until a `worker`
  process starts and completes it. A test that fails prints the binary's log. Unit tests in `src/config.rs` cover the flavours: without the feature a local agent is
  refused naming `agent-local`, with it it is accepted, and `AGENT_LOCAL_CONCURRENCY` defaults to 4.
* Unit tests in `src/artifacts.rs`: no settings is `Off`, which refuses every call with "no artifact store configured"; `Debug` shows no credential; a directory store is built (the root and its parents are made) and keeps, reads and removes a file; a root that is a file stops startup naming `artifacts.fs.root`; a bucket store is built with no connection (nobody listens on its endpoint) and shows no credential in any error; and, in a build without a store's feature, building it is refused naming the feature. Unit tests in `src/config/file.rs`: no section is no store; a directory store reaches the `Config` with its root resolved against the file's directory and `--print-config` shows the key as written; a bucket store arrives with its credentials resolved, in no `Debug` and not in what `--print-config` prints; missing credential variables are 78 naming each variable; a build without a store's feature refuses it (78, naming the feature); a store without its section is refused by every build.
* `src/config/file.rs` tests also pin `toolServers` (ADR 0024): the public part reaches `AppConfig` (a name trimmed, the icon, the allow-list, the agents, the timeout) with no URL, credential or icon in any `Debug`; the servers also reach the relay's part of the configuration as `ToolServerEndpoint`s, in the order of the file, with the URL, the timeout and the credentials resolved and in no `Debug`; no key is nothing attachable; an `agents` id the agents file lacks is refused naming `toolServers[i].agents[j]`; a credential at an `http://` URL is a note that names the server and nothing else. `tests/smoke.rs` exits 78 for a list of mistakes, with no value on stderr, and `--print-config` shows the references and never a credential. `the_binary_relays_an_attached_servers_tool_with_the_configured_bearer` runs the real process on a configuration file that lists a web search with a bearer: a fake agent that lists `thread-tools/v1` calls the relayed tool with its grant, a real MCP server sees the bearer, the call is one step with the server's icon, and neither the bearer, the grant's token nor the key is in the thread or the log.
* `tests/smoke.rs`: the built executable as a process.* `tests/smoke.rs`: the built executable as a process. The configuration file: a file with many mistakes lists every one on stderr and exits 78 with no value in any line; a syntax error and a missing file are 78; `--print-config` prints the merged configuration with references and never a value (and the variable that won on stderr), lists errors with exit 78, and **runs on `dev/orchestrator.yaml` and `dev/orchestrator.live.yaml`**, each beside its agents file, as a control plane and as a worker that is not given the routes' secrets; a variable over the file is a `WARN` naming the variable and the key and a process override an `INFO`, before anything connects; without a file, one warning. With a database and a linkset server of its own, `the_platforms_agents_are_served_with_no_agent_file_and_leave_when_the_registry_goes_down`: no `AGENTS_FILE`, `AGENT_REGISTRY_URL` naming the registry; `/api/agents` lists its agent (`source: registry`, its tags), `/api/registry` says both sources are `ok`, a thread runs on that agent and the agent receives `AGENT_REGISTRY_AGENT_TOKEN`, then with the registry down the list is empty, `/api/registry` says `unavailable` with its detail, a run on the agent is a 503, and the list is back when the registry is; no secret in the log. Without the feature, a registry URL exits 78 naming `registry-platform`. Without the feature, `transport: local` exits 78 naming `agent-local`. The artifact store: `--print-config` prints the section with references and no credential; a bucket without a bucket name, a directory store without its section and a `maxFileBytes` of 0 are 78 before anything connects; with a database, a root that is a file stops the process with 78 naming `artifacts.fs.root` and a root that is not there yet is made (under the file's directory) and the process serves and exits 0 on SIGTERM; without `artifacts-fs`, `store: fs` exits 78 naming the feature. Configuration-error
  tests always run (including a gate this build cannot run (the verifier with no agent to ask), in the environment and in `AGENTS_FILE`: exit 78; the unreachable-database one waits out sqlx's 30 s
  connect timeout). The CLI tests spawn the executable: `--help`, each variable
  read from the environment alone, a flag over its variable, a usage error, the removed
  `chat-api` (`the_removed_chat_api_surface_is_a_config_error_pointing_to_agui`: from the variable,
  beside `agui`, from the flag, and the flag over a valid variable: exit 78, the log says it was
  removed on 2026-09-30 and points to AG-UI, nothing connects first), and the default serving `agui`
  and the resource API only (the AG-UI route answers 400 to `{}` and 401 without identity; the
  four removed legacy routes answer 404, or 405 for `POST /api/threads`, while the thread list, an
  unknown thread and cancel answer as resources). With a database: a gate that requires the verifier (`ORCH_GATE`,
  `ORCH_VERIFIER`, `ORCH_VERIFIER_TIMEOUT_SECS`) through two fake agents, the verifier finding something once
  (one run, the verifier a subagent of its own, two verifications in contexts of their own, its own credentials);
  `/healthz`, `/readyz`, 401 without
  identity, a thread run and completed over AG-UI (the default surface) through a fake agent with the bearer from
  `tokenEnv`, JSON logs, a clean exit on SIGTERM, and two processes on one
  database with a SIGKILL mid-task. The MCP surface (`mcp_without_its_tokens_or_hosts_is_a_config_error`: each missing piece is
  exit 78 naming it, before anything connects, with no token in the log; `mcp_is_mounted_by_its_name_and_a_token_lists_the_tools`:
  the process mounted with `ORCH_SURFACES=agui,mcp` answers 401 with the challenge without or with a wrong token, `tools/list` (six tools) and
  `list_agents` with the token, 403 for a `Host` that is not listed; `mcp_is_not_there_unless_it_is_named`: 404). The thread-tools surface (`the_thread_tools_surface_fails_closed_and_never_prints_its_key`: named with no key, with a key and no URL, with a short key or a bad URL, in `all` and in a `worker`, is exit 78 naming the variable before anything connects and never prints the key; `the_thread_tools_endpoint_is_mounted_by_its_name_and_a_minted_token_opens_it`: the process mounted with `ORCH_SURFACES=agui,thread-tools` refuses no token, a token that is not one, a token for a thread nobody created and a token for another agent with the same `401`, and a token minted with the configured key for the thread a run created lists `get_ui_catalog` and `turn_output`, with the key never in the log; `thread_tools_is_not_there_unless_it_is_named`: not mounted, with the key and URL set; `an_agent_that_lists_the_extension_calls_the_binary_back_with_the_grant_it_was_given`: the whole loop through the real binary, whose A2A adapter mints the grant for a fake agent that lists `thread-tools/v1`, the agent calls the binary's own endpoint back with it and reports the catalog the run's screen sent, and neither the token nor the key is in the thread or the log). The roles: `--role worker` (over a
  nonsense `ORCH_ROLE`) serves `/healthz` and `/readyz` answers 404 on
  `/api/...` and `/metrics` with role and instance on every log line; a `control-plane` process serves the API but the thread stays
  `queued` and the agent is never called until a worker process starts (its `/metrics` shows one
  due row meanwhile), then it completes and the backlog reads zero; a due timer row that a control plane alone leaves pending and a worker process applies (`INBOX_POLL_SECS=1`); one control plane and two workers, where the worker that holds
  the delegation is SIGKILLed and the other finishes it (message delivered once,
  events once each); a worker stopped on SIGTERM within its grace hands a running
  task over at once.

| Variable | Meaning |
|---|---|
| `ORCH_TEST_DATABASE_URL` | enables the database tests; without it they print a notice and pass without running |

CI runs them against a Postgres service and against the built image
([`orchestrator.yml`](../../../.github/workflows/orchestrator.yml)).

## See also

[`orch-app`](../../crates/app/README.md),
[`orch-api`](../../crates/api/README.md),
[`orch-e2e`](../../crates/e2e/README.md).
