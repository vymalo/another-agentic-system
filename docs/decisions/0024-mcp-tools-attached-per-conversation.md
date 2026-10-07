# ADR 0024 — MCP tools attached per conversation, from the UI

- **Status:** accepted (2026-10-01), on the owner's delegation: open question 35 is decided for the relay, which
  amends decisions 3 and 6; see the [status note](#status-note-2026-10-01-accepted-on-the-owners-delegation). The owner may revisit it.

## Context

The owner (2026-10-01): "I can imagine how A2A does coding, another does web research, ... And if I
wanna do casual chat, I might use each one of them, pass in MCP tools for e.g. web search, e.g. from
the UI directly, and let the agent somehow use it." Tools with an icon should show it as the step's
trailing icon ([vision](../vision.md#3-tools-mcp-per-conversation-from-the-ui)).

Today an agent's MCP servers are part of the agent (adam-rs `mcp.json`, compiled in), and the
orchestrator is an MCP server for other systems ([ADR 0019](0019-mcp-server-over-streamable-http.md)),
but nothing passes tools from the person to an agent.

- MCP tool definitions carry optional `icons` (`src`, `mimeType`, `sizes`), "for display in user
  interfaces" (*verified 2026-10-01*, MCP specification 2025-11-25,
  <https://modelcontextprotocol.io/specification/2025-11-25/server/tools>). The same page says clients
  must treat tool annotations as untrusted unless they come from trusted servers.
- The platform plans "caller-supplied tools" that must pass policy before an agent uses them
  (another-agentic-platform, architecture §35).

## Decision

1. **The person attaches MCP servers to a conversation** in the UI and can detach them. By default
   they choose from servers the deployment lists (name, URL, description, icon); entering any URL is
   off unless the deployment allows it.
2. **The thread records it**: new event kinds `tools_attached` and `tools_detached` (ADR 0004, ADR
   0001). Every later task of the thread sees the current set.
3. **Agents get the set through an optional A2A extension**, detected from the card (the
   [ADR 0008](0008-platform-integration-via-a2a-extension.md) pattern): message metadata under the
   extension's URI lists each server (name, URL, transport, icon, an optional tool filter). An agent
   whose card does not list the extension gets nothing, and **the UI says that this agent cannot use
   attached tools**; nothing is dropped silently (fail closed).
4. **An agent may refuse** a server (for instance by platform policy, §35); the refusal reaches the
   chat as an error on the step, not a silent skip.
5. **Icons.** A tool step shows the tool's MCP icon, or the configured server icon, as its trailing
   icon. Only http(s) icons are used, and how they are fetched follows the image decision (open
   question 38); until then the web shows a generic icon.
6. **Credentials are not decided** (open question 35). The two candidates:
   - **direct:** the agent connects to the server itself, with credentials passed by reference;
   - **relay:** the orchestrator exposes a per-thread MCP endpoint that forwards to the attached
     servers and holds their credentials; the agent gets that endpoint and a short-lived token for
     the thread. The relay also lets the orchestrator see each tool call for the steps (ADR 0025).

## Consequences

- One agent serves several uses: a chat agent with web search attached, the same agent later with a
  database tool.
- The relay, if chosen, makes the orchestrator an MCP client and a proxy: more code on the critical
  path, but credentials never travel in A2A messages.
- Required elsewhere: the deployment's server list, a picker in the composer, the events and their
  migration, the extension contract, the A2A adapter, and agent support (adam-rs reads the set and
  adds the servers to its tool universe for the task).

## Alternatives rejected

- **Tools configured only in the agent.** That is today; it cannot follow the person from chat to
  chat.
- **Browser-side tools (AG-UI frontend tools).** They need a model in the browser's loop
  ([ADR 0013](0013-a2ui-generative-ui.md) context); our agents are remote.

## Status note, 2026-10-01: accepted on the owner's delegation

The owner delegated the points this ADR left open so that the MVP can be completed (2026-10-01). They were
decided on that delegation as follows; the owner may revisit them.

- **Credentials: the relay (decision 6, open question 35).** The orchestrator holds the credentials of the servers the
  deployment lists (configuration and environment) and relays their tools on its per-thread MCP endpoint (extension
  `https://agents.vymalo.com/a2a/extensions/thread-tools/v1`; see the status note of
  [ADR 0023](0023-ui-component-catalog-as-an-a2a-extension.md) and the contract
  [`api/thread-tools-v1.md`](../api/thread-tools-v1.md)). Credentials never travel in A2A messages and never reach the
  event log or the outbox.
- **Decision 3, amended.** The agent receives no server URL: it receives the endpoint (URL, token, expiry) under the
  thread-tools URI with the names of the attached servers, and finds their tools on the endpoint, named
  `<server>__<tool>`. An agent whose card does not list the extension gets nothing, and the UI says that it cannot use
  attached tools, as decided.
- **Steps and icons (decision 5).** The orchestrator sees every relayed call and reports it as a step
  ([ADR 0025](0025-nested-steps-events-carry-their-source-path.md)) with the server's icon from the configuration;
  icons at remote URLs wait for open question 38.
- **Refusals (decision 4).** A server the deployment does not offer to the thread's agent is refused when it is
  attached; a call that a server refuses is a failed step and an error result for the agent.
- Still open: authentication to the servers beyond static credentials (question 11; OIDC for MCP after the MVP).

## Status note, 2026-10-02: the contract written, on the owner's decisions of plan 11

The contract is written in [`api/thread-tools-v1.md`](../api/thread-tools-v1.md#attached-servers-and-the-relay-slice-8); it
is not built. It settles what this ADR and its note of 2026-10-01 left to the slice, on the owner's delegation:

- **Where the attachable servers are configured:** the `toolServers` section of the YAML configuration
  ([ADR 0034](0034-one-yaml-configuration-secrets-by-reference.md)), credentials by reference, in place of a file named by
  an environment variable.
- **Who may attach:** anyone with `thread.write` on the thread, for the servers whose `agents` list includes the thread's
  agent; no per-role filter yet.
- **Icons (decision 5, narrowed):** `data:` URIs from the configuration only; an icon at an http(s) URL is never fetched
  (open question 38), and the icons an upstream server offers are dropped.
- **Steps (decision 3 of the note):** the relayed tool's `_meta["thread-tools/v1"]` says `reportsStep: true`, so an agent
  knows the orchestrator reports the call and does not report its own; the step carries the call's input and output under
  [ADR 0030](0030-a-step-carries-its-input-and-output-bounded-and-redacted.md), which overrides the earlier plan that they
  are not logged.
- **Timeouts:** `_meta` `timeoutSecs` tells the agent how long a call may run, in place of a fixed 60 seconds.
- **The message:** `attached` names the servers (id, name, description), never a URL or credential.

## Status note, 2026-10-02: attaching is built; the relay is not

Built (slice 8, first half): the `toolServers` section of the configuration ([`config.md`](../api/config.md#toolservers)); the
events `tools_attached` and `tools_detached` and the set in the thread's job ledger (carried from job to job, migration
`0012_tools.sql`); `GET /api/tool-servers` and `PUT /api/threads/{threadId}/tools`
([`chat-api.yaml`](../api/chat-api.yaml)); `forwardedProps["vymalo.tools"]` on the run that creates a thread, in the same commit as
the first message; the `vymalo.tools` activity and `thread.tools` of the state snapshot ([`agui.md`](../api/agui.md#attaching-mcp-servers));
and `attached` in the `thread-tools/v1` message, for an agent whose card lists the extension ([`thread-tools-v1.md`](../api/thread-tools-v1.md#the-attached-member)).
Decided where the plan was silent, on the same delegation:

- **A thread keeps what it has.** Only a server that is *new* to the thread is checked against the deployment's list and the
  thread's agent; one the thread has already is not, so a deployment that stops listing a server does not make the set of its
  threads unsettable, and a person can detach it. It is no longer told to the agent.
- **A fork carries the set its copied log left**, minus the servers its agent may not use; those are detached in the fork's own
  first events, so the log and the thread agree.
- **`GET /api/tool-servers` says which agents a server is for** (`agents`, absent for every agent), beside the id, name,
  description and icon, so the picker offers only what the thread's agent can have; the allow-list of a server's tools, its
  timeout, its URL and its credentials are not in it.
- **A sixty-fifth server is a configuration error**, so the list the API shows is bounded.

Still to build: the relay (the tools on the thread's endpoint, the step of each call, the errors), the client port it needs, and the
picker in the composer.

## Status note, 2026-10-02: the client port is built; the relay is not

Built (slice 8, second half, first part): the port the relay calls the servers through, `ToolServerClient` in `orch-ports`
([ADR 0009](0009-swappable-implementations-at-build-time.md)), and its MCP implementation, `orch-tools-mcp` (`rmcp`'s client over
streamable HTTP). The port lists a server's tools and calls one, in MCP's shapes as plain JSON, so no MCP library type is in a
signature. Decided where the plan was silent, on the same delegation:

- **The timeout belongs to the endpoint** (`toolServers[].timeoutSecs`) and bounds the whole request, connecting included; a
  caller that wants another limit for one call clones the endpoint with another timeout. A handshake that has not answered by
  then is `Unreachable`, a call that has not is `TimedOut`. A call is cancelled by dropping its future.
- **The credentials are `ToolSecret`s** (`Debug` prints `<redacted>`), on the endpoint beside the URL; no error of the port holds
  one, and the client follows no redirect, so the headers go to the configured server and nowhere else.
- **One request is one session**: nothing is held between calls and no listing is cached, so any replica serves any request.
- **The 256 KiB bound is the port's**: every implementation cuts a result's content at `MAX_RESULT_BYTES` and says so
  (`truncated`), so the relay appends its note and does not re-measure. A tool's `isError` is an `Ok` result, passed through; a
  server's JSON-RPC error (an unknown tool among them) is `Remote`, its message cut at 1 KiB.

Still to build: the relay itself (the tools on the thread's endpoint, the step of each call, the errors) and the picker in the
composer.

## Status note, 2026-10-02: the relay is built

Built (slice 8, second half): the relay, `RelayTools` in `orch-surface-thread-tools`, a provider of the thread's endpoint. It lists
the tools of each server that is attached to the thread and offered for the caller's agent, named `<server>__<tool>` with the
deployment's allow-list applied and `_meta["thread-tools/v1"] = {reportsStep: true, timeoutSecs}`, and relays a call through the
`ToolServerClient` port with the server's bearer and headers, recording **one step per call** through the same input an agent's
step takes (`App::record_step`): the server's icon (`mcp-server:<id>`), the arguments as `input` and the result's text as
`output` under the bounds of [ADR 0030](0030-a-step-carries-its-input-and-output-bounded-and-redacted.md), `running` and then
`completed`, `failed` or `canceled`. Every row of the contract's error table is a test, on the in-memory client and on the real
MCP client against a real server, and no credential reaches a table, the log, the export or a frame. The contract is
[`api/thread-tools-v1.md`](../api/thread-tools-v1.md#attached-servers-and-the-relay-slice-8) ("As built" lists what it left
open); the picker in the composer is the web's slice. Decided where the plan was silent, on the same delegation:

- **The binary composes it behind the Cargo feature `tool-relay`, on by default**, with `surface-thread-tools` (the relay is
  one of that surface's providers). On by default because it does nothing without `toolServers` in the configuration and the
  surface mounted, and the image the stack runs must have it; a build without it still lets a person attach the listed servers
  and says at startup that they give an agent no tools.
- **A tool that fits no rule the relay can check is the server's to refuse.** The relay answers `-32602` for what it can see
  is not on the endpoint (an unlisted or detached server, one not offered for the agent, a name the allow-list or the name rule
  leaves out). A name of an attached server's tool that the server does not have is sent to it, and the server's JSON-RPC
  error is a failed step: the relay does not list before each call.
- **The step's id comes from the agent's `callId`** (`tool-<callId>`), so a retry is the same step; the call itself is not
  deduplicated (at least once, as the contract says). Without a usable `callId` the id is `tool-<uuid>`.
- **A call is bounded twice**: by the endpoint's timeout in the client, and by that timeout plus 5 s around the client, so a
  client that does not keep the port's promise cannot hold the endpoint.
- **A call that is dropped ends its step as `canceled`** from a guard that records it from a task of its own, because a
  connection that closes drops the call's future, which cannot await.

Still to build: the picker in the composer and the web's step icon (PR-8 of plan 11), and the adam-rs side that reads
`reportsStep`, `timeoutSecs` and sends `callId` and `parentStepId`.

## Status note, 2026-10-06: people may add their own servers (proposed)

[ADR 0046](0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md) (proposed) lifts "a person cannot enter a URL" for
servers a person adds themselves, kept per person with their secrets in a credential broker; they join the same attach and relay
path. The deployment's `toolServers` and this ADR's relay are unchanged.
