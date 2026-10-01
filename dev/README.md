# Local development stack

## Test it locally

One command starts the whole system on your machine, **offline and deterministic**: the chat UI, the
orchestrator, Postgres, the default agent (adam-coder) and a scripted model, GitHub, git remote and CI for it, and two more
agents beside it, a chat and a researcher ([Several agents](#several-agents)). No
account, no API key, no network after the images are built. Every credential in it is a dummy and every port is
bound to `127.0.0.1`.

### What you need

| | |
|---|---|
| Docker Compose | v2 (`docker compose version`); **v2.24.4 or newer** for `compose.live.yaml`, which uses the `!override` tag that older versions do not parse (the version is from Docker's documentation, *not run* on that release; this stack was checked with v5.1.1). |
| Disk and memory | About 10 GB of free disk and 8 GB of memory for Docker: the coder image is 2.9 GB, and the Rust and web builds add several more. *An estimate, not measured.* |
| CPU | `linux/amd64`. The coder image has no arm64 build, so `compose.yaml` names the platform and an ARM machine (Apple Silicon) runs it under emulation (slower; your Docker setup must have emulation enabled). |
| Host tools | `curl`, `jq`, `git` and `openssl`, for the scenario scripts (not for the stack). |
| Free ports (all on 127.0.0.1) | **8080** the edge (chat, API, MCP, webhooks), 5432 Postgres, 8081 to 8083 the mock agents, 8090 the coder, 8091 to 8093 its model, GitHub and git mocks, 8094 the chat's and researcher's model, 8096 the mock web search, 8097 the chat, 8098 the researcher. Each has a variable (`EDGE_PORT`, `POSTGRES_PORT`, `CODER_PORT`, `CHAT_PORT`, `RESEARCHER_PORT`, `MOCK_*_PORT`, `GIT_SERVER_PORT`; see [`.env.example`](../.env.example)) if it clashes. |

### Start it

```sh
docker compose --profile app up --build
```

The first run pulls the coder (2.9 GB; the chat and the researcher run from the same image, so it is pulled once) and builds the orchestrator (Rust: several minutes), the web UI, the git
server and the mock CI; a later run takes seconds. The logs stream in this terminal and Ctrl-C stops it. To run it in the
background and return when everything is healthy: `docker compose --profile app up -d --build --wait`. It is ready
when `docker compose --profile app ps` shows `edge`, `coder`, `chat` and `researcher` as `healthy` (the orchestrator has no health check
of its own: the edge probes it). `docker compose --profile app down -v` stops it and forgets the databases and the
pushed branches (`-v` matters: see [Troubleshooting](#troubleshooting)).

### Where things are

| What | URL | Notes |
|---|---|---|
| The chat UI | http://127.0.0.1:8080 | The coder is preselected. Every request carries the fixed identity `dev@example.com`: the edge stands in for oauth2-proxy and authenticates nobody |
| The API | http://127.0.0.1:8080/api/agents, `/api/threads` | The resource API. The AG-UI run route is `POST /agui/agents/{agentId}` (what the web and the scripts speak) |
| MCP | http://127.0.0.1:8080/mcp | Bearer token `dev-mcp-token-0123456789abcdef0123456789`; see [Connect Claude Code](#connect-claude-code-over-mcp) |
| Webhooks | `POST http://127.0.0.1:8080/webhooks/github` and `/webhooks/ci` | Signed with the dummy secret `dev-webhook-secret-0123456789abcdef0123`, no identity. `mock-ci` posts here on its own; [`ci-webhook.sh`](ci-webhook.sh) plays a CI by hand |
| Probes | http://127.0.0.1:8080/healthz, `/readyz` | |
| The mocks' journals | http://127.0.0.1:8091/__admin/requests (the coder's model), :8094 (the model of the chat and the researcher), :8092 (GitHub), :8081 (mock agent), :8083 (verifier) | What each mock was asked, and `/unmatched` for what it did not know |
| The git remote | http://127.0.0.1:8093/local/sandbox.git | Seeded; the branches the coder pushes are here |
| The mock web search | http://127.0.0.1:8096/mcp (MCP, bearer `dev-search-token`), `/__journal` | An MCP server with one canned `web_search` tool; [Mock web search (MCP)](#mock-web-search-mcp) |

### Try it in the chat

Open http://127.0.0.1:8080, keep **Coder** selected and say hello first:

```text
hi
```

The coder answers with a greeting, not a request for a task: it says its name, what it does in one sentence and asks
which repository to work on, and the thread waits for you (**Blocked**, an A2A `input_required`). The words are the
first lines of the coder's instructions, which the model mock repeats back
([Change what the coder says](#change-what-the-coder-says); `greeting-e2e.sh` asserts it). Then send, in the same
thread or a new one:

```text
In http://git-server:8080/local/sandbox.git (base branch main), add hello.txt containing hello.
```

The scripted model always does the same job (clone `local/sandbox.git`, write `hello.txt`, push a branch, open a
pull request on the mock GitHub); the text only has to name the seeded repository. What you see, in order:

1. **Working**, with the coder's status lines and its artifacts as cards: the coder's `checks` (twice: the run
   of the checks, then the same result bound to the commit it pushed), the `branch` it pushed and the `pull_request` it
   opened (JSON, not a link).
2. The pill turns to **Checking the work…**: the coder is gated on two things, its own checks and CI
   ([`agents.yaml`](agents.yaml)). A card **Passed · Agent checks** shows at once, with the short commit and the summary.
3. A second card **Passed · CI** follows within a few seconds: `mock-ci` saw the pushed branch on `git-server` and reported
   `mock-ci/build` for that commit through the webhook. The badge ends **Done**.

Two more agents answer without a pull request: pick **Chat** and say `hi`, or pick **Researcher** and ask
`Who won the football world cup in 2014?` ([Several agents](#several-agents)). Which gate, badge and card each agent shows (pick the agent in the chat, send the keyword; the scripts assert all of it):

| Agent | Send | What the chat shows | Script |
|---|---|---|---|
| **Coder** | `hi` | a greeting that says "I'm Coder", what it does and asks which repository; the thread is **Blocked**, waiting for you | `greeting-e2e.sh` |
| **Coder** | the repository message above | the steps above: checks, **Checking the work…**, **Agent checks** and **CI** cards, **Done** | `coder-e2e.sh` |
| **Chat** | `hi` (or anything) | a greeting that says "I'm Chat" and what it does, no repository question, and the thread is **Done** | `agents-e2e.sh` |
| **Researcher** | `Who won the football world cup in 2014?` | "I searched the web for you. The best source I found is https://example.org/mock-search/world-cup-2014." and **Done** | `agents-e2e.sh` |
| **Mock coder (gated)** | `red-once fix the login` | **Checking the work…**, a card **Failed · Agent checks** with the finding, a **rework divider** ("Attempt 2 of 3: sent back with 1 finding"), a second card **Passed · Agent checks**, **Done** | `verify-e2e.sh` |
| **Mock coder (gated)** | `red-always fix the login` | three failed cards, two dividers, the pill **Failed** and "Checks failed after 3 attempts" | `verify-e2e.sh` |
| **Mock coder (verified)** | `push-flawed fix the login` | the coder, then the **Verifier** as a subagent of its own (a pending card, then **Failed · Verifier** with its findings), the divider, the coder again, the verifier again, **Passed · Verifier**, **Done** | `verifier-e2e.sh` |
| **Mock coder (CI gated)** | `red-once fix the login`, then play CI from a terminal ([CI](#ci-the-gate-by-webhook)) | **Checking the work…** until a signed report arrives; a red one sends the agent back, a green one for the new commit ends the job | `ci-e2e.sh` |
| **Mock coder** | anything, or `slow` | no gate: **Working**, then **Done** with a pull-request artifact; `slow` takes 8 s | `mcp-e2e.sh` (over MCP) |

The web draws the gate's verdicts (`vymalo.check`: a **CI** card is the gate's verdict on the check it required),
the rework divider and the verifier subagent, and one CI result card per report (`vymalo.ci`: the conclusion, the check
name, the short commit and a link to the run), which is also where the scripts assert them. The badge, counter and cards come back after a reload: the page replays
the log.

### Share a chat with a developer

When something goes wrong in a thread (a card that looks wrong, a job that ended where you did not expect), send the developer
the whole thread as one file. In the chat, **Export JSON** in the thread's header downloads `thread-<id>.json`. From a
terminal, with the stack up:

```sh
dev/export-thread.sh <thread-id>              # writes thread-<thread-id>.json here; the id is in the address bar, /threads/<id>
dev/export-thread.sh <thread-id> chat.json    # or name the file ("-" writes it to stdout)
```

The file is `GET /api/threads/{id}/export` ([`docs/api/chat-api.yaml`](../docs/api/chat-api.yaml), operation `exportThread`): a
versioned document (`format` `another-agentic-system/thread-export`, `version` 1) with the thread, its **full job** (the gate, the
attempt, the pushed commit, what each check said), the agent binding and **every event of the log in order**: your messages, every agent
status and artifact, the check, CI and verifier cards, each rework and the state changes. Every card of the chat is drawn from that log.
**Read it before you send it.** It holds what you and the agents wrote in the thread, and your e-mail address as the author of your
messages; it never holds a credential of the orchestrator (no bearer token, webhook secret or database URL is ever written to the log), but a
person can paste anything into a chat. Only the owner of a thread can export it (another identity gets a 404, as when reading it).
`coder-e2e.sh` and `verify-e2e.sh` export the thread they drive and check what is in the file.

### Run the scenarios

Each scenario is one script of this directory, and `e2e-all.sh` runs them all against the running stack and prints a summary:

```sh
dev/e2e-all.sh                 # every scenario below, one after the other, then a summary
dev/e2e-all.sh verify mcp      # only these
VERBOSE=1 dev/e2e-all.sh       # stream each script's output instead of keeping it in a log file
```

| Scenario | Script | It proves |
|---|---|---|
| `greeting` | `dev/greeting-e2e.sh` | "hi" gets a greeting that says the coder's name and what it does and asks which repository, and the thread waits (`blocked`); the model got the folder's instructions |
| `agents` | `dev/agents-e2e.sh` | `GET /api/agents` lists `coder chat researcher`; the chat greets in role (`done`, no repository talk, no tool of the coder); the researcher searches the mock web search exactly once with the person's words and answers citing a link of it; the coder still greets and waits (`blocked`); the model mock matched every request |
| `choices` | `dev/choices-e2e.sh` | the coder asks three questions at once as one form drawn from the web's catalog (one `a2ui-surface` with a `Choices`, under the catalog's id); one action answers them and the coder's next words quote them; a message from a newer screen records a second `ui_catalog`; the thread's own tools reached the coder ([Choices](#choices-the-coder-asks-with-a-form)) |
| `cards` | `dev/cards-e2e.sh` | the researcher searches the mock web search and answers with one surface under the web's catalog (a Text, three cards with the links it found, a Mermaid graph) beside its words; an older screen writing to the thread leaves its catalog alone; a screen whose catalog has no `Cards` gets words only ([Cards and Mermaid](#cards-and-mermaid-the-researcher-answers-with-cards-and-a-graph)) |
| `title` | `dev/title-e2e.sh` | after the agent's first reply the thread is given a short title by the orchestrator's own model (`mock-title` on `mock-model`: one `thread_titled` of the orchestrator with `source: model`, the sidebar's list says it, the model was asked once with the conversation fenced as data); a model that says `NONE` or fails (a 500, asked three times) leaves the first words as the title and the thread `done`; a person's rename is final, the model is not asked again ([Thread titles](#thread-titles-the-orchestrator-asks-a-model)) |
| `coder` | `dev/coder-e2e.sh` | a chat message becomes a branch, `mock-ci` reports it green and the job is `done`, with a pull request opened once |
| `coder-no-opencode` | `NO_OPENCODE=1 dev/coder-e2e.sh` | the same when the check command makes the change |
| `verify` | `dev/verify-e2e.sh` | red once, sent back, green; red always, failed; and a run cannot weaken the gate |
| `verifier` | `dev/verifier-e2e.sh` | the verifier finds fault, the agent is sent back, the verifier passes it |
| `mcp` | `dev/mcp-e2e.sh` | an MCP client starts a job and follows it with progress notifications |
| `ci` | `dev/ci-e2e.sh` | a red signed report sends the agent back, a green one for the new commit ends the job |
| `folder` | `dev/agent-folder-e2e.sh` | the coder restarted on a copy of its agent folder with another name (`docker compose up -d --no-build`, no rebuild) greets as that name, then on its own folder as its own again. It restarts the coder, so it runs last; without `docker compose` on the machine that runs the stack it is `SKIP` |

Every script prints one `ok` or `FAIL` line per check and exits non-zero on a failure; `e2e-all.sh` exits 1 if any scenario
failed and prints the tail of its output. `ci` passes **once per database** (a commit belongs to the first job that
pushed it), so a second run of it is reported as `SKIP`, not as a failure (so is `folder` where there is no `docker compose`): `docker compose --profile app down -v` and
`up` again to run it fresh. The split roles (`dev/split-e2e.sh`) need another shape of the stack and are not in the list
([The split profile](#the-split-profile-a-control-plane-and-two-workers)); `dev/check-mocks.sh` checks the WireMock agents alone and needs only `docker compose up -d --wait`; `dev/check-agent-mocks.sh` checks the mock web search and the scripted models (the agents' and the title's) and needs `docker compose --profile app up -d --wait mock-mcp-search mock-model`.

### Connect Claude Code over MCP

The stack serves the orchestrator's MCP server at `http://127.0.0.1:8080/mcp` ([ADR 0019](../docs/decisions/0019-mcp-server-over-streamable-http.md)).
With the stack up:

```sh
claude mcp add --transport http orchestrator http://127.0.0.1:8080/mcp \
  --header "Authorization: Bearer dev-mcp-token-0123456789abcdef0123456789"
```

[`mcp.json.example`](mcp.json.example) is the same for a client that reads a JSON file (Claude Code's `.mcp.json`, opencode
and most others). Then ask Claude to list the agents and start a job on `mock-coder` ("start a job with mock-coder: add a
health endpoint"); the tools are `list_agents`, `start_job`, `get_job`, `wait_for_job`, `answer` and `cancel_job`, and the answer of
`start_job` carries a `web_url`: the same job is in the chat, because the token belongs to `dev@example.com`. More in
[The MCP server](#the-mcp-server). *Claude Code and opencode against this server have not been run; `curl` and rmcp's own client have.*

### Going live

The offline stack never reads `.env`. To point the coder at a real model and a real GitHub, copy [`.env.example`](../.env.example)
to `.env`, edit it, and add the override file:

```sh
cp .env.example .env         # then edit it: a model endpoint, a GitHub token, and secrets of your own (openssl rand -hex 32)
docker compose -f compose.yaml -f compose.live.yaml --profile app up --build
```

[`compose.live.yaml`](../compose.live.yaml) (Compose v2.24.4 or newer, for `!override`) replaces the coder's whole environment
with the values of `.env`; stops `mock-openai`, `mock-github`, `git-server`, `mock-ci` and `mock-model` (they move to a profile,
`offline-mocks`, that is never enabled, and the coder no longer waits for them); puts the real secrets on the orchestrator
(`CODER_A2A_TOKEN`, `WEBHOOK_GITHUB_SECRETS`, `MCP_TOKEN_DEV`, each 32 bytes or more); and gives the orchestrator
[`agents.live.yaml`](agents.live.yaml), where the coder is gated on its own checks only. The chat and the researcher
go live with it: `compose.live.yaml` gives them the same model endpoint (`CHAT_MODEL` and `RESEARCHER_MODEL` name another alias for each, else
`MODEL`) and a bearer token each (`CHAT_A2A_TOKEN`, `RESEARCHER_A2A_TOKEN`, from `.env`), and drops `mock-model`. The orchestrator's own thread titles go live the same way (`TITLE_MODEL`, else `MODEL`). **The live researcher still
searches the mock web search**, canned results whatever the question: this stack has no search provider credential. To search for real,
write the `url` and the token of a search MCP server of your own into a copy of `dev/agents/researcher/agent/mcp.json` and point
`RESEARCHER_AGENT_DIR` at it ([Add a fourth agent by writing a folder](#add-a-fourth-agent-by-writing-a-folder) says how a folder names its tools). In the chat, name a repository you can push to
(`In https://github.com/<you>/<repo>.git (base branch main), ...`; the host must be in `ALLOWED_REPO_HOSTS`). Check the
files without starting anything: `docker compose -f compose.yaml -f compose.live.yaml --env-file .env.example config -q`.

Notes on going live:

- **The edge still authenticates nobody** and still says `dev@example.com` for every request. Live means a real model and a real
  GitHub, not a stack you may expose. The MCP token and the webhook secret are the only credentials that mean anything.
- A variable exported in your shell **wins over `.env`**: an exported `GITHUB_TOKEN` (common when you use `gh`) is the one the coder gets. `docker compose ... config` shows the result.
- **The CI gate is opt-in live**, because the check name `mock-ci/build` means nothing on GitHub: edit `agents.live.yaml` as its comments say
  (the exact name of the check run, and the webhook below).
- **GitHub webhooks need a public URL.** GitHub cannot reach `127.0.0.1`. Either expose port 8080's `/webhooks/github` yourself
  (a tunnel of your choosing: point it at a route that carries **only** that path, never at the edge, which injects an identity), or
  use `smee` below.
- **smee (optional, opt-in).** Set `SMEE_URL` in `.env` to a channel from https://smee.io/new and add `--profile smee` (or
  `COMPOSE_PROFILES=app,smee` in `.env`). `smee-client` (pinned, [`dev/smee/Dockerfile`](smee/Dockerfile)) then forwards the deliveries to
  `smee-proxy`, a Caddy of its own ([`Caddyfile.smee`](Caddyfile.smee)) that passes `POST /webhooks/github` to the orchestrator and answers
  404 to everything else; nothing reaches the identity-injecting edge. In the repository's Settings, Webhooks: Payload URL = your smee URL,
  Content type `application/json`, Secret = `WEBHOOK_GITHUB_SECRETS`, events "Check runs" and "Workflow runs" (not "Check suites"), and give the
  orchestrator the check's name in `agents.live.yaml`. **smee.io is a third party: it sees every payload** (repository names, commit
  messages, check results) and anyone who learns the channel URL can read and post to it; the orchestrator still verifies the
  signature. *Unverified:* smee re-serialises the JSON body it relays, so a payload GitHub escapes differently (`<`, `>`, `&` are written
  `<` and so on) may fail the signature check with a 401; a delivery with plain text is fine (tried here with smee-client 5.0.0 against a stand-in relay).
- The `local-agent` profile is separate: [An agent inside the orchestrator](#an-agent-inside-the-orchestrator-agent-local).

### Troubleshooting

| You see | It is | Do |
|---|---|---|
| `port is already allocated` or `address already in use` | one of the ports above is taken (a local Postgres on 5432 is the usual one) | stop it, or set the variable of that port (`POSTGRES_PORT=5433 docker compose ...`, or in `.env`); the scripts read `EDGE_PORT` too, or take `BASE_URL` |
| `docker compose` rejects `compose.live.yaml` at an `!override` tag | Compose older than v2.24.4 | update Docker Compose |
| MCP answers `403` | **Host validation**: the server accepts `Host` 127.0.0.1 and localhost only (`MCP_ALLOWED_HOSTS`), and you reached the edge by another name (a LAN address, `host.docker.internal`, a tunnel) | use `http://127.0.0.1:8080/mcp`, or add the name to `MCP_ALLOWED_HOSTS` in `compose.yaml` |
| MCP answers `401` with `WWW-Authenticate: Bearer` | the token is missing or wrong (nothing says which, on purpose) | send `Authorization: Bearer <MCP_TOKEN_DEV>`; the token is the value in the orchestrator's environment, and `dev/mcp-tokens.yaml` names the variable |
| a webhook delivery gets `401` | **a secret mismatch** (or, for the generic route only, a timestamp more than `WEBHOOK_GENERIC_MAX_SKEW_SECS`, 300 s, from the clock): the signature matches none of `WEBHOOK_GITHUB_SECRETS` / `WEBHOOK_GENERIC_SECRETS` | the same secret on both sides (`docker compose logs orchestrator` says `webhook delivery refused ... status=401`); GitHub's "Recent Deliveries" shows the response; via smee see the note above |
| a report is `202` but the job stays **Verifying** | the report is **parked** (the inbox keeps it up to a day) until a job watches its key, host/owner/name of the repository and the commit | compare the two log lines that carry the same `watch_key`: `watching for the CI reports of a pushed commit` (the job, from the coder's `branch` artifact) and `a CI report will be matched to the job that watches this key` (the report); `dev/coder-e2e.sh` prints both when the thread does not end done: `docker compose logs --tail 100 orchestrator mock-ci` |
| the two `watch_key`s differ, and no report ever matches | **repository spelling**: the coder names the repository the way the chat message did (`http://git-server:8080/local/sandbox.git`), CI reports `repository.html_url`; case, `.git` and a trailing slash are ignored, a different host, port or owner is not | name the repository in the chat as the report spells it (`https://github.com/<owner>/<repo>.git`); locally `mock-ci` reports `http://git-server:8080/local/sandbox` |
| a job ends **Blocked** with `ci_timeout` | no report for the pushed commit arrived within `ORCH_CI_TIMEOUT_SECS` (an hour); no attempt was used | fix the delivery (the rows above) |
| `dev/ci-e2e.sh` says `SKIP`, or a second run of it times out | it passes once per database (a commit belongs to the first job that pushed it) | `docker compose --profile app down -v` and `up` again |
| an old scenario behaves oddly, or the coder cannot clone | leftovers: the databases, the pushed branches (`coder-work`, `git-data`) | `docker compose --profile app down -v` forgets all of them |
| `coder` never becomes healthy | the image is still being pulled or is starting (`start_period` 10 s, then 30 tries), or it exited | `docker compose --profile app logs coder`; on an ARM machine it runs under emulation and is slow |
| a chat message to the coder fails at once with a delivery error | the coder was not up yet: the orchestrator reads its card when a thread is delegated, not at boot | wait for `coder` to be `healthy`, send it again |
| `.env` values seem ignored | the offline stack never reads `.env`; and a variable exported in your shell wins over it | add `-f compose.live.yaml`; `docker compose -f compose.yaml -f compose.live.yaml config` shows what is used |

Read on for the details of every service and scenario.

What `compose.yaml` at the repository root starts, and how to steer the mock agents. The quick
start is in the root [README](../README.md#local-development); this page is the reference.

The mocks are [WireMock](https://wiremock.org/) 3.13.2 stubs of the one protocol the orchestrator
speaks to agents: A2A 1.0 over JSON-RPC and SSE. They stand in for a real coding agent so the whole
loop (chat, orchestrator, Postgres, A2A adapter) runs on a laptop without an agent host, a model or
a GitHub token. They hold no state and are not a substitute for the agent contract tests of a real
host. The `app` profile also runs a real agent, adam-coder, the default agent
([below](#the-default-agent)).

## Services

| Service | Image | Host port (127.0.0.1) | Profile | What it is |
|---|---|---|---|---|
| `postgres` | `postgres:16.15-alpine` | `5432` (`POSTGRES_PORT`) | default | The orchestrator's database `orch`, and `orch_test` for `cargo test`. User and password are both `postgres`. Named volume `postgres-data`. |
| `mock-agent` | `wiremock/wiremock:3.13.2` | `8081` (`MOCK_AGENT_PORT`) | default | A fake A2A 1.0 coding agent. |
| `mock-agent-releases` | `wiremock/wiremock:3.13.2` | `8082` (`MOCK_AGENT_RELEASES_PORT`) | default | The same agent, declaring the [release-channels extension](https://github.com/vymalo/another-agentic-platform/blob/main/docs/extensions/release-channels-v1.md). |
| `mock-verifier` | `wiremock/wiremock:3.13.2` | `8083` (`MOCK_VERIFIER_PORT`) | default | A fake A2A 1.0 **verifier** agent ([ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md)): it answers a request to review a commit with a `verdict` artifact, findings for a commit of forty `a` and a pass for any other ([below](#verifier-the-verifier-agent-of-the-gate)). |
| `orchestrator` | built from [`orchestrator/`](../orchestrator/Dockerfile) | not published | `app` | The real orchestrator, with [`dev/agents.yaml`](agents.yaml): the coder first (the default agent, under a gate of its own checks and CI), then `chat` and `researcher`, then the mocks (`mock-coder`, `mock-coder-gated` under the verification gate, `mock-coder-verified` under the verifier's, the `verifier` itself, `mock-coder-ci` under a CI gate, `mock-coder-releases`). `ORCH_ROLE` is `all` unless `ORCHESTRATOR_ROLE` says otherwise, and `ORCH_SURFACES` is `agui,mcp,thread-tools,webhook-generic,webhook-github`: the AG-UI routes the web and the scripts here run on, beside the resource API, the [MCP server](#the-mcp-server) at `/mcp`, the [thread tools](#the-thread-tools) at `/thread-tools/{threadId}/mcp` (not routed by the edge), and the two webhooks `POST /webhooks/ci` and `POST /webhooks/github` (secret `dev-webhook-secret-0123456789abcdef0123`, see [CI](#ci-the-gate-by-webhook)). The legacy chat API routes were removed on 2026-09-30 (`ORCH_SURFACES` naming `chat-api` stops the orchestrator at startup). |
| `web` | built from [`web/Dockerfile`](../web/Dockerfile) | not published | `app` | The real chat UI. |
| `edge` | `caddy:2.11.4-alpine` | `8080` (`EDGE_PORT`) | `app` | Stands in for oauth2-proxy: one origin for the UI, the API (`/api/*`), the AG-UI routes (`/agui/*`, streams unbuffered) and the MCP server (`/mcp`, unbuffered, **no identity header**: it authenticates a bearer token itself). |
| `orchestrator-worker-1`, `orchestrator-worker-2` | the `orchestrator` image | not published | `split` | Workers: `ORCH_ROLE=worker`, so the dispatcher and a port that serves only `/healthz`, `/readyz` and `/metrics`. The instance id is the service name (it is the `lease_owner` of the outbox rows they hold) and the lease is 5 s. See [the split profile](#the-split-profile-a-control-plane-and-two-workers). |
| `coder` | `ghcr.io/vymalo/another-adam-rs/coder`, pinned by tag and digest (once, as `x-adam-image` at the top of `compose.yaml`) | `8090` (`CODER_PORT`) | `app` | adam-coder, the default agent: an A2A agent that turns a task into a branch and a pull request. About 2.9 GB, `linux/amd64` only. It reads its agent folder (instructions, card) from [`coder/agent/`](coder/agent/instructions.md), mounted read-only at `/etc/adam/agent` (`ADAM_AGENT_DIR`; `CODER_AGENT_DIR` points the mount elsewhere), once at startup: [Change what the coder says](#change-what-the-coder-says). |
| `coder-postgres` | `postgres:16.15-alpine` | not published | `app` | The coder's own database, `coder`. Named volume `coder-postgres-data`. |
| `mock-openai` | `wiremock/wiremock:3.13.2` | `8091` (`MOCK_OPENAI_PORT`) | `app` | The coder's model endpoint: two scripts, `mock-coder` and `mock-opencode`. Vendored, see [`coder/UPSTREAM`](coder/UPSTREAM). |
| `agents-postgres` | `postgres:16.15-alpine` | not published | `app` | The database `agents`, shared by every agent that is only a folder (`chat`, `researcher`, and the next one): runs are scoped by the agent's name. Named volume `agents-postgres-data`. |
| `mock-model` | `wiremock/wiremock:3.13.2` | `8094` (`MOCK_MODEL_PORT`) | `app` | The model of the chat and the researcher, and of the orchestrator's thread titles: three scripts, `mock-persona`, `mock-researcher` and `mock-title`, in [`wiremock/model/mappings/`](wiremock/model/mappings). Ours, not vendored. See [Several agents](#several-agents) and [Thread titles](#thread-titles-the-orchestrator-asks-a-model). |
| `chat` | the coder's image, entrypoint `tini -- adam-agent` | `8097` (`CHAT_PORT`) | `app` | A casual chat: `adam-agent` serving the folder [`agents/chat/agent/`](agents/chat/agent/instructions.md), mounted read-only at `/etc/adam/agent` (`CHAT_AGENT_DIR` points the mount at a copy), model `mock-persona`. |
| `researcher` | the coder's image, entrypoint `tini -- adam-agent` | `8098` (`RESEARCHER_PORT`) | `app` | A researcher: the folder [`agents/researcher/agent/`](agents/researcher/agent/instructions.md) (`RESEARCHER_AGENT_DIR`), whose `mcp.json` names the mock web search, model `mock-researcher`. Waits for `mock-mcp-search` to be healthy. |
| `mock-github` | `wiremock/wiremock:3.13.2` | `8092` (`MOCK_GITHUB_PORT`) | `app` | The GitHub REST subset the coder uses to open a pull request. Vendored. |
| `git-server` | built from [`coder/git-server/`](coder/git-server/Dockerfile) | `8093` (`GIT_SERVER_PORT`) | `app` | A git remote over smart HTTP, seeded with `local/sandbox.git`. No authentication. Vendored. |
| `mock-ci` | built from [`mock-ci/`](mock-ci/Dockerfile) (`alpine:3.23`, pinned by tag and digest, with git, curl and openssl; the secret is read from `WEBHOOK_SECRET` and never on a command line) | not published | `app` | The CI of the repository, as a stand-in: polls `git ls-remote` on `git-server` for `agent/*` branches and posts a signed GitHub `check_run` named `mock-ci/build` (`MOCK_CI_SHAPE=github-workflow`: a `workflow_run`; `generic`: the generic body) for each new commit through the edge. The coder is gated on CI, so its jobs end `done` when this has reported. See [CI](#ci-the-gate-by-webhook). |
| `mock-mcp-search` | built from [`mock-mcp-search/`](mock-mcp-search/Dockerfile) (`node:24-alpine3.23`, pinned by tag and digest; no dependencies, nothing is installed) | `8096` (`MOCK_MCP_SEARCH_PORT`) | `app` | A mock web-search MCP server: streamable HTTP at `http://mock-mcp-search:8080/mcp` (bearer `dev-search-token`), one tool `web_search` with an icon, canned results from [`mock-mcp-search/results.json`](mock-mcp-search/results.json). See [Mock web search (MCP)](#mock-web-search-mcp). |
| `smee-proxy` | `caddy:2.11.4-alpine` | not published | `smee` | A Caddy of its own ([`Caddyfile.smee`](Caddyfile.smee)) that passes `POST /webhooks/github` to the orchestrator and nothing else (404); never the identity-injecting `edge`. See [Going live](#going-live). |
| `smee` | built from [`smee/`](smee/Dockerfile) (`node:24-alpine3.23` by tag and digest, `smee-client` 5.0.0) | not published | `smee` | Forwards the deliveries smee.io holds for `SMEE_URL` to `smee-proxy`. Opt-in; exits with a message when `SMEE_URL` is unset. smee.io is a third party that sees the payloads. |
| `orchestrator-local`, `local-postgres` | the orchestrator built with `--build-arg ORCH_FEATURES=agent-local` (long: it links the adam-rs runtime); `postgres:16.15-alpine` | `8095` (`ORCH_LOCAL_PORT`) | `local-agent` | A second orchestrator that hosts an `echo` agent in its own process ([`agents.local-echo.yaml`](agents.local-echo.yaml)), with a database of its own and `AUTH_DEV_USER` for the identity; no web UI. See [An agent inside the orchestrator](#an-agent-inside-the-orchestrator-agent-local). |

The default profile builds nothing and starts in seconds. `--profile app` builds the two images
(the Rust build takes a few minutes the first time), the git server, `mock-ci` and `mock-mcp-search`, and pulls the coder image (which
`chat` and `researcher` use too). `--profile smee` and
`--profile local-agent` are opt-in and belong to no other profile. [`compose.live.yaml`](../compose.live.yaml) is an override, not a profile.

```mermaid
sequenceDiagram
  actor U as Browser or curl
  participant E as edge (Caddy)
  participant W as web
  participant O as orchestrator
  participant P as postgres
  participant M as mock-agent (WireMock)
  U->>E: GET / , /api/* and /agui/* on 127.0.0.1:8080
  E->>W: everything except /api/* and /agui/*
  E->>O: /api/* and /agui/* with X-Auth-Request-Email: dev@example.com (any client value replaced)
  O->>P: append the event, enqueue the delegation
  O->>M: GET /.well-known/agent-card.json
  M-->>O: card (streaming, bearer scheme, interface URL from the Host header)
  O->>M: POST /a2a SendStreamingMessage, Authorization: Bearer dev-mock-token
  M-->>O: SSE: task, statusUpdate, artifactUpdate, statusUpdate
  O->>P: append agent_status, artifact and thread_state events
  U->>E: POST /agui/agents/{agentId} (the run), or GET /agui/threads/{id}/connect
  E->>O: SSE (unbuffered)
  O-->>U: the AG-UI frames, live
```

### The edge is not oauth2-proxy

`edge` puts the chat UI, the resource API and the AG-UI routes on one origin, as the production ingress does, and sets
`X-Auth-Request-Email: dev@example.com` on every API and AG-UI request, replacing whatever the client sent. `/webhooks/*` is the one
exception: it reaches the orchestrator with **no** identity header at all (a webhook is authenticated by its signature).
It authenticates nobody. It exists so the UI works locally without an identity provider; it must
never be exposed beyond `127.0.0.1` (the compose file binds it there) and never used in production,
where oauth2-proxy authenticates the user and the orchestrator trusts the header only because the
proxy owns it (see the identity notes in [`orchestrator/README.md`](../orchestrator/README.md)).

Without `--profile app`, talk to a host-run orchestrator with `AUTH_DEV_USER` or send the header
yourself, as [`try-thread.sh`](try-thread.sh) does.

## The default agent

The first entry of [`agents.yaml`](agents.yaml) is the default agent, and it is
[adam-coder](https://github.com/vymalo/another-adam-rs) ([ADR 0014](../docs/decisions/0014-adam-coder-default-agent-over-a2a.md)):
`GET /api/agents` lists it first and the chat UI preselects it. The two WireMock mocks stay in the
file, after it, to try the other thread endings.

**Provenance.** The image is the published one, pinned in `compose.yaml` by tag `sha-<7>` and digest.
Everything else the coder needs is vendored from the same adam-rs commit, named in
[`coder/UPSTREAM`](coder/UPSTREAM), byte for byte, under `coder/`:

| Vendored path | Upstream path | What it is |
|---|---|---|
| `coder/wiremock/mock-openai/` | `dev/wiremock/mock-openai/` | `mappings/coder-script.json`, `coder-choices.json` ([Choices](#choices-the-coder-asks-with-a-form)) and `opencode-script.json`, plus the bodies they reference (`opencode-bash.sse`, `opencode-done.sse`, and `chat-text.sse` and `chat-text.json` as OpenCode's fallbacks). Nothing else of the upstream mock: an off-script request must be a 404. |
| `coder/wiremock/mock-github/` | `dev/wiremock/mock-github/` | `mappings/pulls.json` and its two bodies. |
| `coder/git-server/` | `dev/git-server/` | The Dockerfile, nginx config, entrypoint and the seed of `local/sandbox.git`. |
| `coder/agent/` | `bin/adam-coder/agent/` | The agent folder the coder reads at run time (`instructions.md`: its name, its card, its instructions), mounted at `/etc/adam/agent`. The whole upstream folder, nothing else. |

Do not edit them here. [`coder/check-vendored.sh`](coder/check-vendored.sh) compares every one with
`raw.githubusercontent.com` at the commit in `UPSTREAM`, checks that nothing is missing (every body file
a vendored mapping names is vendored too, and `coder/git-server/` and `coder/agent/` hold exactly the files of
`dev/git-server/` and `bin/adam-coder/agent/` upstream, listed through the GitHub API), and checks that `compose.yaml` pins the
image of that commit (`sha-<first 7 characters>@sha256:`); CI runs it first. The mappings are a
deliberate subset, the scripted coder run only: a mapping the coder starts to need upstream shows up
as an unmatched request in `dev/coder-e2e.sh`. To move to a newer adam-rs
commit, change the commit in `UPSTREAM`, refresh the copies, and re-pin the image, all in one change. The pin is written **once**,
as `x-adam-image` at the top of `compose.yaml`: the coder and the agents that are only a folder ([Several agents](#several-agents)) take it by
alias (`adam-agent` ships inside the same image), and the check fails on a second pin. Not vendored, on purpose: upstream's `mock-assistant` model
and its example agent `dev/agents/assistant`; the chat and the researcher here are ours and follow the same persona convention.

**The scripted run.** The coder's model is `mock-coder`, a script the mock follows by looking at which
tool-call ids the conversation already holds (it keeps no state). The message names the seeded
repository, and the coder then calls `prepare_workspace`, `delegate_to_opencode`, `run_checks`,
`commit_and_push` and `open_pull_request`, and ends with a text. Since adam-rs `ae540e9` the coder also reports its checks: `run_checks` emits a `checks`
artifact (`passed`, `commit`, `tree`, `summary`, `findings`), and `commit_and_push` emits a second one, bound to the commit it pushed, before the `branch`
artifact. The orchestrator's gate for the coder (`require: [agent-checks, ci]`) reads the last one: it must have passed on exactly the pushed commit. OpenCode (model `mock-opencode`) runs
one `bash` call, `echo hello > hello.txt`. With `[mock:no-opencode]` in the message OpenCode is not
started and `run_checks` makes the file itself. The result is a branch `agent/<run id prefix>` on
`git-server` with `hello.txt` = `hello`, and one pull request created on `mock-github`.

A greeting (`hi`, `hello`, `hey`) is not a task: `mock-coder` answers it with
`Hi! I'm <name>. <summary>. Which repository should I work on, and what should I change?`, built from the first two
lines of the instructions the coder rendered into its system prompt (`Your name is {{display_name}}.` and
`In one sentence: <summary>.`), and the person's next message (the repository) continues with the script above.

```mermaid
sequenceDiagram
  actor U as coder-e2e.sh
  participant E as edge
  participant O as orchestrator
  participant C as coder
  participant M as mock-openai
  participant G as git-server
  participant H as mock-github
  U->>E: GET /api/agents, POST /agui/agents/coder (the first agent)
  E->>O: with X-Auth-Request-Email
  O->>C: SendStreamingMessage, bearer CODER_A2A_TOKEN
  C->>M: chat completions, model mock-coder (tool calls, one per turn)
  C->>G: clone local/sandbox.git, push agent/run-prefix
  C->>M: OpenCode runs, model mock-opencode, bash echo hello
  C->>H: POST /repos/local/sandbox/pulls
  C-->>O: artifacts branch and pull_request, then completed
  O-->>U: RUN_FINISHED, then the frames with the two artifacts
  U->>H: __admin journal, exactly one POST
  U->>M: __admin journal, nothing unmatched
  U->>G: ls-remote and clone, hello.txt is hello
```

```mermaid
stateDiagram-v2
  [*] --> Created: POST /agui/agents/coder
  Created --> Delegated: dispatcher sends the message
  Delegated --> Scripted: mock-coder answers each turn
  Scripted --> Scripted: next tool call
  Scripted --> Published: branch pushed, pull request created
  Scripted --> Off_script: a request the script does not know
  Off_script --> Failed: 404 from mock-openai
  Published --> Done: task completed
  Done --> Verified: journals and git match
  Failed --> [*]
  Verified --> [*]
```

### Change what the coder says

What the coder says and offers is a folder, not code: [`coder/agent/instructions.md`](coder/agent/instructions.md) holds its
name (`display_name` and `card.name`), its card and its instructions. `compose.yaml` mounts that folder read-only at
`/etc/adam/agent` and sets `ADAM_AGENT_DIR` to it; the coder reads it **once, at startup**, so a change needs a restart and
no rebuild ([adam-rs, "A folder at run time"](https://github.com/vymalo/another-adam-rs/blob/7b2d8f95ffd9abe8af990bd79d7d690e8183c392/bin/adam-coder/README.md#a-folder-at-run-time-adam_agent_dir)):

```sh
$EDITOR dev/coder/agent/instructions.md            # for example: display_name: Cody (and card.name: Cody)
docker compose --profile app up -d coder           # recreates only the coder: the same image, a few seconds
docker compose --profile app logs coder | grep 'agent files'   # source=folder, the path, a digest, the agent, the warnings
```

```mermaid
sequenceDiagram
  actor P as person
  participant F as dev/coder/agent (mounted at /etc/adam/agent)
  participant D as docker compose
  participant C as coder
  participant O as orchestrator
  participant M as mock-openai
  P->>F: edit instructions.md (the name, the first two lines)
  P->>D: up -d coder
  D->>C: new container, same image, the folder mounted read-only
  C->>F: read once at startup (ADAM_AGENT_DIR)
  C-->>D: healthy, or exit 78 with every problem as path:line
  P->>O: "hi" in a new thread
  O->>C: GET the card, then SendStreamingMessage "hi"
  C->>M: chat completions, system prompt = the folder's instructions
  M-->>C: "Hi! I'm name. summary. Which repository ...?"
  C-->>O: input_required with that question
  O-->>P: the greeting, and the thread waits (blocked)
```

```mermaid
stateDiagram-v2
  [*] --> Reading: container starts
  Reading --> Running: the folder is valid (warnings are logged)
  Reading --> Refused: an error in the files, or not the coder's name (exit 78)
  Running --> Reading: up -d coder after an edit (a restart, no build)
  Refused --> Reading: fix the folder, up -d coder
```

Then say `hi` in the chat (a new thread): the greeting follows the new first two lines. A folder with an error (YAML, a tool that does not exist in
`tools:`, a `name` other than `coder`, no `max_check_cycles` var) stops the coder at startup with every problem as `path:line`
(exit status 78): `docker compose --profile app logs coder` says which. What a folder cannot change is what the tools do (a folder
may narrow them with `tools:`). Runs that were in flight when the coder restarted are durable and go on, and a changed tool
set can fail them on replay, as any new version of the code would
([adam-rs ADR 0004](https://github.com/vymalo/another-adam-rs/blob/7b2d8f95ffd9abe8af990bd79d7d690e8183c392/docs/decisions/0004-agent-folders-at-run-time.md)).

To try a change **without touching the vendored copy**, copy the folder and point the mount at the copy (`dev/agent-folder-e2e.sh`
does exactly this and puts the coder back):

```sh
cp -R dev/coder/agent /tmp/my-coder && chmod -R a+rX /tmp/my-coder   # the container runs as uid 10001
CODER_AGENT_DIR=/tmp/my-coder docker compose --profile app up -d coder
```

An edit to `dev/coder/agent/` that you commit **fails CI**: `coder/check-vendored.sh` compares it with the file upstream at the
commit in [`coder/UPSTREAM`](coder/UPSTREAM). Change the instructions in `vymalo/another-adam-rs` (`bin/adam-coder/agent/`), then move
this directory, the commit and the image pin together ([above](#the-default-agent)). With `compose.live.yaml` the same mount applies
(`ADAM_AGENT_DIR` is in its replaced environment).

**Run it.**

```sh
docker compose --profile app up -d --build --wait     # pulls the coder, builds git-server, mock-ci, orchestrator, web
dev/coder-e2e.sh                                       # OpenCode makes the change
NO_OPENCODE=1 dev/coder-e2e.sh                         # the check command makes it
dev/e2e-all.sh                                         # this and every other scenario, with a summary ("Test it locally")
docker compose down -v                                 # also forgets the pushed branches
```

`dev/coder-e2e.sh` goes through the edge and speaks AG-UI, as the web does (the legacy chat API was removed on
2026-09-30): it checks the default agent, runs the thread with one
`POST /agui/agents/coder` (a UUID it mints as `threadId`), waits for the thread to end `done`,
and prints one `ok` or `FAIL` line for each check: the run stream ends with `RUN_FINISHED`, the artifacts (`checks` at least twice, the last passed on exactly the pushed commit
with a 40-hex tree; `branch`; `pull_request`; the JSON the coder sent is in the `content.text` of the `vymalo.artifact` activities of
`GET /agui/threads/{id}/connect?mode=run`), the job's gate `ci+agent_checks` with an `agent_checks` `vymalo.check` card that passed on the pushed commit and exactly one `vymalo.ci` card for `mock-ci/build`, exactly one `POST /repos/local/sandbox/pulls` on `mock-github`
with the branch as head and `main` as base, no unmatched request on `mock-openai` and at least one
`mock-opencode` request (none with `NO_OPENCODE=1`), and the branch with `hello.txt` on `git-server`. It
resets both journals first, so it can be run repeatedly. To use the chat by hand, open
http://127.0.0.1:8080, keep the coder selected and send
`In http://git-server:8080/local/sandbox.git (base branch main), add hello.txt containing hello.`
The pull request appears as JSON in the `pull_request` artifact, not as a link (adam-coder sends its URL
in a data part).

**Limits.**

- `linux/amd64` only and about 2.9 GB: the first `--profile app` run pulls it. `compose.yaml` sets `platform: linux/amd64`, so an arm64
  machine runs it under emulation instead of failing to pull. The default profile does not start it.
- The coder's card advertises `PUBLIC_URL`, `http://coder:8080/` in compose, so **an orchestrator on the
  host cannot use it**: [`agents.local.yaml`](agents.local.yaml) leaves it out, and its default is
  `mock-coder`. Run the orchestrator in the compose network to use the coder.
- The model is a script, not a model: any text other than a greeting runs the same script
  (it always clones `local/sandbox.git` and writes `hello.txt`), and the greeting is the instructions' first two lines repeated back.
  How a live model follows the instructions is *unverified* here (`compose.live.yaml`). `mock-openai` has no fallback for
  `mock-coder`, so a change of the coder's tool calls upstream shows up as a 404 and a failed thread.
- `mock-openai`, `mock-github` and `git-server` are the adam-rs mocks with all their limits (no
  authentication, no state beyond the journal and the repository). `docker compose down -v` is what
  gives the next run a fresh repository and fresh databases.
- The coder does not support `ListTasks`: if the orchestrator dies between sending a message and
  recording the task, the retry starts a second run (ADR 0014).

## Several agents

`GET /api/agents` lists three agents, in the order of [`agents.yaml`](agents.yaml), and the chat UI preselects the first:

| Agent | What it is | On the mocks, "hi" or any message | Gate | Script |
|---|---|---|---|---|
| `coder` | adam-coder, the default agent ([above](#the-default-agent)) | a greeting that asks which repository to work on; the thread waits (`blocked`) | `agent-checks` and `ci` | `greeting-e2e.sh`, `coder-e2e.sh` |
| `chat` | `adam-agent` over the folder [`agents/chat/agent/`](agents/chat/agent/instructions.md): greets, chats in plain words, has no tool of its own and does not talk about repositories | `Hi! I'm Chat. I chat with you and answer your questions in plain words.`; the thread is `done` | none | `agents-e2e.sh` |
| `researcher` | `adam-agent` over [`agents/researcher/agent/`](agents/researcher/agent/instructions.md): searches the web before it answers and cites every source as a link. Its `mcp.json` names the [mock web search](#mock-web-search-mcp) | it calls `search__web_search` with your words, then `I searched the web for you. The best source I found is <the first link of the results>.`; the thread is `done`. With `[mock:cards]` in the question it goes on to show the sources as cards and a graph ([Cards and Mermaid](#cards-and-mermaid-the-researcher-answers-with-cards-and-a-graph)) | none | `agents-e2e.sh`, `cards-e2e.sh` |

An agent with no gate is `done` when it says so (an agent whose answer is a question parks the thread `blocked`, as the coder's greeting does).

**An agent here is a folder.** `adam-agent` (vymalo/another-adam-rs, `bin/adam-agent`) serves the one agent a folder describes:
`instructions.md` (the frontmatter is the name, the card and the limits, the body is the system prompt), an optional `mcp.json` naming MCP
servers whose tools it gets as `<server>__<tool>`, optional `skills/` and `subagents/`. It ships **inside the coder's image**, beside
`adam-coder`, so there is no second image to pull or publish: a service is that image with the entrypoint `tini -- adam-agent` and the folder mounted
read-only at `/etc/adam/agent` (`ADAM_AGENT_DIR`). The folder is read once, at startup (a change needs `docker compose --profile app up -d <id>`, no
rebuild), and a mistake in it stops the agent with every problem as `path:line` (exit status 78, `docker compose --profile app logs <id>`). The
contract is adam-rs's ([`bin/adam-agent/README.md`](https://github.com/vymalo/another-adam-rs/blob/f882b910b620ea583130a0517b4e52c5f7939179/bin/adam-agent/README.md), ADR 0005
there); the three facts that matter here:

- **The persona lines.** The body of every folder here opens with `Your name is {{display_name}}.` and `In one sentence: <summary>.`
  (the summary without `"` and ending at its first period; `display_name` is a var of the frontmatter, kept in step with `card.name`). The model mock
  greets from those two lines as the agent rendered them into its system prompt, so **editing them changes the mocked answer**, for any folder.
- **The tools of a folder** are `ask_user`, `show` and `ui_catalog` (the person's screen: [Choices](#choices-the-coder-asks-with-a-form)), one tool per
  MCP tool of its `mcp.json` (the `tools` allow-list of a server keeps only the ones listed), the tools of its skills and subagents, and the tools of
  the conversation the orchestrator announces ([the thread tools](#the-thread-tools): `get_ui_catalog`). Which kinds of MCP server a folder may name is
  the deployment's, not the file's: plain `http` to another container needs `MCP_ALLOW_INSECURE=true` (set for every folder service in `x-adam-agent-env`:
  development only; the thread tools are plain `http` to the orchestrator too), and a `${VAR}` in the `headers` of a server
  reads the service's environment (`SEARCH_MCP_TOKEN`), so the folder holds a name and never a secret.
- **One database for all of them.** The agents that are folders share `agents-postgres`: a run belongs to the agent's `name`, so no agent reads
  another's. (The coder keeps its own, `coder-postgres`.)

**The model of these two** is the `mock-model` service, WireMock with the scripts of [`wiremock/model/mappings/`](wiremock/model/mappings) (a stub per
model name; an off-script request is a 404, and `/__admin/requests/unmatched` lists them):

| Model | Answers | How |
|---|---|---|
| `mock-persona` | `Hi! I'm <name>. <summary>.` to any request; a fixed text when the last message is a tool result | `<name>` and `<summary>` are taken from the first message (the system prompt) by the two persona lines. Any folder that follows the convention works: it is what a fourth agent uses |
| `mock-researcher` | a tool call `search__web_search` with the person's words as `query`, then `I searched the web for you. The best source I found is <link>.` | the turn is told by the **last** message: a tool result means the search came back, so it answers with the first `https://` link of it (or says no source was found); anything else is a question, so it searches. The query is the first run of letters, digits and spaces of the question (at most 60 characters), because a template must not put a quote or a backslash into JSON, and `[mock:empty]` or `[mock:error]` therefore cannot reach the search through this model |
| `mock-researcher` with `[mock:cards]` in the conversation | four turns, told by the call ids the history holds: `search__web_search` for `async programming` (`cards-call-1`), then `ui_catalog` (`cards-call-2`), then `show` with a Text, a Cards of the three links of the search's `async` results and a Mermaid `graph TD` (`cards-call-3`), then `Here are the three sources I found: <the three links>.` | [`researcher-cards.json`](wiremock/model/mappings/researcher-cards.json). The three scripts above carry `doesNotContain "[mock:cards]"`, so a conversation that holds the keyword never reaches them; the blocks were validated against the web's catalog schemas when the file was written |

Limits of the scripts: the first three ignore the history (a second question in the thread is searched like the first, with the same tool call id
`researcher-call-1`; `[mock:cards]` reads the history, by its call ids, and once `cards-call-3` is in it answers in words only), and a real model is what makes the agent *good*, which the mocks cannot show ([Going live](#going-live)).

```mermaid
sequenceDiagram
  actor U as agents-e2e.sh
  participant O as orchestrator
  participant R as researcher (adam-agent)
  participant M as mock-model
  participant S as mock-mcp-search
  U->>O: POST /agui/agents/researcher, "Who won the football world cup in 2014?"
  O->>R: SendStreamingMessage, bearer RESEARCHER_A2A_TOKEN
  R->>M: chat completions, model mock-researcher, tools ask_user and search__web_search
  M-->>R: tool call researcher-call-1: search__web_search with the query
  R->>S: POST /mcp tools/call web_search, bearer dev-search-token
  S-->>R: numbered results, each with its link (one call in the journal)
  R->>M: chat completions again, the tool result is the last message
  M-->>R: I searched the web for you. The best source I found is the first link.
  R-->>O: task completed with that text
  O-->>U: RUN_FINISHED, the thread is done, the words are in its frames
```

```mermaid
stateDiagram-v2
  [*] --> Asked: a message reaches the researcher
  Asked --> Searching: the model calls search__web_search
  Searching --> Answered: the results, or an error, come back as a tool result
  Asked --> Answered: the model answers without a search
  Answered --> [*]: the task completes and the thread is done
```

A search that fails (the tool answers an error, or the server cannot be reached) is a result the model reads, not a failed run; only a search
server that is down **at startup** keeps the researcher from starting (`depends_on` waits for `mock-mcp-search`, and adam-agent exits 69 otherwise).

`dev/agents-e2e.sh` drives all three through the orchestrator (AG-UI, as the web does) and reads the journals of `mock-model` and `mock-mcp-search`;
`dev/check-agent-mocks.sh` plays the model scripts over HTTP. To see an agent's work by hand: pick it in the chat, or from a terminal
`AGENT_ID=researcher dev/try-thread.sh "Who won the football world cup in 2014?"`.

### Add a fourth agent by writing a folder

An agent is a folder and about a dozen lines of compose. This adds a `poet` that answers in rhyme (on the mocks it greets in role, which is
what the model mock gives any folder; to script more, add a model name to `wiremock/model/mappings/`).

1. **Write the folder** `dev/agents/poet/agent/instructions.md`, readable by uid 10001 (`chmod -R a+rX dev/agents/poet`):

   ```markdown
   ---
   name: poet
   description: "A poet: it answers every message in four lines that rhyme."
   vars:
     display_name: Poet
   card:
     name: Poet
     skills:
       - id: rhymes
         name: Rhymes
         description: Answers in four lines that rhyme.
   ---
   Your name is {{display_name}}.
   In one sentence: I answer in four lines that rhyme.

   You are {{display_name}}. Answer every message in four lines that rhyme, in plain words.
   ```

   `name` is the registered name of the agent and keys its stored runs, so do not rename it later. Every `vars` entry needs a value in the
   file, and every `{{var}}` the body uses must be declared. Optional: a `mcp.json` (the tools of an MCP server: copy the researcher's), `skills/`
   and `subagents/` (adam-rs's [authoring guide](https://github.com/vymalo/another-adam-rs/blob/f882b910b620ea583130a0517b4e52c5f7939179/docs/authoring.md)).
2. **Add the service** to `compose.yaml`, copying `chat` (the anchors `x-adam-agent` and `x-adam-agent-env` carry the image, the entrypoint, the
   healthcheck and the database; a folder that names an MCP server also copies `researcher`'s `depends_on` and `SEARCH_MCP_TOKEN`; `MCP_ALLOW_INSECURE` is in the anchor):

   ```yaml
     poet:
       <<: *adam-agent
       environment:
         <<: *adam-agent-env
         MODEL: mock-persona                  # greets from the persona lines of the folder
         A2A_BEARER_TOKENS: dev-poet-token    # the orchestrator sends it as POET_A2A_TOKEN
         PUBLIC_URL: http://poet:8080/        # the compose name: what the card advertises and the orchestrator posts to
       ports:
         - "127.0.0.1:${POET_PORT:-8099}:8080"   # optional: only to reach it from the host
       volumes:
         - ./dev/agents/poet/agent:/etc/adam/agent:ro
   ```
3. **Tell the orchestrator.** In `x-orchestrator-env` add `POET_A2A_TOKEN: dev-poet-token`, and in [`agents.yaml`](agents.yaml) an entry
   (after `researcher`, before the mocks, so the coder stays the default):

   ```yaml
   - id: poet
     name: Poet
     cardUrl: http://poet:8080/.well-known/agent-card.json
     tokenEnv: POET_A2A_TOKEN
   ```
4. **Start it.** The orchestrator reads `AGENTS_FILE` and its environment at startup, so recreate both:
   `docker compose --profile app up -d poet orchestrator`.
5. **Check it.** `curl -s -H 'X-Auth-Request-Email: dev@example.com' http://127.0.0.1:8080/api/agents | jq -r '.[].id'` lists `poet`; say `hi`
   to it in the chat, or `AGENT_ID=poet dev/try-thread.sh hi`: on the mocks it answers `Hi! I'm Poet. I answer in four lines that rhyme.`

Going live with it takes the same three edits as `chat`'s in [`compose.live.yaml`](../compose.live.yaml) (a real model and a token from `.env`),
[`agents.live.yaml`](agents.live.yaml) and [`.env.example`](../.env.example). A folder that fails to load keeps the service from becoming healthy: read
`docker compose --profile app logs poet` (the `agent files` line says what was read, and an error is `path:line: error: ...`).

## The MCP server

The `orchestrator` service of the `app` profile also mounts the MCP server ([ADR 0019](../docs/decisions/0019-mcp-server-over-streamable-http.md),
`ORCH_SURFACES=agui,mcp`), so Claude Code, opencode or any MCP client can start and follow a job. It is a **machine route**:
the edge forwards `/mcp` without an identity header (and drops one the client sent), and the orchestrator authenticates
`Authorization: Bearer <token>` itself.

| What | Where |
|---|---|
| The URL | `http://127.0.0.1:8080/mcp` (the edge; `EDGE_PORT` moves it) |
| The token | `dev-mcp-token-0123456789abcdef0123456789`, a dummy: `MCP_TOKEN_DEV` in `compose.yaml`, named by `tokenEnv` in [`mcp-tokens.yaml`](mcp-tokens.yaml) (`MCP_TOKENS_FILE`) |
| Whose jobs | `dev@example.com`, the identity the edge gives the chat, so a job started over MCP is in the chat's thread list (`web_url` in the answer of `start_job` points at it: `ORCH_PUBLIC_URL`) |
| Which `Host` | `127.0.0.1` and `localhost`, any port (`MCP_ALLOWED_HOSTS`); anything else is 403 |
| The tools | `list_agents`, `start_job`, `get_job`, `wait_for_job`, `answer`, `cancel_job` |

```sh
claude mcp add --transport http orchestrator http://127.0.0.1:8080/mcp --header "Authorization: Bearer dev-mcp-token-0123456789abcdef0123456789"
```

[`mcp.json.example`](mcp.json.example) is the same as a generic client configuration (Claude Code's `.mcp.json`, most
others). Then ask the client to start a job on the agent `mock-coder` ("start a job with mock-coder: add a health
endpoint"), or drive it from the terminal:

```sh
dev/mcp-e2e.sh          # curl and jq: 401s, initialize, tools/list, start_job (and a retry), get_job to done, the refusals,
                        # then wait_for_job with a progress token on a `slow` job (8 s) and a timeout that is resumed
```

`wait_for_job` answers as an event stream when the request has a `progressToken`: one `notifications/progress` per event
of the job (`#3 artifact: Pull request`, a counter that only increases) and a heartbeat every 60 s, then the result. The
edge does not buffer it, so the notifications arrive as the events happen; the script reads them from the SSE response.
The script speaks MCP by hand and prints one `ok` or `FAIL` line per check. Its default agent is `mock-coder`, which ends with
a pull request in seconds; `AGENT_ID`, `MCP_TOKEN`, `BASE_URL`, `AUTH_EMAIL` and `TIMEOUT` change what it uses.
The server is **stateless**: it hands out no `Mcp-Session-Id`, so any replica serves any call.
A `wait_for_job` without a `progressToken` returns `timed_out` (with `resume_after_seq`) after at most one heartbeat interval (60 s),
and a user can hold 16 waits open (256 per process): more is the tool error "too many waits" (`MCP_WAIT_MAX_PER_USER`,
`MCP_WAIT_MAX_CONCURRENT`). A request with an `Origin` header (a browser) is 403 unless listed in `MCP_ALLOWED_ORIGINS`.
The dummy token is 32 bytes or more because the orchestrator refuses a shorter one.

Troubleshooting: `401` with `WWW-Authenticate: Bearer` is a missing or wrong token (nothing else says why, on
purpose); `403` is a `Host` the server does not list, for example a client that reaches the edge by another name
(add it to `MCP_ALLOWED_HOSTS`); the orchestrator refusing to start with `MCP_TOKEN_DEV` in the message means
the variable named by `mcp-tokens.yaml` is not set.

## The thread tools

The `orchestrator` service also serves **one MCP endpoint per thread**, `/thread-tools/{threadId}/mcp`
([`docs/api/thread-tools-v1.md`](../docs/api/thread-tools-v1.md)), for the agents it sends work to. It is a machine
route like `/mcp`, but **the edge does not route it**: an agent calls the orchestrator directly, on the compose network.
What an agent of the stack receives, **only if its card lists** `https://agents.vymalo.com/a2a/extensions/thread-tools/v1`
(read for every message), is this, in the metadata of the A2A message and activated in `A2A-Extensions`:

```json
{"https://agents.vymalo.com/a2a/extensions/thread-tools/v1": {
  "url": "http://orchestrator:8080/thread-tools/<threadId>/mcp",
  "token": "<an HS256 JWT scoped to that thread and that agent, valid two hours>",
  "expiresAt": "2026-10-01T14:00:00Z"}}
```

| What | Where |
|---|---|
| The key | `THREAD_TOOLS_SECRET`, a dummy in the `x-orchestrator-env` of `compose.yaml` (`dev-thread-tools-secret-…`); a `.env` that sets `THREAD_TOOLS_SECRET` replaces it (it is in [`.env.example`](../.env.example)). **Every orchestrator process has it**: a worker mints the grant it sends with the message, the control plane verifies it |
| The address agents use | `THREAD_TOOLS_URL=http://orchestrator:8080`, the service name; the `orchestrator` service names `thread-tools` in `ORCH_SURFACES` and accepts the `Host` values `THREAD_TOOLS_ALLOWED_HOSTS=orchestrator:8080,127.0.0.1:8080,localhost:8080` |
| The tool | `get_ui_catalog`: the newest UI catalog the thread's screen sent, or an error "this thread has no UI catalog; answer in text" |
| `split` profile | the control plane serves it, the workers (same variables) mint |

The coder and the agents that are folders (`chat`, `researcher`) list the extension since adam-rs `d411249` (the WireMock agents do not), and
list the endpoint's tools at every model turn with the grant of the message, so the model is offered what it lists (`get_ui_catalog` today) under its listed
name; the URL is plain `http` on the compose network, so these services set `MCP_ALLOW_INSECURE` ([Choices](#choices-the-coder-asks-with-a-form)).
The endpoint answers the checks of
[`orchestrator/crates/surface-thread-tools`](../orchestrator/crates/surface-thread-tools/README.md) and the whole loop is
tested by [`orchestrator/crates/e2e/tests/thread_tools.rs`](../orchestrator/crates/e2e/tests/thread_tools.rs) (the real
dispatcher and A2A adapter, a fake agent that lists the extension and calls back with the grant it was given, on the
in-memory store and on Postgres) and by the real binary in `orchestrator/bin/orchestrator/tests/smoke.rs`. To try it by
hand against a binary, start `orch-fake-agent` with the extension (`cargo run -p orch-testsupport --bin orch-fake-agent`
with `FAKE_AGENT_EXTENSIONS=thread-tools`, see [its README](../orchestrator/crates/testsupport/README.md)), point an
`AGENTS_FILE` entry at it (`coder` is on 127.0.0.1:4021, `plain` on 4022), give the
orchestrator `THREAD_TOOLS_SECRET` (32 bytes or more: `openssl rand -hex 32`) and `THREAD_TOOLS_URL`
(`http://127.0.0.1:8080`), name `thread-tools` in `ORCH_SURFACES`, and send it `thread-tools hello`: the agent calls the
endpoint back and its artifact says what it got (`thread-tools: tools=get_ui_catalog; …`); `GET /__control/<agent>/calls`
on the fake agent shows the grant under `threadTools`. A `401` from the endpoint is the same answer for a missing,
expired, foreign or forged token (nothing else says why, on purpose); a `403` is a `Host` it does not list.

## Choices: the coder asks with a form

Since adam-rs `d411249` ([#59](https://github.com/vymalo/another-adam-rs/pull/59), MVP slice 3 of [`docs/mvp.md`](../docs/mvp.md)) the coder, and
every agent served by `adam-agent`, draws from the component catalog of the person's screen
([ADR 0023](../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md), [`ui-catalog-v1.md`](../docs/api/ui-catalog-v1.md)): `ask_user`
takes `choices` (up to eight questions of two to eight options), and the coder asks several questions at once as **one form** instead of a
paragraph, `show` and `ui_catalog` draw other blocks. The person answers all of them with one submit, which is one action; the answers come back as the result of the
tool call, `- db: pg` per question, which the model quotes in its next words.

| What | Where |
|---|---|
| The catalog | the web's own, `web/src/features/chat/lib/a2ui/catalog/catalog.json` and its `catalog.lock.json`. The web sends it under `forwardedProps["vymalo.uiCatalog"]` on the run that opens a thread, and the orchestrator hands it to the agent (inline on the first message, by reference after). `dev/choices-e2e.sh` reads the same two files |
| What the coder says it can do | its card lists A2UI v0.9.1 with `acceptsInlineCatalogs`, `ui-catalog/v1` and `thread-tools/v1`; the orchestrator reads the card at every send (ADR 0008), so nothing is configured for it here |
| The thread's own tools | `get_ui_catalog` at `http://orchestrator:8080/thread-tools/<id>/mcp` ([above](#the-thread-tools)): plain `http` between containers, so the coder has `MCP_ALLOW_INSECURE: "true"` in `compose.yaml` (so have the folder agents, in `x-adam-agent-env`). Without it the grant is refused, the agent has no thread tools, and a catalog that comes only by reference (a later message, a coder that has not kept it) cannot be fetched: the coder asks in text, with the options listed |
| The mock model | `dev/coder/wiremock/mock-openai/mappings/coder-choices.json` (vendored, [`coder/UPSTREAM`](coder/UPSTREAM)): a task that holds `[mock:choices]` makes `mock-coder` call `ask_user` with three questions (database, login, where it runs); a request that holds the call's result and `db: pg` gets "Going with Postgres, Keycloak and Compose." |
| The scenario | `dev/choices-e2e.sh`, `choices` in `dev/e2e-all.sh` |

```mermaid
sequenceDiagram
  actor U as choices-e2e.sh
  participant O as orchestrator
  participant C as coder
  participant M as mock-openai
  U->>O: run 1: "[mock:choices] ...", forwardedProps vymalo.uiCatalog (the web's catalog)
  O->>C: SendStreamingMessage: ui-catalog/v1 (inline), A2UI capabilities, thread-tools/v1 {url, token}
  C->>O: tools/list at the thread's endpoint (plain http, MCP_ALLOW_INSECURE)
  C->>M: chat completions, model mock-coder, tools ask_user, show, ui_catalog, get_ui_catalog
  M-->>C: ask_user with three questions
  C-->>O: input-required: the question and one a2ui-surface (a Choices, under the catalog's id)
  O-->>U: the frames, then RUN_FINISHED (interrupt)
  U->>O: run 2: forwardedProps.a2uiAction.userAction (answers db=pg, auth=keycloak, deploy=compose)
  O->>C: one A2UI action on the same task
  C->>M: the tool result "db: pg, auth: keycloak, deploy: compose"
  M-->>C: "Going with Postgres, Keycloak and Compose."
  O-->>U: the frames, then RUN_FINISHED (interrupt)
  U->>O: run 3: a message with a newer catalog (version + 1, a dummy component)
  O-->>U: the export holds a second ui_catalog, the state says the newer one is the thread's
```

```mermaid
stateDiagram-v2
  [*] --> Asking: run 1 with the catalog
  Asking --> Form: the catalog is read and has Choices
  Asking --> TextQuestion: no catalog, none with Choices, or it cannot be read
  Form --> Waiting: input-required with the surface
  TextQuestion --> Waiting: input-required, the options in the text
  Waiting --> Answered: run 2, the A2UI action
  Answered --> Waiting: the coder's next words (it parks them as a question)
  Waiting --> NewerScreen: run 3, a catalog of a higher version
  NewerScreen --> Waiting: two ui_catalog events in the log, the newer is current
  Waiting --> [*]
```

The script prints one `ok` or `FAIL` line per check, and what it asserts is at the top of the file: the card, the one surface and its
`Choices`, the state's `thread.uiCatalog`, the tools the model was offered (`get_ui_catalog` among them: the grant arrived), the action and the
quoted answers, the second `ui_catalog`, and a model mock that answered every request. **To try it in the chat**, send the coder `[mock:choices] set up
the project`: a form with three questions should appear, and your answers come back as your own message, "Your answers". (That click path is covered by the
web's own tests on a fake agent; against the coder in containers it is *unverified* here, the script drives the same requests without a browser.)
The vendored mapping is a deliberate part of the mocks: without it the coder's model mock answers `[mock:choices]` with a 404, like any off-script request.

## Cards and Mermaid: the researcher answers with cards and a graph

Since adam-rs `c13ddf1` ([#60](https://github.com/vymalo/another-adam-rs/pull/60), MVP slice 4 of [`docs/mvp.md`](../docs/mvp.md)) the researcher
folder tells its agent to show what it found: after it searched it reads which components the screen has (`ui_catalog`) and, when there is a `Cards`, calls `show`
with one `Cards` block of the sources it used (title, site, a sentence, the link exactly as the search returned it, tags), and a `Mermaid` block when the question
is about how things relate. The blocks come back as **one surface**, a Column of a Text, the Cards and the Mermaid, under the screen's own catalogId; the words still
carry every link ([ADR 0023](../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md), [`ui-catalog-v1.md`](../docs/api/ui-catalog-v1.md) version 3: the web draws
the cards and the graph, and a surface whose card has no title or a link that is not a plain `http(s)` URL is refused, visibly).

| What | Where |
|---|---|
| The folder | [`agents/researcher/agent/instructions.md`](agents/researcher/agent/instructions.md): the stack's own text plus adam-rs's paragraph "Show what you found, when the screen can draw it" and the card's skill line, as in adam-rs's `dev/agents/researcher/agent/instructions.md` at `c13ddf1`. Not vendored (the folder is ours and `check-vendored.sh` does not read it): the paragraph is copied by hand when adam-rs changes it. `mcp.json` is ours (it names the mock web search). The folder is read at startup: `docker compose --profile app up -d researcher` after an edit |
| The sources | the mock web search's keyword `async` ([`results.json`](mock-mcp-search/results.json)): three results, `https://example.org/mock-search/async-book`, `/async-futures`, `/async-tokio` |
| The script | `mock-researcher` with `[mock:cards]` in the question: search for `async programming`, `ui_catalog`, `show` (a Text, three cards, a `graph TD`), then the words that name the links ([above](#several-agents)) |
| The catalog | the web's own (`catalog.json` and its lock, version 3): `dev/cards-e2e.sh` sends it as the web does |
| The scenario | `dev/cards-e2e.sh`, `cards` in `dev/e2e-all.sh` |

```mermaid
sequenceDiagram
  actor U as cards-e2e.sh
  participant O as orchestrator
  participant R as researcher (adam-agent)
  participant M as mock-model
  participant S as mock-mcp-search
  U->>O: run 1: "[mock:cards] what is async rust?", forwardedProps vymalo.uiCatalog (version 3)
  O->>R: SendStreamingMessage: ui-catalog/v1 (inline), A2UI capabilities, thread-tools/v1
  R->>M: chat completions, model mock-researcher
  M-->>R: search__web_search "async programming"
  R->>S: tools/call web_search
  S-->>R: three results with links
  R->>M: the results
  M-->>R: ui_catalog, then show (Text, Cards of three, Mermaid)
  R-->>O: artifact ui (application/a2ui+json) under the catalog's id, then the words
  O-->>U: one agent message and one a2ui-surface, RUN_FINISHED
  U->>O: run 2: the same thread from an older screen (no catalog sent)
  O-->>U: the thread's catalog is still version 3, no new ui_catalog in the log
  U->>O: run 3: a new thread from a screen whose catalog has no Cards (version 2)
  O->>R: the older catalog inline
  R-->>O: show is refused, the words only
  O-->>U: no a2ui-surface, the three links in the words
```

```mermaid
stateDiagram-v2
  [*] --> Searching: the question carries [mock:cards]
  Searching --> ReadingCatalog: results in
  ReadingCatalog --> Showing: the mock calls show whatever the catalog says
  Showing --> Surface: show accepted (the catalog has Cards), one ui artifact
  Showing --> Words: show refused (it has none), the model goes on
  Surface --> Words: the answer names every link
  Words --> [*]
```

An **older screen** is one whose own catalog version is below the thread's: by the contract it sends no catalog (it sends one only when the thread has none or its own is
newer), so the orchestrator tells the agent the thread's current catalog **by reference** and the agent keeps drawing against it; what the old browser does with a
`Cards` it does not know is the web's business (a visible "needs a newer version of the app" placeholder, covered by the web's tests). The scenario therefore asserts, for
that run, what the orchestrator owns: **no `ui_catalog` is added to the log and the thread's catalog stays at its version**. A screen that really is on a catalog without
`Cards` (run 3, a new thread) is the case the agent can see, and the researcher answers it in words. The catalog the script uses for it is the shipped one without `Cards` and
`Mermaid` and one version down, its digest recomputed with `jq` and `sha256sum`.

## Thread titles: the orchestrator asks a model

MVP slice 6 ([ADR 0005](../docs/decisions/0005-openai-compatible-model-endpoint.md) amended 2026-10-01; [`docs/orchestrator.md`](../docs/orchestrator.md) "Thread titles") is the first time the
orchestrator itself asks a model something. After an agent's reply it writes a 3 to 6 word title of the conversation, so the sidebar lists "Fix the login page" and not the first
message's first words. It asks the same OpenAI-compatible endpoint the agents use, and stays out of the way: a model that is off, down or has nothing to say costs the thread nothing, and
a person's rename (the thread menu, `PATCH /api/threads/{id}`) is final.

| What | Where |
|---|---|
| The settings | `ORCH_TITLE_MODEL` (unset: titles are off), `ORCH_MODEL_BASE_URL` (an OpenAI-compatible endpoint, with `/v1`), `ORCH_MODEL_API_KEY` (optional), `ORCH_MODEL_TIMEOUT_SECS` (20). [`compose.yaml`](../compose.yaml) sets the first two on the orchestrator (`mock-title` at `http://mock-model:8080/v1`); [`compose.live.yaml`](../compose.live.yaml) points them at your endpoint (`TITLE_MODEL`, else `MODEL`) |
| The script | `mock-title` on `mock-model`, [`wiremock/model/mappings/title.json`](wiremock/model/mappings/title.json): any conversation is titled `Mock thread title`; one that holds `[mock:untitled]` gets `NONE` (no topic yet); one that holds `[mock:title-error]` gets a 500 |
| The agent | the `chat` of [`agents/chat/`](agents/chat/agent/instructions.md) (it runs from the coder's image), which answers every first message |
| The scenario | `dev/title-e2e.sh`, `title` in `dev/e2e-all.sh`; it empties `mock-model`'s request journal first |

A title is only written for a thread whose first reply comes after the model was configured, and the web shows it as soon as the event reaches it (a replay of an old thread shows the title
it had at each point, so a title can appear in the middle of a replay). The mock is not the model: whether a real one writes a good title is for `compose.live.yaml` and a person to judge.

## The split profile: a control plane and two workers

The `split` profile runs what [ADR 0015](../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)
describes: the `orchestrator` service as a **control plane** (the API and the surfaces, no dispatcher) and two
**workers** (`orchestrator-worker-1` and `-2`, the dispatcher only), all on the one Postgres. They share nothing
else: a thread created through the control plane is `queued` until a worker claims its outbox row.

```sh
ORCHESTRATOR_ROLE=control-plane docker compose --profile app --profile split up -d --build --wait
dev/split-e2e.sh
docker compose --profile app --profile split down -v
```

`ORCHESTRATOR_ROLE` is the role of the `orchestrator` service (default `all`, as before); the workers are always
`worker`. Left at `all`, the orchestrator delivers threads itself and `dev/split-e2e.sh` fails saying so. The edge
and the web UI are unchanged: they talk to `orchestrator:8080`, the control plane. The workers publish nothing;
every process serves `GET /metrics` (the outbox queue, see
[`docs/orchestrator.md`](../docs/orchestrator.md#observability-and-scaling)), which the edge does not route, so read it
from inside the network: `docker compose exec -T edge wget -qO- http://orchestrator-worker-1:8080/metrics`.
Log lines start `role=worker instance=orchestrator-worker-1`.

**The handover.** `dev/split-e2e.sh` creates a thread for `mock-coder` with the keyword `slow` (an 8 s answer, see
[the scenarios](#mock-agent-scenarios)), waits for `working`, reads which worker holds the delegate row
(`SELECT lease_owner FROM outbox ...`), kills that container with `SIGKILL`, and checks that the thread still ends
`done`: the survivor claims the row once the 5 s lease has lapsed, resubscribes (the mock answers task not found) and
polls `GetTask`, which is `completed`. It then asserts the row was claimed twice and ended `delivered`, that the
thread has exactly one `thread_state: done` event and that the control plane's `/metrics` shows nothing due or
leased. It then checks **live text across the processes**: a thread with the keyword `stream` is held by the surviving worker while a viewer connected to the control plane reads the reply grow (at least three live deltas of one message before the log's message completes it, the deltas joined by offset are the final text, the message starts once), a viewer that reconnects about 2 s in with `Last-Event-ID: 2` reads it once too, and the export holds one `agent_message` with the reply's id and none that is not final ([ADR 0027](../docs/decisions/0027-live-text-relayed-not-stored.md)). The killed worker is started again when the script ends. It prints one `ok` or `FAIL` line per check, like
`coder-e2e.sh`, and needs `curl`, `jq` and `docker compose`; CI runs it at the end of the Coder E2E workflow.

```mermaid
sequenceDiagram
  actor U as split-e2e.sh
  participant E as edge
  participant C as orchestrator (control plane)
  participant P as postgres
  participant A as orchestrator-worker-1
  participant B as orchestrator-worker-2
  participant M as mock-agent
  U->>E: POST /agui/agents/mock-coder (text with slow)
  E->>C: with X-Auth-Request-Email
  C->>P: thread, events, outbox row (pending)
  A->>P: claim the row (lease 5 s, attempts 1)
  A->>M: SendStreamingMessage, an 8 s answer
  U->>E: GET /api/threads/id, until working
  U->>P: lease_owner of the inflight row is worker-1
  U->>A: docker compose kill -s SIGKILL
  Note over A: no goodbye, no lease release
  B->>P: claim the row once the lease lapsed (attempts 2)
  B->>M: SubscribeToTask (task not found), then GetTask
  M-->>B: completed
  B->>P: thread_state done, row delivered
  U->>E: GET /api/threads/id, done, and connect?mode=run holds one RUN_FINISHED
  U->>C: /metrics through edge: due 0, leased 0
  U->>A: docker compose up (started again)
```

```mermaid
stateDiagram-v2
  [*] --> Queued: control plane commits the thread and the outbox row
  Queued --> HeldByOne: a worker claims the row (attempts 1)
  HeldByOne --> Working: the agent answers, task known
  Working --> Orphaned: the holder is killed, its lease runs out
  Orphaned --> HeldBySurvivor: the other worker claims it (attempts 2)
  HeldBySurvivor --> Done: task completed, row delivered
  Done --> Verified: one done event, nothing due or leased
  Verified --> [*]
```

**Limits.** Only the mock agent is used, so the resubscribe falls back to polling: a real agent that supports
`SubscribeToTask` is covered by the binary's smoke tests instead. The workers have no healthcheck (the image has no
shell), so `up --wait` returns when they run, and the script waits for each `/metrics` itself. Both workers' logs are
in `docker compose logs`.

## Mock agent scenarios

Every stub requires a bearer token (any non-empty value; `dev-mock-token` is what the compose
orchestrator sends) and answers `401 Unauthorized` with a plain-text body without one, like an auth
proxy in front of a real agent. The agent card is public. The scenario of a new task is chosen by a
**whole word in the message text**, case-insensitive; when several appear, the first row of the
table wins.

| Keyword in the text | `SendStreamingMessage` answers with | Thread ends |
|---|---|---|
| `red-once` | `submitted`, `working`, artifacts `branch` and `checks` (failing, commit `1111111…`), `completed`; **with "this is attempt 2" or later in the text** (the gate's rework prompt, which carries the task and quotes the findings, so it still says `red-once`) the same with passing checks on commit `2222222…`. See [Verification](#verification-the-gate) | `done` under a gate, at attempt 2 |
| `red-always` | as the failing `red-once` (commit `3333333…`), on every attempt | `failed` under a gate, after 3 attempts |
| `error` | JSON-RPC error `-32602` (HTTP 200): a permanent rejection, no retry | `failed`, `error` event |
| `reject` | task `submitted`, then `rejected` with a message | `failed` |
| `fail` | `submitted`, `working`, `failed` with a message | `failed` |
| `ask` | `submitted`, `working`, `input-required` ("Which branch should I base the change on?") | `blocked` |
| `slow` | the default script, dribbled over 8 s in 16 chunks (frames split mid-line) | `done`, after 8 s |
| `push-flawed` | `submitted`, `working`, artifact `branch` (commit `aaaa…`, which `mock-verifier` finds fault with), `completed`; **with "this is attempt 2" or later and the heading "### the verifier" in the text** (the gate's rework prompt after the verifier's findings) the same on commit `bbbb…`, which it passes. See [Verifier](#verifier-the-verifier-agent-of-the-gate) | `done` under the verifier's gate, at attempt 2 |
| `push-clean` | as `push-flawed`, on commit `cccc…`, which `mock-verifier` passes | `done` under the verifier's gate, at attempt 1 |
| `steps` | `submitted`, `working`, then the work as four nested steps (`steps/v1`, [ADR 0025](../docs/decisions/0025-nested-steps-events-carry-their-source-path.md); the card lists the extension): a sub-agent `OpenCode` (`tool:c2`), a command `npm test` under it (`acp:c2:1`, parent `tool:c2`) that fails with the detail `1 failed`, the sub-agent's end, each as a `working` status whose message metadata holds the step, then `completed` ("Done."). See [`steps-v1.md`](../docs/api/steps-v1.md) | `done`, four `agent_step` events |
| `stream` | `submitted`, `working`, then a reply streamed as six chunks (`text-stream/v1`, [ADR 0027](../docs/decisions/0027-live-text-relayed-not-stored.md); the card lists the extension), dribbled over 6 s: artifact updates named `reply` whose metadata holds the byte `offset` of each piece, the last one `lastChunk`, then `completed` whose message states the whole text ("Streaming a reply, word by word, so the chat can show it grow.") under the stream id `<task>-reply`. See [`text-stream-v1.md`](../docs/api/text-stream-v1.md). The chat shows the words growing; the log holds one `agent_message`; `split-e2e.sh` reads it from the control plane while a worker holds the stream | `done`, one `agent_message` |
| none | `submitted`, `working`, artifact "Pull request" with the URL `https://github.com/example/sandbox/pull/1`, `completed` | `done` |

A **follow-up** message on an existing task (the message carries a `taskId`, which is what the
orchestrator sends when you answer a `blocked` thread) gets `working`, a new artifact and
`completed`; with the keyword `ask` again it asks another question ("One more question: should I
add tests?"), so multi-turn threads can be tried.

Other methods, whatever the text:

| Method | Answer |
|---|---|
| `SendMessage` | One task in the final state: `completed` with the PR artifact; `fail` gives `failed`, `ask` gives `input-required`. |
| `GetTask` | The task `params.id`, `completed`, with the PR artifact. |
| `CancelTask` | The task `params.id`, `canceled`. |
| `ListTasks` | An empty page (the orchestrator then treats the agent as unable to find a task by message). |
| `SubscribeToTask` | `-32001` task not found, as a real agent answers for a task that is not executing; the orchestrator falls back to `GetTask` polling. |
| anything else, for example the A2A 0.3 `message/send` | `-32601` method not found |

```mermaid
stateDiagram-v2
  [*] --> Received: POST /a2a with a bearer token
  [*] --> Unauthorized: no bearer token (401, plain text)
  Received --> FollowUp: message has a taskId
  Received --> Errored: keyword error
  Received --> Rejected: keyword reject
  Received --> Failed: keyword fail
  Received --> Asking: keyword ask
  Received --> Slow: keyword slow
  Received --> Steps: keyword steps
  Received --> Stream: keyword stream
  Received --> Completed: no keyword
  FollowUp --> Asking: keyword ask
  FollowUp --> Completed: any other text
  Asking --> [*]: stream ends, the task stays input-required
  Slow --> Completed: frames arrive over 8 s
  Steps --> Completed: four step statuses, then the words
  Stream --> Completed: six chunks over 6 s, then the whole text
  Completed --> [*]
  Failed --> [*]
  Rejected --> [*]
  Errored --> [*]
  Unauthorized --> [*]
```

### Verification (the gate)

`mock-coder-gated` in [`agents.yaml`](agents.yaml) is the same mock agent under
`gate: {require: [agent-checks]}` ([ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md)): an
agent that says `completed` is not done until the checks it reported pass. The mock reports them the way the
gate reads them: an artifact `branch` (`{repository, branch, commit}`) and an artifact `checks`
(`{passed, commit, summary, findings}`), both JSON in a data part, before it completes. The keywords choose what
the checks say:

- `red-once fix the login`: attempt 1 reports failing checks with one finding; the orchestrator sends the agent
  back (a `rework` event, then a **new A2A task in the same context** whose text starts "Your work did not pass
  verification (attempt 1 of 3); this is attempt 2", carries **the person's messages in their own words** (every one of the thread, in order, in a fenced
  block labelled `request`: each attempt is a new task, and an agent need not remember the one before) and quotes the finding as
  untrusted data); attempt 2 reports
  passing checks on another commit. The thread ends `done`, `job.attempt` 2.
- `red-always fix the login`: every attempt fails; after the third (`maxAttempts` 3 unless configured) the thread
  ends `failed` and the run ends with `RUN_ERROR` `code: "checks_failed"`.

```mermaid
sequenceDiagram
  actor U as Browser or dev/verify-e2e.sh
  participant O as orchestrator
  participant M as mock-agent (red-once)
  U->>O: POST /agui/agents/mock-coder-gated "red-once fix the login"
  O->>M: SendStreamingMessage (new task, context C)
  M-->>O: working, branch 1111111, checks failed, completed
  O-->>U: SUBAGENT_FINISHED, STATE_SNAPSHOT verifying, vymalo.check failed, vymalo.rework, SUBAGENT_STARTED
  O->>M: SendStreamingMessage (a new task in context C, "this is attempt 2" and the finding)
  M-->>O: working, branch 2222222, checks passed, completed
  O-->>U: vymalo.check passed, STATE_SNAPSHOT done, RUN_FINISHED success
```

```mermaid
stateDiagram-v2
  [*] --> Working: red-once or red-always
  Working --> Verifying: completed, checks reported
  Verifying --> Working: checks failed, attempts left (rework, attempt + 1)
  Verifying --> Done: checks passed (red-once, attempt 2)
  Verifying --> Failed: checks failed on the last attempt (red-always)
  Done --> [*]
  Failed --> [*]
```

**What the chat shows** (the web of the `app` profile at http://127.0.0.1:8080, pick **Mock coder (gated)** and send
`red-once fix the login`; MVP slice 4):

1. The pill reads **Checking the work…** as soon as the agent says `completed` (there is no attempt counter in the
   header: the attempts are in the divider). The composer stays open for drafting ("Send a follow-up…") and the
   button is **Stop**.
2. A card **Failed · Agent checks** appears in the conversation with the attempt, the short commit, the summary
   ("1 test failed") and the finding as plain text. Then a divider, **Attempt 2 of 3: sent back with 1 finding**,
   and the pill goes back to **Starting…** and **Working…**: the next attempt is a new
   subagent in the same run, its status lines and artifacts under the divider.
3. A second card, **Passed · Agent checks**, on the new commit; the pill ends **Done**.

`red-always fix the login` ends with three failed cards, two dividers, the pill **Failed** and
the notice **Checks failed after 3 attempts** (not "This thread is failed": the agent did its work and the work did
not pass). Findings are text from a tool: a finding with `<script>` or markdown shows those characters, and a long
one is cut with **Show more**. Reload the page mid-verification and the same pill and cards come back
(the page replays the log). The web's own mock (`pnpm dev:mock`, [`web/README.md`](../web/README.md#mock-server)) plays
the same story with `verify-red-once` and `verify-red`, and three scenarios for CI and the wait: `verify-ci` (the orchestrator's `ci` golden: a red `ci/build`, a rework, a green one), `verify-ci-stale` (a pending
card replaced by its answer, a late report of an older push and a stale answer shown apart) and `verify-wait` (stays Verifying until cancelled).

**A CI report is a card of its own** (`mock-coder-ci` or the coder, once a report has arrived, see [CI](#ci-the-gate-by-webhook)):
a badge with the conclusion in words and an icon (**Success**, **Failure**, **Cancelled**, **Timed out**, **Neutral**,
**Skipped**, **Action required**, **Stale**, **Startup failure**), the check's name, the short sha (the whole one on hover),
the branch, the provider and repository in small type, the summary as text (cut with **Show more** when long) and a
**View run** link that opens in a new tab, only when the report's URL is `http` or `https`. It comes before the
**Passed** or **Failed · CI** card that says what the gate made of it. The web's mock plays this with `verify-ci fix the login`:
a red `ci/build` report for the first commit, the agent sent back, a green report for the second.

`dev/verify-e2e.sh` drives all of it over AG-UI, like `try-thread.sh`, and asserts what a user sees: one run across
both attempts with two subagents; a `vymalo.check` that failed and one that passed; the `vymalo.rework`; the final
`STATE_SNAPSHOT` (`done`, attempt 2 of 3, gate `agent_checks`, the second commit) and the thread of the resource
API with the same `job`; `red-always` ending in `checks_failed` at attempt 3; a run that lowers the attempts with
`forwardedProps["vymalo.gate"] = {"maxAttempts": 2}`; and the four refusals (a 400 problem, no thread created) for
a run that removes the required source, asks for more attempts than `ORCH_MAX_ATTEMPTS_CAP`, requires `ci` where no check is named
(`ci.required`) or requires `verifier` where no verifier agent is configured. CI runs it in the `Coder E2E` workflow. `dev/check-mocks.sh` checks the mock's side
(the artifacts and how the rework prompt changes the answer) on its own.

To gate every agent instead of one, set `ORCH_GATE=agent-checks` on the `orchestrator` service; the mocks that
report no `checks`, or no `branch` (the checks count only on the commit the agent pushed, [ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md#status-note-2026-09-30-the-agents-checks-need-a-pushed-commit)),
would then be sent back three times and fail, which is the fail-closed reading of "no checks reported" and of "no pushed commit".

### Verifier (the verifier agent of the gate)

`mock-coder-verified` in [`agents.yaml`](agents.yaml) is the same mock agent under
`gate: {require: [verifier], verifier: verifier}` ([ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md),
slice 10): an agent that says `completed` is not done until **another agent**, `verifier` (the `mock-verifier` service),
has passed the commit it pushed. The mock coder reports only a `branch` artifact (`{repository, branch, commit}`), like an
agent that leaves the checking to someone else; the keywords choose the commit, and `mock-verifier` judges by it:

- `push-flawed fix the login`: attempt 1 pushes commit `aaaa…`; the verifier answers with a `verdict` whose `passed` is
  false and one finding; the orchestrator sends the coder back (a `rework` event, then a **new A2A task in the same
  context** whose text starts "Your work did not pass verification (attempt 1 of 3); this is attempt 2", has the heading
  "### the verifier" and quotes the finding as untrusted data); attempt 2 pushes `bbbb…`, which the verifier passes. The
  thread ends `done`, `job.attempt` 2, and the chat shows the verifier as a subagent of its own, twice.
- `push-clean fix the login`: pushes `cccc…`, passed at once, attempt 1.
- `push-flawed` with `forwardedProps["vymalo.gate"] = {"maxAttempts": 1}`: the findings are final, `RUN_ERROR`
  `checks_failed`.

```mermaid
sequenceDiagram
  actor U as Browser or dev/verifier-e2e.sh
  participant O as orchestrator
  participant M as mock-agent (push-flawed)
  participant V as mock-verifier
  U->>O: POST /agui/agents/mock-coder-verified "push-flawed fix the login"
  O->>M: SendStreamingMessage (new task, context C)
  M-->>O: working, branch aaaa, completed
  O-->>U: SUBAGENT_FINISHED, STATE_SNAPSHOT verifying, SUBAGENT_STARTED verifier, vymalo.check pending
  O->>V: SendStreamingMessage (context C-verify-1-1, "Check that commit aaaa... ", the task and the summary quoted)
  V-->>O: working, artifact verdict (passed false, one finding), completed
  O-->>U: vymalo.check failed, SUBAGENT_FINISHED verifier, vymalo.rework, SUBAGENT_STARTED
  O->>M: SendStreamingMessage (a new task in context C, "this is attempt 2", "### the verifier" and the finding)
  M-->>O: working, branch bbbb, completed
  O->>V: SendStreamingMessage (context C-verify-2-2, "Check that commit bbbb... ")
  V-->>O: working, artifact verdict (passed true), completed
  O-->>U: vymalo.check passed, STATE_SNAPSHOT done, RUN_FINISHED success
```

```mermaid
stateDiagram-v2
  [*] --> Working: push-flawed or push-clean
  Working --> Verifying: completed, branch pushed
  Verifying --> Done: the verifier passed the commit (cccc, or bbbb after a rework)
  Verifying --> Working: the verifier found fault, attempts left (rework, attempt + 1)
  Verifying --> Failed: the verifier found fault on the last attempt (maxAttempts 1)
  Done --> [*]
  Failed --> [*]
```

`mock-verifier` (`wiremock/verifier/`) matches the request the dispatcher really sends: a JSON-RPC `SendStreamingMessage` whose
`params.message.contextId` is the verification context, `messageId` the outbox row and `parts[0].text` the prompt, which says
"Check that commit `<sha>`". A `commit aaaa…` (forty `a`) gets the failing `verdict` and any other commit the passing one;
`GetTask` answers *task not found* on purpose, so a verification whose stream breaks is held (`blocked`) rather than passed, and
`SubscribeToTask`, `CancelTask` and `ListTasks` answer like the other mocks.

`dev/verifier-e2e.sh` drives all of it over AG-UI and asserts what a user sees: one run across both attempts with four
subagents (the coder twice, the verifier twice, named after it); the verifier's cards (pending and failed at attempt 1,
pending and passed at attempt 2) and the finding; the `vymalo.rework`; the final `STATE_SNAPSHOT` (`done`, attempt 2 of 3, gate
`verifier`, the second commit) and the thread of the resource API with the same `job`; `maxAttempts: 1` ending in
`checks_failed` with the finding in the message; `push-clean` done at attempt 1 with no rework; **what the verifier was
sent**, read from the mock's own request journal (`/__admin/requests`: the contexts `<thread>-verify-1-1` and
`<thread>-verify-2-2`, never the thread's own, the commits, the attempt and the task quoted as untrusted); and the three refusals
(a 400 problem, no thread created) for a run that chooses another verifier, drops the required source or requires `ci` where no check is named (`ci.required`). CI runs it
in the `Coder E2E` workflow. `dev/check-mocks.sh` checks the mocks' side (the `verdict` for each commit, the coder's commits, and
how the rework prompt changes the coder's answer) on its own.

To make every agent need a verifier, set `ORCH_GATE=verifier` and `ORCH_VERIFIER=verifier` on the `orchestrator` service and give
the `verifier` entry a `gate: {require: []}`: an agent cannot verify its own work, and the orchestrator refuses to start when one
would (exit 78, naming the agent and the fix). Mocks that push no `branch` would then be sent back three times and fail, which is
the fail-closed reading of "no pushed commit".

### CI (the gate, by webhook)

`mock-coder-ci` in [`agents.yaml`](agents.yaml) is the same mock agent under `gate: {require: [ci], ci: {required: [ci/build]}}`
([ADR 0017](../docs/decisions/0017-ci-results-by-webhook.md), [ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md)):
an agent that says `completed` is not done until a **signed CI report about the commit it pushed** arrives at
`POST /webhooks/ci`, and **named `ci/build`**: a gate that requires CI names the checks that count (`ci.required`), and a
report of any other name is a card and nothing else (there is no "first report decides": it let a red commit pass on
a `skipped` report of another check). The orchestrator serves that route because compose sets
`ORCH_SURFACES=agui,mcp,thread-tools,webhook-generic,webhook-github` and `WEBHOOK_GENERIC_SECRETS=dev-webhook-secret-0123456789abcdef0123`
on it (a secret is at least 32 bytes; a process that mounts no webhook refuses `ci` and exits 78 if a gate requires it); the edge passes `/webhooks/*` on **without an identity**
(`header_up -X-Auth-Request-Email` in the [Caddyfile](Caddyfile): a webhook is a machine route, authenticated by its
signature and nothing else). The mock pushes a `branch` artifact and completes; the job then waits for CI, at most
`ORCH_CI_TIMEOUT_SECS` (3600), after which the thread is blocked with `ci_timeout` and no attempt is used.

[`ci-webhook.sh`](ci-webhook.sh) plays the CI system: it builds the body, signs `"<timestamp>.<body>"` with `openssl`
and posts it (`--help` lists the options; see [`docs/api/webhooks.md`](../docs/api/webhooks.md) for the contract):

```sh
dev/ci-webhook.sh --sha 1111111111111111111111111111111111111111 --conclusion failure \
                  --summary 'red-once: tests::login fails: expected 200, got 500'
dev/ci-webhook.sh --sha 1111111111111111111111111111111111111111 --secret wrong --expect 401   # refused
dev/ci-e2e.sh                                                                    # the whole story, asserted
```

`dev/ci-e2e.sh` runs `red-once fix the login` on `mock-coder-ci` and asserts: a wrong secret and a stale timestamp are
401; the thread is `verifying` with the gate `ci` and the sha `1111111`; a red report for that commit sends the agent
back (attempt 2, which pushes `2222222`); a report about the old commit changes nothing; a green report for the new
commit ends the job `done` at attempt 2 of 3, and the run ends `RUN_FINISHED` (success); a `skipped` report of a check the
gate does not name (`docs`) does not decide, even for the new commit; the same report again (same timestamp and body,
another delivery id) is accepted twice and counted once; and that the chat shows a `vymalo.ci` card per report, each with
an id of its own and none replacing another (the conclusion, the short sha, the link and the summary). The rework prompt quotes the report's summary, and the mock picks its answer by keyword, so the red
report's summary keeps `red-once`. The mock pushes the same two commits every time and a commit is watched by the first
job that pushed it, so the script passes once per database (`docker compose down -v` to run it again; a second run is recognised and reported as a skip, exit status 77, `CI_E2E_FORCE=1` runs it anyway). CI runs it in
the `Coder E2E` workflow.

**The GitHub shape and `mock-ci`.** `ci-webhook.sh --shape github [--event check_run|workflow_run] [--fork]` posts what
GitHub would (`X-GitHub-Event`, `X-GitHub-Delivery`, `X-Hub-Signature-256` over the raw body; `--ping` sends the `ping` of a
new webhook, answered 204) to `POST /webhooks/github`; `--fork` makes the run one of a fork's code, which is acknowledged and
not stored. `mock-ci` does the same on its own: every 2 s it lists the `agent/*`
branches of `local/sandbox` on `git-server`, and for each commit it has not reported it posts a `check_run` named
`mock-ci/build` through the edge, `success`, or `failure` when the commit message contains `CI_FAIL`. A commit is reported once
(a marker file); the orchestrator's key for the delivery is made of the check run's id and completion time, so a restart that
forgets what it did posts again with a new completion time, which is a new report. It reports
the repository as `http://git-server:8080/local/sandbox`, the address the coder's `branch` artifact names, because the orchestrator
matches a report to a job by `host/owner/name` and the commit. The **coder is gated on its own checks and on CI** (`gate: {require: [agent-checks, ci], ci: {required: [mock-ci/build]}}` in
`agents.yaml`), so `dev/coder-e2e.sh` also asserts the job's gate and one `vymalo.ci` card, `mock-ci/build`, `success`, on the pushed commit
(when the thread does not end `done`, it prints the tail of the orchestrator's and `mock-ci`'s logs, whose info lines
`watching for the CI reports of a pushed commit` and `a CI report will be matched to the job that watches this key` carry the same
`watch_key` from both sides); in
the chat, send the coder a task and the card appears when `mock-ci` has reported. `MOCK_CI_SHAPE=generic` (or `github-workflow`) `docker compose --profile app up -d mock-ci`
makes it use the generic route instead.

```mermaid
sequenceDiagram
  participant C as coder
  participant G as git-server
  participant M as mock-ci
  participant E as edge
  participant O as orchestrator
  C->>G: push agent/run-prefix
  C-->>O: branch artifact (repository, branch, commit): the watch on the commit
  M->>G: git ls-remote --heads agent/*, then fetch the tip's message
  M->>E: POST /webhooks/github check_run completed, signed (success, or failure on CI_FAIL)
  E->>O: no identity header
  O-->>M: 202, stored (parked if the watch is not there yet)
  C-->>O: completed
  O-->>O: the report is applied: done on success, rework on failure
```

```mermaid
stateDiagram-v2
  [*] --> Polling: every MOCK_CI_POLL_SECS
  Polling --> Seen: the commit is already reported (marker file)
  Polling --> Posting: a new commit on agent/*
  Posting --> Reported: 2xx from the webhook
  Posting --> Polling: any other answer, tried again on the next pass
  Reported --> Polling
  Seen --> Polling
```

```mermaid
sequenceDiagram
  actor U as dev/ci-e2e.sh
  participant E as edge
  participant O as orchestrator
  participant M as mock-agent (red-once)
  U->>E: POST /agui/agents/mock-coder-ci "red-once fix the login"
  E->>O: with X-Auth-Request-Email
  O->>M: SendStreamingMessage
  M-->>O: branch 1111111, checks, completed
  O-->>U: thread verifying (the gate is ci), the run stays open
  U->>E: POST /webhooks/ci failure for 1111111, signed (dev/ci-webhook.sh)
  E->>O: no identity header
  O-->>U: 202, stored in the inbox
  O->>M: a new task in the same context, "this is attempt 2" and the report as a finding
  M-->>O: branch 2222222, completed
  U->>E: POST /webhooks/ci success for 1111111 (old): 202, changes nothing
  U->>E: POST /webhooks/ci success for 2222222, twice with one delivery id
  O-->>U: thread done at attempt 2 of 3, RUN_FINISHED success
```

```mermaid
stateDiagram-v2
  [*] --> Working: red-once, mock-coder-ci
  Working --> Verifying: completed, commit pushed, CI required
  Verifying --> Working: a red report for the pushed commit (attempt + 1)
  Verifying --> Verifying: a report for another commit, or one already counted
  Verifying --> Done: a green report for the pushed commit
  Verifying --> Blocked: no report within ORCH_CI_TIMEOUT_SECS (no attempt used)
  Done --> [*]
```

### Release channels (`mock-agent-releases`)

Its card declares `https://agents.vymalo.com/a2a/extensions/release-channels/v1` with the example of
the extension contract: default channel `production`; channels `production` = `coder-r47`, `staging` =
`coder-r51`, `latest` = `coder-r53`; revisions `coder-r53`, `coder-r51`, `coder-r47`. The UI's
release dropdown appears for it, and only for it. The scenario keywords above work the same.

On a new task the mock reads `params.message.metadata[<extension URI>].release`, resolves it and
echoes `{"requested": …, "revision": …}` in the task metadata of every event (no release means the
default channel, echoed as `requested: production`). The thread's events then carry the revision
(`actor.revision`). A `release` that is neither a channel nor a listed revision **fails the task**,
naming the value; it never falls back to the default. (The orchestrator already refuses such a thread
with a 400 before it reaches the agent, so the failure is only reachable by calling the mock directly.)

### Limits

- **No state.** Ids are derived from the request: the task id is `task-<messageId>`, message ids
  are built from it, the context id is the request's. `GetTask` therefore returns `completed` for any
  id, and a task's release metadata is echoed only on its first turn.
- **JSON-RPC ids are echoed as strings**, which is what the A2A SDK the orchestrator uses sends
  (UUIDv7). A client that sends numeric ids gets them back quoted.
- **Text is not echoed.** Putting request text into a JSON body without escaping could produce
  invalid JSON, so the answers are fixed sentences.
- Only the JSON-RPC binding is offered (`supportedInterfaces` lists `JSONRPC`, version `1.0`).

## Mock web search (MCP)

`mock-mcp-search` is an MCP server that answers canned web-search results, so that an agent that searches the
web, and a web search attached to a chat, run offline and give the same answer every time. It is
[`mock-mcp-search/server.mjs`](mock-mcp-search/server.mjs), about 250 lines of Node with no dependencies, in a
`node:24-alpine3.23` image (profile `app`, host port 8096, `MOCK_MCP_SEARCH_PORT`).

| | |
|---|---|
| Endpoint | `POST http://mock-mcp-search:8080/mcp` from the other containers, `http://127.0.0.1:8096/mcp` from the host |
| Protocol | MCP 2025-11-25 over streamable HTTP, without sessions: one JSON answer per POST (never an SSE stream), no `Mcp-Session-Id`; `GET` and `DELETE` answer 405, as the transport spec has a server do that offers no SSE stream. Clients of 2025-06-18 and 2025-03-26 are answered too |
| Authentication | `Authorization: Bearer dev-search-token` (`MOCK_MCP_TOKEN` in [`compose.yaml`](../compose.yaml)); 401 without it. `/healthz` and `/__journal` take none |
| Tool | `web_search { query: string }`, with a title, an `annotations` hint (read-only) and `icons`: one `data:image/svg+xml;base64,…` entry (a magnifying glass, under 400 bytes), which is what the UI shows on the step of a call |
| Answer | One `text` content: `1. <title> — <url>` and the snippet on the next line, one entry per result |
| Keywords | The first keyword of [`results.json`](mock-mcp-search/results.json) (in file order, case-insensitive) that the query contains picks the list (`world cup`: the 2014 final; `rust`; `async`: three sources on async programming, what the [`[mock:cards]`](#cards-and-mermaid-the-researcher-answers-with-cards-and-a-graph) script searches for); any other query gets `default`: `https://example.org/mock-search/1` and `/2` |
| Scenarios | `[mock:empty]` in the query answers `No results.`; `[mock:error]` answers a tool execution error (`isError: true`); a missing or empty `query` is one too, not a protocol error (the spec's way to let a model correct itself); an unknown tool is `-32602` |
| Journal | `GET /__journal` lists the calls of the tool, `{"calls": [{"tool", "arguments", "at"}]}` (a call refused with 401 is not in it; the last 1000 are kept); `DELETE /__journal` empties it |

```mermaid
sequenceDiagram
  participant C as MCP client (an agent, or the orchestrator's relay)
  participant S as mock-mcp-search
  C->>S: POST /mcp initialize (Bearer token, Accept: application/json, text/event-stream)
  S-->>C: 200 JSON: the version it speaks, the tools capability, serverInfo with an icon (no session id)
  C->>S: POST /mcp notifications/initialized
  S-->>C: 202, no body
  C->>S: POST /mcp tools/list (MCP-Protocol-Version: 2025-11-25)
  S-->>C: web_search, with its input schema and its icon
  C->>S: POST /mcp tools/call web_search {query}
  S-->>C: 200 JSON: the canned results (or No results., or isError)
  Note over S: the call is appended to the journal
```

The server keeps no state apart from the journal, so there is no lifecycle to draw.

**Changing the canned results.** Edit [`mock-mcp-search/results.json`](mock-mcp-search/results.json): `default` is the
list for a query that matches no keyword, `keywords` maps a keyword (lower case, matched as a substring of the
lower-cased query, first in file order wins) to a list; an entry is `{"title", "url", "snippet"}`, all three non-empty.
A malformed file stops the server at startup, with the reason in `docker compose logs mock-mcp-search`. The file is
copied into the image, so apply an edit with `docker compose --profile app up -d --build mock-mcp-search` (seconds).
Do not put a URL under `https://example.org/mock-search/` there if a scenario counts on that prefix to tell the mock's
sources from others.

**Checking it.** `dev/check-agent-mocks.sh` plays the handshake and every behaviour above over HTTP with `curl` and `jq`
and **empties the journal** (before and after); `node --test dev/mock-mcp-search/server.test.mjs` runs the same
behaviours against the server in process (CI: workflow Compose, jobs `mocks` and `scripts`).

**An agent uses it** through an `mcp.json` of its folder, the way adam-rs reads it (the stack's [researcher](#several-agents) does,
with [`agents/researcher/agent/mcp.json`](agents/researcher/agent/mcp.json)):

```json
{"mcpServers": {"search": {"type": "http", "url": "http://mock-mcp-search:8080/mcp",
  "headers": {"Authorization": "Bearer ${SEARCH_MCP_TOKEN}"}, "tools": ["web_search"]}}}
```

The agent sees the tool as `search__web_search`. The deployment of the agent needs `MCP_ALLOW_INSECURE=true` (the URL is
`http`, and the token goes over it: development only).

## Driving it

```sh
docker compose up -d --wait
dev/check-mocks.sh                                # one call per scenario (curl, jq)
```

With the `app` profile up, or any orchestrator that serves the AG-UI routes (the default):

```sh
dev/try-thread.sh "add a health endpoint"                        # done
dev/try-thread.sh "ask me which branch"                          # blocked, prints the thread id
THREAD_ID=<id> dev/try-thread.sh "use main"                      # the answer: done
dev/try-thread.sh "fail please"                                  # failed
AGENT_ID=mock-coder-releases RELEASE=staging dev/try-thread.sh "ship it"   # events carry [coder-r51]
AGENT_ID=mock-coder-gated dev/try-thread.sh "red-once fix the login"       # checks fail, sent back, done at attempt 2
dev/verify-e2e.sh                                                          # asserts that, red-always and the refusals
AGENT_ID=mock-coder-verified dev/try-thread.sh "push-flawed fix the login" # the verifier finds fault, sent back, done at attempt 2
dev/verifier-e2e.sh                                                        # asserts that, what the verifier was sent, and the refusals
dev/ci-e2e.sh                                                              # mock-coder-ci: a signed CI report sends it back, then ends the job
dev/e2e-all.sh                                                             # all of the scripts above and the coder's, then a summary
```

The script runs the thread with `POST /agui/agents/{agentId}` (a UUID it mints as the thread id; `THREAD_ID`
sends a follow-up run to an existing thread) and prints one line per AG-UI frame of the thread (number,
event type, detail; `[coder-r51]` marks the revision that answered), read from
`GET /agui/threads/{id}/connect?mode=run`. It exits 0 for `done` and `blocked`.
`BASE_URL` (default `http://127.0.0.1:8080`, the edge) and `AUTH_EMAIL` point it elsewhere. Its default
target is `mock-coder`, not the default agent: the real coder is driven by
[`coder-e2e.sh`](#the-default-agent).

### The orchestrator on the host

```sh
docker compose up -d --wait                       # postgres + mocks only
cd orchestrator
MOCK_AGENT_TOKEN=dev-mock-token \
DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch \
AGENTS_FILE=../dev/agents.local.yaml \
AUTH_DEV_USER=dev@example.com \
LOG_FORMAT=text \
LISTEN_ADDR=127.0.0.1:8090 \
  cargo run -p orchestrator
BASE_URL=http://127.0.0.1:8090 ../dev/try-thread.sh "add a health endpoint"
```

For the UI on top of it, `MOCK_API_ORIGIN=http://127.0.0.1:8090 pnpm dev` in `web/` (that variable is
the dev rewrite of `/api/*`, whatever serves it). `pnpm dev:mock` is the web app's own contract mock
and needs none of this.

### An agent inside the orchestrator (`agent-local`)

The orchestrator can host an agent in its own process ([ADR 0015](../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)),
behind the Cargo feature `agent-local`, which is off in the default build and in the compose image. [`agents.local-echo.yaml`](agents.local-echo.yaml)
lists one, `echo`, which repeats your message back, so the loop (AG-UI, dispatcher, a durable run in Postgres) runs with no mock and
no agent card:

```sh
docker compose up -d --wait postgres
cd orchestrator
DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch \
AGENTS_FILE=../dev/agents.local-echo.yaml \
AUTH_DEV_USER=dev@example.com \
LOG_FORMAT=text \
LISTEN_ADDR=127.0.0.1:8090 \
  cargo run -p orchestrator --features agent-local
BASE_URL=http://127.0.0.1:8090 AGENT_ID=echo ../dev/try-thread.sh "hello"
psql postgres://postgres:postgres@localhost:5432/orch -c 'select id, agent, status from orch_agent_runs'
```

Without `--features agent-local` the same file stops the orchestrator at startup with exit code 78 and a message naming the feature.
To run the image with the feature, the compose profile `local-agent` builds it (`ORCH_FEATURES=agent-local`, a long build) beside a Postgres of its own and serves it on
http://127.0.0.1:8095 with `AUTH_DEV_USER=dev@example.com` and no web UI:

```sh
docker compose --profile local-agent up -d --build --wait
BASE_URL=http://127.0.0.1:8095 AGENT_ID=echo dev/try-thread.sh "hello"
```

(*unverified*: no Docker daemon was available when this was written; the same binary was run on the host as above.) It is not part of `app`: it shares nothing with the `orchestrator` service.

### The Rust test against the mocks

`orchestrator/crates/e2e/tests/wiremock_agent.rs` runs the real dispatcher and A2A adapter (driven over the AG-UI run route of the test instance: `Chat::create_thread`, `Chat::follow_up`)
against the mocks: the default script, `ask` and its answer, `fail`, `error` and `reject`, cancelling
a blocked thread, the release echo, the verification scenarios (`red-once` sent back and done at attempt 2,
`red-always` failed after three; through `AppConfig.target_gates`, as `dev/agents.yaml` gates `mock-coder-gated`) and the
verifier's (`push-flawed` judged by `mock-verifier`, sent back and done at attempt 2, `push-clean` done at once; as
`dev/agents.yaml` gates `mock-coder-verified`), so that what the orchestrator really sends a verifier is matched against the WireMock mappings.
It skips unless told where the mocks are:

```sh
docker compose up -d --wait mock-agent mock-agent-releases mock-verifier
cd orchestrator
ORCH_TEST_MOCK_AGENT_URL=http://127.0.0.1:8081 \
ORCH_TEST_MOCK_AGENT_RELEASES_URL=http://127.0.0.1:8082 \
ORCH_TEST_MOCK_VERIFIER_URL=http://127.0.0.1:8083 \
  cargo test -p orch-e2e --test wiremock_agent
```

`cargo test --workspace` against the compose database works too:
`ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test`.

## Changing a mock

(The mock web search is not WireMock: [Mock web search (MCP)](#mock-web-search-mcp). The model of the chat and the researcher is: edit
`wiremock/model/mappings/*.json` and `docker compose --profile app restart mock-model`; `dev/check-agent-mocks.sh` says whether it still answers as documented.) Stubs are files: `wiremock/<mock>/mappings/*.json` (matching and response settings, one stub per file,
lower `priority` wins) and `wiremock/<mock>/__files/*` (bodies; JSON-RPC frames use Handlebars
templates, see WireMock's response templating). The three mocks are separate directories so each can
diverge; a change to a shared behaviour goes into all of them. The directories are mounted read-only, so
after editing run `docker compose restart mock-agent mock-agent-releases mock-verifier`, or reload the stubs with
`curl -X POST http://127.0.0.1:8081/__admin/mappings/reset`. To see what a client actually sent:
`curl http://127.0.0.1:8081/__admin/requests` and, for requests no stub matched,
`/__admin/requests/unmatched`.

## What was checked

*Verified 2026-09-29* against the `wiremock-standalone-3.13.2.jar` (the version compose pins, run
with the same `--global-response-templating` flag and the same directories): every scenario above by
`dev/check-mocks.sh`; the real `orchestrator` binary (debug build) on Postgres 16 driven with
`try-thread.sh` for the default, `ask` plus its answer, `fail`, `error`, `reject`, `slow`, the three
release selections and cancel; `wiremock_agent.rs` (6 tests); `dev/Caddyfile` with `caddy validate`
and `caddy run` from the 2.11.4 release (client-supplied `X-Auth-Request-Email` replaced, SSE passing
through); `docker compose config` for both profiles; the image tags exist on Docker Hub.

The verification scenarios (`red-once`, `red-always`, `mock-coder-gated`, `dev/verify-e2e.sh`):

*Verified 2026-09-30*: `dev/verify-e2e.sh` against the real `orchestrator` binary (debug build, `ORCH_SURFACES=agui`) on
Postgres 16, with an agents file holding the `mock-coder-gated` entry of `dev/agents.local.yaml`, and a stand-in that reads the mappings and files
of `dev/wiremock/agent` (the card, the priorities, the body patterns and the four template variables of the
`red-*` streams): every check printed `ok`, exit 0. `dev/check-mocks.sh`'s verification section the same way.
`shellcheck dev/*.sh` and `docker compose --profile '*' config -q` are clean. This proves the script's `jq` paths, the
orchestrator's side of the gate and the mappings' matching logic as the stand-in implements it, not WireMock itself.

*Unverified*: `red-once` and `red-always` running in the `wiremock/wiremock:3.13.2` image (the regular expressions of
the body patterns and the templates are the ones the other stubs use); the first run is the `Compose` workflow
(`check-mocks.sh`, `wiremock_agent.rs`) and the `Coder E2E` workflow (`verify-e2e.sh`).

The verifier (`mock-verifier`, `push-flawed`, `push-clean`, `mock-coder-verified`, `dev/verifier-e2e.sh`):

*Verified 2026-09-30*: the `wiremock-standalone-3.13.2.jar` (the version compose pins, run with
`--global-response-templating --disable-banner` and the directories of `dev/wiremock/agent`, `agent-releases` and `verifier`, on three
ports) against `dev/check-mocks.sh` (every check `ok`, including the verifier's section and the coder's new keywords); the real
`orchestrator` binary (debug build) on Postgres 16, with `dev/agents.local.yaml` moved to those ports, against `dev/verifier-e2e.sh`
(every check `ok`, exit 0, including the request journal of the mock verifier) and `dev/verify-e2e.sh` (still `ok`);
`wiremock_agent.rs` with the three URLs set (10 tests). `shellcheck dev/*.sh dev/coder/*.sh` and `docker compose --profile '*' config -q`
are clean. This is WireMock itself, not its container image.

*Unverified*: the `mock-verifier` service running in the `wiremock/wiremock:3.13.2` image and the healthcheck of its service, and
the `Coder E2E` step that runs `dev/verifier-e2e.sh` inside the `app` profile; the first runs are the `Compose` and `Coder E2E`
workflows.

*Unverified*: `docker compose up` itself, that is the containers, the image healthchecks, the two
image builds and the WireMock image's argument handling. The machine that wrote this had no Docker
daemon. CI (`.github/workflows/compose.yml`) starts the default profile and runs the same checks;
the `app` profile is only parsed there, and run by the `Coder E2E` workflow (below).

The MCP server (`dev/mcp-e2e.sh`, `dev/mcp-tokens.yaml`, the `@mcp` block of the `dev/Caddyfile`):

*Verified 2026-09-30*: `dev/mcp-e2e.sh`, including the `wait_for_job` checks (every check `ok`, exit 0) against the real `orchestrator` debug binary (default
features, `ORCH_SURFACES=agui,mcp`) on Postgres 16, `dev/wiremock/agent` served by `wiremock-standalone-3.13.2.jar`, and Caddy
2.11.4 running the `dev/Caddyfile` with the two upstream addresses and the port changed (the `@mcp` block, `header_up
-X-Auth-Request-Email` and `flush_interval -1` as committed); `caddy validate` accepts the Caddyfile; `docker compose config`
accepts every profile; `shellcheck dev/*.sh` is clean. *Unverified*: `docker compose up` itself (no Docker daemon here);
Claude Code and opencode against this server (neither was run; only `curl` and rmcp's own client were).

The CI webhook (`mock-coder-ci`, `dev/ci-webhook.sh`, `dev/ci-e2e.sh`):

*Verified 2026-09-30* (re-run after the review fixes, with `ci.required: [ci/build]`, the two webhooks and 32-byte secrets): `dev/ci-e2e.sh` against the real `orchestrator` binary (debug build, `ORCH_SURFACES=agui,webhook-generic,webhook-github`,
`WEBHOOK_GENERIC_SECRETS=dev-webhook-secret-0123456789abcdef0123`) on Postgres 16, with an agents file holding the `mock-coder-ci` entry of `dev/agents.yaml`
and a stand-in that reads the mappings and files of `dev/wiremock/agent` (as for the verification scenarios above): every check
printed `ok`, exit 0, including the signatures made by `dev/ci-webhook.sh` with `openssl` and checked by the Rust route. `shellcheck dev/*.sh`
and `docker compose --profile '*' config -q` are clean. This proves the scripts' `jq` paths, the signing and the orchestrator's side; it does not prove
Caddy's `header_up -X-Auth-Request-Email` (the syntax is Caddy's documented delete form; the webhooks never read the header either way) or
WireMock itself.

*Unverified*: the `Coder E2E` workflow running `dev/ci-e2e.sh` in containers (the machine that wrote this had no Docker daemon).

*Verified 2026-09-30* (GitHub webhook, `mock-ci`, `ci-webhook.sh --shape github`, re-run after the review fixes): `dev/mock-ci/mock-ci.sh` (`MOCK_CI_ONCE=1`, against a bare repository
through `file://` in place of `git-server`, and the real `orchestrator` binary with both webhooks mounted): for the shapes `github` (a `check_run`),
`github-workflow` (a `workflow_run`) and `generic` the branches `agent/*` were reported once each (`success`, and `failure` for a commit whose
message contains `CI_FAIL`), every post was `202`, and the stored rows are keyed `check_run:<id>:<completed_at>`, `workflow_run:<id>:<attempt>` and the
digest of the signed string. The hand-made HMAC of `mock-ci` (no secret on any command line) matches `openssl dgst -hmac` for keys shorter than,
as long as and longer than a SHA-256 block, under dash and bash (`sh` of Alpine is busybox `ash`, with busybox `od` and `awk`: *unverified*).
`ci-webhook.sh --shape github` for a `ping` (204), the two events and a bad signature (401). `shellcheck` is clean. The base image tag and digest were read from the Docker Hub
registry API. *Unverified*: the image build, `mock-ci` running in the compose network, **the coder's `branch` artifact naming the repository the way
`mock-ci` reports it** (read in `adam-coder`'s `publish.rs` at `882e239`: `repository` is `wt.repo().url`), and the coder job ending `done` through it: the first
run of all of it is the `Coder E2E` workflow.

The default agent (the `coder` service, its mocks and `dev/coder-e2e.sh`):

*Verified 2026-09-29*:

- The vendored files are byte-identical to `vymalo/another-adam-rs` at `0e08fe07eaea` (the scripted mocks landed in `22d7c6e126b8` and did not change since) (`dev/coder/check-vendored.sh`, over
  `raw.githubusercontent.com`; the same script also asserts the image pin, see below).
- The vendored WireMock mappings load in `wiremock-standalone-3.13.2.jar` (16 mappings for `mock-openai`, 13 for
  `mock-github`, run with `--global-response-templating`), and the whole `mock-coder` script, the `mock-opencode` turns
  and the pull request creation answer as scripted when driven turn by turn with the histories a client sends.
- `dev/coder-e2e.sh`, both variants, against those two WireMock instances, a bare git repository seeded from the
  vendored `seed/` and a stand-in chat API that ran the script and pushed the branch: every check printed `ok`, exit 0;
  with a thread that ended `failed` it printed `FAIL` lines and exited 1; with the API down it failed at the first call.
  This proves the script's jq paths and journal queries, not the real stack.
- The scripts on AG-UI (2026-09-29, after the chat API went off by default): `dev/try-thread.sh` (a new thread, a
  blocked one and its answer with `THREAD_ID`) and the AG-UI half of `dev/coder-e2e.sh` (the run stream ends
  with `RUN_FINISHED`, the thread ends `done`, its frames are fetched with `connect?mode=run`; the artifact `jq`
  paths on hand-written frames) against the real `orchestrator` binary at its default `ORCH_SURFACES` (`agui`), the
  `orch-fake-agent` and a real Postgres; the background run and the frame count of `dev/split-e2e.sh` the same
  way. This proves the AG-UI calls and the `jq` paths, not the compose stack: the mock and git checks of
  `coder-e2e.sh` and the docker steps of `split-e2e.sh` still run only in CI.
- `docker compose config` (Compose v5.1.1, no daemon) accepts both profiles; `shellcheck dev/*.sh dev/coder/*.sh` and
  `actionlint` are clean; the docs check passes.
- The ghcr manifest and digest of `coder:sha-0e08fe0` (anonymous token, `docker-content-digest`), and the tag list of
  the repository. adam-rs's `coder` workflow built, smoke-tested and ran its own compose e2e (both variants) on this
  image before pushing it.

The split profile (`orchestrator-worker-*`, `dev/split-e2e.sh`):

*Verified 2026-09-29*: `docker compose config` (Compose v5.1.1, no daemon) accepts every combination of the `app`
and `split` profiles, and the resolved `orchestrator` service differs from the earlier one only by `ORCH_ROLE=all`;
`shellcheck` and `actionlint` are clean; `dev/split-e2e.sh` against a stand-in `docker` and a stand-in chat API over a
real Postgres, in the passing case and with the wrong owner, a missing `/metrics`, a second done event and a wrong
attempt count (this proves the script's control flow, its queries and its `jq` paths, not the stack); the binary's smoke
test with one control plane and two workers, a SIGKILL of the holder and a finish by the other, over real processes and
a real Postgres.

*Unverified*: the split profile running in containers: `up --wait` with the workers (no healthcheck), the recreation of
`orchestrator` as a control plane while `edge` runs, `docker compose exec edge wget` (busybox `wget` in the caddy image is
assumed, as its healthcheck uses it), `docker compose kill` and `up` of a single worker, and the mock agent's
`SubscribeToTask` and `GetTask` answers during the handover (from `dev/README.md`, checked only by curl). The first run is
the `Coder E2E` workflow.

*Unverified*:

- **A run of the stack.** The machine that wrote this had no Docker daemon: the coder container, its health check and
  environment (variable names were read from `crates/adam-coder/src/config.rs` at `22d7c6e`, unchanged at `0e08fe0`), the git-server build,
  `up --no-build --wait`, and `dev/coder-e2e.sh` against the real orchestrator, web, edge and coder. The first run is the
  `Coder E2E` workflow.
- That `mock-openai` matches every request of a real coder and OpenCode without the upstream `models.json`,
  `chat-completions.json` and `errors.json`, which are not vendored: the "nothing unmatched" check will say.
- That the orchestrator's delegation of a run that takes minutes stays within its own limits on the CI runner.

The complete local stack (slice 13: `compose.live.yaml`, `.env.example`, the `smee` and `local-agent` profiles, `dev/e2e-all.sh`, the coder re-pin):

*Verified 2026-09-30*:

- **The coder pin (2026-09-30, re-pinned).** `coder:sha-31df660@sha256:dafa562b...` is the manifest digest the ghcr API returns for that tag (anonymous token); the image is one manifest (no arm64 variant). adam-rs `31df660` continues a referenced task's conversation (A2A `referenceTaskIds`), adds `run_command`, bash, a default base branch and a missing-toolchain report; `e9bfea3` parks a stop without a pull request as a question and works only on a repository the person named. The vendored mocks equal `vymalo/another-adam-rs` at `31df660f5714` (mock-github gained the pull-request comment mapping and the base-aware pull list); `dev/coder/check-vendored.sh` passes. Earlier pins: `sha-e9bfea3`, `sha-ae540e9`.
- **The coder's new gate.** The real `orchestrator` binary (debug build) on Postgres 16, `dev/agents.yaml` (coder at `127.0.0.1:18090`), WireMock 3.13.2 for the mock agents, Caddy 2.11.4 running `dev/Caddyfile`, and a stand-in coder that streams the artifacts in the order adam-rs `ae540e9` documents (`checks` on the base commit, `checks` on the pushed commit, `branch`, `pull_request`, completed): `dev/coder-e2e.sh` printed `ok` for every check that does not need a Docker mock (two `checks` artifacts, the last passed on the pushed commit with a tree, the gate `ci+agent_checks`, the `agent_checks` card, one `vymalo.ci` card for `mock-ci/build` once a report for the pushed commit was posted with `ci-webhook.sh --shape github`, the thread `done`); the checks on `mock-github`, `mock-openai` and `git-server` failed, as they must without those mocks. This proves the gate and the script's `jq` paths, not the real coder's stream.
- **`dev/e2e-all.sh`** against that stack: `verify`, `verifier`, `mcp` and `ci` passed; a second `ci` was `SKIP` (exit 77) and the summary said how to reset; `VERBOSE=1` streams; an unknown scenario and a missing stack exit 2 with a message; a failing scenario prints its tail and exits 1. `shellcheck dev/*.sh dev/coder/*.sh dev/mock-ci/*.sh` is clean.
- **`compose.live.yaml`**: `docker compose -f compose.yaml -f compose.live.yaml --env-file .env.example config -q` passes (Compose v5.1.1, no daemon); the merged model has the coder's environment replaced (no `ALLOW_LOCAL_REPOS`), its `depends_on` reduced to `coder-postgres`, the four mocks in the profile `offline-mocks`, the live agents file and the `.env` secrets on the orchestrator; a missing `.env` value stops `config` with the message of the `:?` form. `docker compose --profile '*' config -q` passes for `compose.yaml`.
- **smee.** `dev/Caddyfile.smee` validates with Caddy 2.11.4, and run against the real orchestrator it answered `POST /webhooks/github` (401 without a signature, 202 with one made by `ci-webhook.sh`) and 404 to `/api/*`, `/agui/*`, `/mcp`, `/webhooks/ci` and `GET /webhooks/github`. `smee-client` 5.0.0 (npm, installed as the Dockerfile does) started through `dev/smee/entrypoint.sh` against a stand-in relay forwarded a signed `ping` through that Caddy to the orchestrator and got 204; the entrypoint exits 1 with a message for an unset or malformed `SMEE_URL`. `hadolint` is clean on `dev/smee/Dockerfile`. `dev/smee/entrypoint.sh` behaves the same under busybox 1.35 `sh` (the shell of the Alpine image): it refuses an unset or malformed `SMEE_URL` and reaches `exec smee` for a URL.
- **`dev/agents.live.yaml`** is accepted by the orchestrator binary with the placeholder secrets of `.env.example` (5 agents, startup); so are its two documented variations, `gate: {}` and `require: [agent-checks, ci]` with `ci.required: [build]`. The base image digest and tag come from the Docker Hub registry API; the npm package version from the npm registry.

*Unverified*: `docker compose up` itself (the machine that wrote this has the Compose CLI but no Docker daemon): the builds of `dev/smee/Dockerfile` and of the orchestrator with `agent-local`, the `smee` and `local-agent` profiles running, the coder container with the new image, `mock-ci` and the real `coder-e2e.sh`, and `dev/e2e-all.sh` `coder` scenarios against them (the first run is the `Coder E2E` workflow); that Compose v2.24.4 is the first release that parses `!override` (from Docker's documentation, not run on it); Claude Code against the MCP server; smee.io itself and GitHub deliveries through it (the body re-serialisation caveat above); the disk and memory estimates.

The coder reads its agent folder (MVP slice 1: the pin to adam-rs `7b2d8f9`, `dev/coder/agent/`, `dev/greeting-e2e.sh`, `dev/agent-folder-e2e.sh`):

*Verified 2026-10-01*:

- **The pin.** `coder:sha-7b2d8f9@sha256:aa84305a...` is the manifest digest the ghcr API returns for that tag (anonymous token), and the sha-256 of the manifest body it returned; one `linux/amd64` manifest (2.86 GB of compressed layers), uid 10001, entrypoint `tini -- adam-coder`, label `org.opencontainers.image.revision` = `7b2d8f95ffd9abe8af990bd79d7d690e8183c392`. adam-rs `7b2d8f9` (#57) reads `ADAM_AGENT_DIR` at startup, names the coder `Coder` and answers a greeting with a greeting; the vendored `coder-script.json` gained the greeting mappings.
- **The vendored set.** `dev/coder/check-vendored.sh` passes against `raw.githubusercontent.com` and the GitHub tree API at that commit, including the new `dev/coder/agent/` (compared with `bin/adam-coder/agent/`, completeness included).
- **The scripts' logic.** `dev/greeting-e2e.sh` and `dev/agent-folder-e2e.sh` were run in a private network namespace against a stand-in for the edge, the model mock's journal, the coder's card and `docker compose` (a fake that records the `CODER_AGENT_DIR` it is called with): every check printed `ok` (the greeting, the persona read back from the folder, the copy named Cody, the restart on the copy and back), and each check failed as it must when the stand-in answered with "give me a task" instead of a greeting; a SIGTERM during the restart scenario put the folder back (exit 130). This proves the scripts' `jq` and shell logic, the frames' shape (taken from [`docs/api/examples/agui/ask.agui.json`](../docs/api/examples/agui/ask.agui.json)) and the restore trap, not the real coder or WireMock.
- `shellcheck dev/*.sh dev/coder/*.sh dev/mock-ci/*.sh dev/smee/*.sh` is clean, and `docker compose --profile '*' config -q` and the live override against `.env.example` pass; the merged live model keeps the folder mount and sets `ADAM_AGENT_DIR`.

*Unverified*: the coder container on that mount (the image was not pulled where this was written), the real `dev/greeting-e2e.sh` and `dev/agent-folder-e2e.sh` against the stack (the `Coder E2E` workflow runs them; `folder` needs the Docker daemon of the machine that runs the stack), that the orchestrator's connection to the old coder container does not trip the first greeting after a restart (the script retries up to three times), the text of the answer when the first message is not a bare greeting, and how a live model follows the instructions.

The mock web search (`mock-mcp-search`, `dev/check-agent-mocks.sh`):

*Verified 2026-10-01*: the image built from `dev/mock-mcp-search/Dockerfile` (241 MB, user 1000, no dependencies) as the
`mock-mcp-search` service of `compose.yaml`, healthy through its own healthcheck, with `dev/check-agent-mocks.sh` (26
checks, all `ok`; it exits 1 with a wrong token) and `node --test dev/mock-mcp-search/server.test.mjs` (22 tests). Two
clients independent of it, against the process and against the container: **rmcp 3.5.0**, which adam-rs's `adam-mcp`
and the orchestrator use, through `adam-mcp`'s own `McpServers::connect` with the `mcp.json` shown above (`allow_insecure`),
then `serve` (legacy `initialize`: it asks for `2026-07-28`, is answered `2025-11-25`, sends no session id, and never
opens a GET stream) and the `Auto` lifecycle (it probes `server/discover` with a `2026-07-28` header, is refused with 400
and falls back to `initialize`): tools listed with their title and icon, `tools/call` answers, `isError` kept, a wrong
token fails the connection at `initialize`; and the **official TypeScript SDK 1.31.0** client (`StreamableHTTPClientTransport`:
listing, calling, an `isError` result). The spec points are from the 2025-11-25 transports and tools pages
(modelcontextprotocol.io, read 2026-10-01): the Accept rule, 202 for a notification, 405 when no SSE stream is offered,
403 for a foreign `Origin`, 400 for an unsupported `MCP-Protocol-Version`, `icons` as `src`, `mimeType`, `sizes`, `theme`, and
input validation errors as tool execution errors. *Unverified*: other clients (Claude Code, opencode), and the 2026-07-28
revision of the protocol, which the mock does not speak.

The chat and the researcher (MVP slice 2: the pin to adam-rs `f882b91`, `dev/agents/`, `mock-model`, `dev/agents-e2e.sh`):

*Verified 2026-10-01*:

- **The pin.** `coder:sha-f882b91@sha256:7c549618...` is the manifest digest the ghcr API returns for that tag (anonymous token), and the sha-256 of
  the manifest body it returned; one `linux/amd64` manifest (2.88 GB of compressed layers), uid 10001, entrypoint `tini -- adam-coder`, label
  `org.opencontainers.image.revision` = `f882b910b620ea583130a0517b4e52c5f7939179`. adam-rs's `coder` workflow smoke-tested both binaries in this image
  (`adam-coder`, then `adam-agent` with the example folder) and ran its own compose scenario for `adam-agent` on it before pushing. The only file of the
  vendored paths that changed upstream since `7b2d8f9` is a new mapping, `agent-script.json` (the `mock-assistant` model), which is not vendored;
  `dev/coder/check-vendored.sh` passes at `f882b91`, and now also fails on a second pin of the image in `compose.yaml`.
- **The model scripts in WireMock itself.** `wiremock-standalone-3.13.2.jar` (the version compose pins, run with `--global-response-templating
  --disable-banner` on `dev/wiremock/model`) against `dev/check-agent-mocks.sh`: 37 checks, all `ok` (26 of the mock web search, 11 of the two
  models: the greeting from the persona lines, a fourth agent's own name, a tool result in, the first turn's tool call and its query, the second
  turn's link, no link, a follow-up question, the 404 and the journal). Facts found there: `jsonPath` of `$.messages[-1].content` is the scalar
  string, while `$.messages[-1:].content` is an array whose `/` are written `\/` (a link cannot be matched in it); `regexExtract` without a variable name
  returns the whole match; `matchesJsonPath` of `$.messages[-1:][?(@.role == 'tool')]` tells the turn by the last message, so a follow-up question
  after an earlier tool result still searches.
- **The scenario, on real processes.** `dev/agents-e2e.sh` printed `ok` for every check (exit 0), and `dev/e2e-all.sh agents greeting` passed, against the
  real orchestrator (a debug build of a working branch of this repository, ahead of `main`, `ORCH_SURFACES=agui,webhook-generic,webhook-github`, `AUTH_DEV_USER`, an agents file made
  from `dev/agents.yaml` with the hosts changed to `127.0.0.1`), two `adam-agent` processes on `dev/agents/chat/agent` and a copy of
  `dev/agents/researcher/agent` whose `mcp.json` URL is `127.0.0.1` (debug builds of an adam-rs working checkout later than `f882b91`), `adam-coder` on
  `dev/coder/agent` and the vendored `mock-openai`, `mock-model` and `mock-mcp-search` (`server.mjs`) on WireMock 3.13.2, and Postgres 16 with one database
  per process group, all in a private network namespace. Both folders loaded with no warning (the `agent files` line), the researcher connected the
  search server at startup, and the tools the model was offered were `ask_user` (chat) and `ask_user` and `search__web_search` (researcher). With the chat folder
  edited to another name the script failed on the three checks that read the persona, as it must.
- `shellcheck dev/*.sh dev/coder/*.sh dev/mock-ci/*.sh dev/smee/*.sh`, `actionlint` on the two workflows, `docker compose --profile '*' config -q`, the live
  override against `.env.example` (the merged model keeps `chat` and `researcher` on the real model, drops `mock-model`) and the docs check are clean.

*Unverified*: the two services in containers (the image's `adam-agent` on the mounts, `depends_on` and the healthchecks, `MCP_ALLOW_INSECURE` over the compose
network, the folders readable by uid 10001), `dev/agents-e2e.sh` through the `edge` in the `Coder E2E` workflow (the first run is CI), the web's agent
picker with three agents, and how a live model follows the two folders' instructions (the mocks prove that a folder reaches the model and that a tool call
reaches the server, not that a model behaves).

Choices (MVP slice 3, `dev/choices-e2e.sh`; the pin to adam-rs `c13ddf1`, whose image holds `d411249`'s Choices):

*Verified 2026-10-01*:

- **The pin.** `coder:sha-c13ddf1@sha256:a77a2890...` is the manifest digest the ghcr API returns for that tag (anonymous token, HTTP 200), and the sha-256 of the manifest
  body it returned; one `linux/amd64` manifest (2.88 GB of compressed layers), uid 10001, entrypoint `tini -- adam-coder`, label `org.opencontainers.image.revision` =
  `c13ddf1a32a1424affa20043f6bd860d93c536cc`. Upstream's `coder` workflow smoke-tested both binaries in it and ran its own compose scenarios before it pushed (`dev/coder-choices-e2e.sh`: the coder and the mock
  model, no orchestrator; `dev/agent-cards-e2e.sh`: the researcher folder). Of the vendored paths, upstream changed two since `f882b91`, both by `d411249` and none by
  `c13ddf1`: the new mapping `coder-choices.json` and `bin/adam-coder/agent/instructions.md` (the paragraph on `choices`); `dev/coder/check-vendored.sh` passes at `c13ddf1`.
- `dev/choices-e2e.sh` (`shellcheck` clean, `sh` syntax) against a Python stand-in for the edge, the coder's card and the model mock's journal (the AG-UI frames shaped like
  `docs/api/examples/agui/a2ui.agui.json`, the export like the contract's), in a private network namespace: every check printed `ok` (exit 0); the stand-in made to quote only
  Postgres, and then to withhold `get_ui_catalog` from the model's tools, made exactly the checks that read those fail (exit 1). That tests the script's own reading (the
  `jq`, the sequence of the three runs, the digest it computes with `jq` and `sha256sum`, which equals the lock's for the shipped catalog), not the stack.
- `shellcheck dev/*.sh dev/coder/*.sh dev/mock-ci/*.sh dev/smee/*.sh`, `docker compose --profile '*' config -q`, the live override against `.env.example`, and the docs check are clean.

*Unverified*: the scenario in containers (the Docker stack was not started here: not enough disk for the orchestrator and web builds; it runs first in the Coder E2E workflow of the pull
request that introduced it), so that the thread-tools grant reaches the coder over plain `http` with `MCP_ALLOW_INSECURE`, that the real orchestrator's frames hold the surface in the shape
the script reads (`createSurface` and `updateComponents` with one `Choices`), and what a *live* model does with `choices`.

Cards and Mermaid (MVP slice 4, `dev/cards-e2e.sh`; the image is the one pinned for Choices, adam-rs `c13ddf1`):

*Verified 2026-10-01*:

- **The script, in WireMock itself.** `wiremock-standalone-3.13.2.jar` (the version compose pins, run with `--global-response-templating --disable-banner` on `dev/wiremock/model`) and the real
  `mock-mcp-search` (`server.mjs`, Node) against `dev/check-agent-mocks.sh`: every check `ok`, among them the new keyword `async` (three links) and the six of `[mock:cards]` (search for `async
  programming`, `ui_catalog`, `show` with a Text, a Cards whose links are the search's and a `graph TD`, the words, and the words again when `show` was refused); the earlier checks of the persona and
  the researcher are unchanged by the `doesNotContain "[mock:cards]"` added to their stubs. `node --test dev/mock-mcp-search/server.test.mjs`: 22 pass.
- **The `show` blocks of the script against the web's catalog.** Each block, with an `id` added as adam-rs's `show` does, validates against its schema in `catalog.json` (JSON Schema 2020-12,
  `jsonschema` 4.26).
- **`dev/cards-e2e.sh`** (`shellcheck` clean) against a Python stand-in for the edge, the researcher's card, the model mock's journal and the search's (AG-UI frames shaped like
  `docs/api/examples/agui/a2ui.agui.json`) in a private network namespace: every check printed `ok` (46 lines, exit 0), and `dev/e2e-all.sh cards` passed; with the stand-in made to accept `show` for a
  catalog without `Cards`, or to send two agent messages, exactly the checks that read those failed (exit 1). That tests the script's own reading (the `jq`, the three runs, the digest of the older
  catalog), not the stack.

*Unverified*: the scenario in containers (the Docker stack was not started: the disk was too small for the builds), so that the researcher in the image really answers with a surface of those three
components, one agent message, from the `ui` artifact the orchestrator maps to an `a2ui-surface`, and that the `ui_catalog` and `show` results read as the script expects (`Cards` and `Mermaid` named in the
first, "Shown to the person." in the second, from adam-rs's README); what an older screen's browser does with a `Cards` (the web's tests); and what a live model chooses to show.
