# AG-UI binding

How the orchestrator speaks [AG-UI 1.0](https://docs.ag-ui.com/spec/1.0/index.md) to people
([ADR 0012](../decisions/0012-ag-ui-user-facing-protocol.md)), and how generative UI travels
([ADR 0013](../decisions/0013-a2ui-generative-ui.md)). The event log is the only source of truth:
every frame below is a function of the log. The resource API (agents, threads, cancel, health)
stays in [`chat-api.yaml`](chat-api.yaml).

> Status: **design**. The surface is built in the slices listed in ADR 0012; until then this is the
> contract those slices implement. Spec facts were *verified 2026-09-29* against the pages linked.

## Endpoints

| Operation | Route | Standard? |
|---|---|---|
| Run (create a thread, send a message, answer an interrupt, send an A2UI action) | `POST /agui/agents/{agentId}` | Yes: HTTP + SSE binding |
| Attach, replay, follow across runs, resume | `GET /agui/threads/{threadId}/connect` | No: our extension ([Connect binding](#connect-binding)) |
| Capabilities | `GET /agui/agents/{agentId}/capabilities` | Shape standard (`AgentCapabilities`), retrieval ours |
| Agent list, thread list and details, cancel, health | `/api/agents`, `/api/threads`, `/api/threads/{id}`, `/api/threads/{id}/cancel`, `/healthz`, `/readyz` | REST resource API |
| Legacy interaction (`createThread`, `postMessage`, `listEvents`, `streamEvents`) | `/api/threads…` | Deprecated; mounted only with `ORCH_SURFACES` including `chat-api` |

All `/agui/*` routes sit behind the edge identity (`X-Auth-Request-Email`, fail closed). Every
pre-stream rejection is an RFC 9457 `application/problem+json` response; nothing is streamed
before the checks pass.

## Ids

Every id is derived from the log, so every replica and every replay agrees.

| Id | Rule |
|---|---|
| `threadId` | The thread UUID. Minted by the consumer on its first run (a UUID; 400 otherwise); the legacy `createThread` mints it server-side. |
| `runId` | `user_message.data.runId` when the run came from AG-UI; otherwise `run-<seq>` of the event that opened the run. |
| user `messageId` | `user_message.data.messageId` (the AG-UI message id), else `evt-<seq>`. |
| agent `messageId` | `agent_message.data.messageId` (the A2A message id). |
| activity `messageId` | `evt-<seq>`; for A2UI, `a2ui-<seq>` of the event that created the surface. |
| `subagentRunId` | `sub-<seq>` of the first agent event of the invocation; reused when a suspended invocation continues on the same A2A task. |
| interrupt `id` | `int-<seq>` of the `agent_status` that asked for input. |
| SSE `id:` | `<seq>` on the last frame produced for that log event, only when no text message is open. |

## Outbound: log event → AG-UI

"Open a run" means `RUN_STARTED{threadId, runId, protocolVersion:"1.0"}` then
`STATE_SNAPSHOT{snapshot:{thread}}`. Audiences: the **requester** (the POST that sent the input)
skips the text triad of a user message whose id came in its own `RunAgentInput`, because
re-streaming a message the consumer holds would append to it; a **viewer** (a connect stream)
gets everything.

| Log event (`kind`, data) | Context | AG-UI frames |
|---|---|---|
| `user_message{text}` | No run open | Open a run. Viewer: `TEXT_MESSAGE_START{messageId, role:"user", metadata:{"vymalo.actor"}}` → `TEXT_MESSAGE_CONTENT{delta:text}` → `TEXT_MESSAGE_END` |
| `user_message` | Run open (a follow-up mid-run) | The user triad inside the current run |
| `agent_message{messageId, text, final:true}` | — | `SUBAGENT_STARTED{subagentRunId, name:agentId}` if no invocation is open; then `TEXT_MESSAGE_START{messageId, role:"assistant", name:agentId, subagentRunId}` → `CONTENT` → `END` |
| `agent_message{final:false}` (cumulative partial) | — | First partial: `START` + `CONTENT(text)`. A later partial or final that extends the text: `CONTENT(suffix)`, plus `END` on final. A partial that does not extend it: open question 14. |
| `agent_status{working, detail?}` | — | `ACTIVITY_SNAPSHOT{messageId:"evt-n", activityType:"vymalo.status", content:{status, detail?}, subagentRunId}`; a `STATE_SNAPSHOT` if the thread moved to `working` |
| `agent_status{input_required \| auth_required, detail}` | Followed by `thread_state{blocked}` | The status activity, then `SUBAGENT_FINISHED{outcome:{type:"suspended", interruptIds:["int-n"]}}` |
| `thread_state{blocked}` | After input or auth required | `STATE_SNAPSHOT` → `RUN_FINISHED{outcome:{type:"interrupt", interrupts:[{id:"int-n", reason:"input_required" \| "auth_required", message:detail, subagentRunId, responseSchema}]}}` |
| `agent_status{completed}` + `thread_state{done}` | — | Status activity → `SUBAGENT_FINISHED{}` → `STATE_SNAPSHOT` → `RUN_FINISHED{outcome:{type:"success"}}` |
| `agent_status{failed, detail}` + `thread_state{failed}` | — | Status activity → `SUBAGENT_ERROR{message:detail, code:"agent_failed"}` → `STATE_SNAPSHOT` → `RUN_ERROR{message, code:"agent_failed"}` |
| `agent_status{canceled}` + `thread_state{cancelled}` | — | Status activity → `SUBAGENT_FINISHED{result:{status:"canceled"}}` (1.0 has no cancelled subagent outcome; open question 16) → `STATE_SNAPSHOT` → `RUN_FINISHED{outcome:{type:"cancelled"}}` |
| `thread_state{cancelled}` alone | Cancelled before the agent started | `RUN_FINISHED{outcome:{type:"cancelled"}}` |
| `error{retryable:true}` + `thread_state{blocked}` | Retryable delivery failure | `ACTIVITY_SNAPSHOT{activityType:"vymalo.error"}` → `SUBAGENT_ERROR{code:"delivery_failed"}` if open → `STATE_SNAPSHOT` → `RUN_ERROR{code:"delivery_failed"}`. The thread stays open; the next input is a new run, not a resume. |
| `error{retryable:false}` + `thread_state{failed}` | Permanent delivery failure | Error activity → `SUBAGENT_ERROR` if open → `RUN_ERROR{code:"delivery_failed"}` |
| `error{…}` | Mid-run, no state change | Error activity only; the run continues |
| `artifact{name, mimeType?, uri?, text?}` | — | `ACTIVITY_SNAPSHOT{messageId:"evt-n", activityType:"vymalo.artifact", content:{name, mimeType?, uri?, text?}, subagentRunId}` |
| `ui_surface{operations}` (ADR 0013) | — | `ACTIVITY_SNAPSHOT{messageId:"a2ui-<seq of createSurface>", activityType:"a2ui-surface", replace:true, content:{a2ui_operations:[every operation of that surface so far]}, subagentRunId}` |
| `ui_action{surfaceId, name, context}` (ADR 0013) | — | Open a run if none is open; `ACTIVITY_SNAPSHOT{messageId:"evt-n", activityType:"vymalo.action", content:{surfaceId, name, context}, metadata:{"vymalo.actor"}}` |
| Any other event | No run open, not user input (a webhook, a timer, a late delivery failure) | A producer-initiated run: `RUN_STARTED{runId:"run-<seq>"}` with no input echo, the event's frames, then closed by the same rules (open question 17) |

- **Why activities, not `CUSTOM`.** Activity messages are part of the message sequence and of
  `MESSAGES_SNAPSHOT`, so they survive history restore; the spec forbids standard semantics in
  `CUSTOM`. `STEP_*` events are not used.
- **Thread state** travels as `STATE_SNAPSHOT` under `thread`. It is producer-owned; an echoed
  `RunAgentInput.state` is ignored.
- **Errors in stream.** `RUN_ERROR.code` is one of `agent_failed`, `agent_rejected`,
  `delivery_failed`, `unavailable`, `internal`; `metadata["vymalo.problem"]` carries
  `{type, title, detail?}`.

## Inbound: AG-UI → core input

| `RunAgentInput` / request | Core effect |
|---|---|
| Unknown `threadId` (a UUID), one new user message | Create the thread, owned by the edge identity, targeting the URL's `agentId` and the release in `forwardedProps["https://agents.vymalo.com/a2a/extensions/release-channels/v1"].release` (validated against the live card, fail closed, ADR 0008); then `Input::UserMessage{text}` with `messageId` and `runId` recorded |
| `threadId` owned by someone else, or colliding with another owner's thread | 404 before the stream |
| Known `threadId`, URL `agentId` is not the thread's target | 409 before the stream |
| Known `threadId`, exactly one user message id not in the log, text content | `Input::UserMessage` |
| Several new messages, or a new non-user message | 422 before the stream (the orchestrator owns the history) |
| Messages whose ids are already in the log | Ignored: reconciliation by id, so a client that re-sends the whole transcript works |
| `resume:[{interruptId:"int-n", status:"resolved", payload:{text}}]` on a blocked thread | `Input::UserMessage{text}` (the A2A task continues) |
| `resume` `cancelled` plus a new user message | `Input::UserMessage` with the new text |
| `resume` `cancelled`, nothing new | `Input::Cancel` |
| A new user message on a blocked thread without `resume` | Accepted as the answer (open question 13) |
| `forwardedProps.a2uiAction.userAction` (ADR 0013) | `Input::UiAction{surfaceId, name, sourceComponentId?, context}`; on a blocked thread it answers the interrupt |
| `resume` on a thread that is not blocked, or naming an unknown id | Entries ignored with a warning |
| Nothing new, no resume, `runId` already recorded | Attach: stream that run from its start (an idempotent retry) |
| Nothing new, no resume, unknown `runId` | 422 (nothing to run) |
| A run already open on the thread | 409 before the stream |
| Thread terminal (`done`, `failed`, `cancelled`) | 409, "start a new thread" |
| `protocolVersion` of another major | 400 before the stream; a newer 1.x is served with a warning |
| `tools`, `context` | Ignored with a warning (open question 18) |
| `state` | Ignored (producer-owned) |
| `parentRunId` | Recorded in metadata, no effect |
| Non-text content parts | Skipped with a warning; the run does not fail |
| Idempotency | Inbox key `(agui, <threadId>:<messageId>)`; a retried POST attaches instead of duplicating |
| Cancel | `POST /api/threads/{id}/cancel`; the outcome arrives as `RUN_FINISHED{outcome:{type:"cancelled"}}` |

## Run binding

`POST /agui/agents/{agentId}` with `Content-Type: application/json`, `Accept: text/event-stream`
and a `RunAgentInput`. The response is `200 text/event-stream`, one JSON event per `data:` line,
from `RUN_STARTED` of the requested run to its terminal event, then EOF, as the
[HTTP + SSE binding](https://docs.ag-ui.com/spec/1.0/basic/transports/http-sse.md) says. It is the
requester-audience projection starting at the run's first log event. Frames also carry `id: <seq>`,
which standard consumers ignore. Closing the response never cancels the run.

## Connect binding

Our extension, a custom transport in the sense of the
[transports page](https://docs.ag-ui.com/spec/1.0/basic/transports/index.md): it keeps the event
model, the patterns and the processing rules, and frames JSON exactly as the SSE binding does. The
sequence and state diagrams are in [ADR 0012](../decisions/0012-ag-ui-user-facing-protocol.md#the-connect-stream-our-extension).

**Request.** `GET /agui/threads/{threadId}/connect`, `Accept: text/event-stream`.

| Input | Meaning |
|---|---|
| `Last-Event-ID` header | Resume after this seq. Absent: replay the whole thread. |
| `?mode=run` | Close after the active run's terminal event, or right after the replay when no run is open. Default: stay open. |

**Errors before the stream.** 401 without an identity; 404 when the thread does not exist or
belongs to someone else; 400 for a cursor that is not a non-negative integer. A cursor beyond the
thread's last seq waits for new events.

**Response.** `200 text/event-stream`:

1. **Replay.** The viewer-audience projection of every event after the cursor, as a sequence of
   runs. With a cursor inside an open run, it starts with a **preamble**: that run's
   `RUN_STARTED` (same `runId`), `SUBAGENT_STARTED` for each open invocation and a
   `STATE_SNAPSHOT`. Every stream thus begins with `RUN_STARTED`, as the spec requires.
2. **Tail.** New log events, projected the same way, across runs, until the client closes (or the
   active run closes, with `mode=run`). Between runs the stream is idle, not closed.
3. **Keepalive.** A comment line (`: keepalive`) at least every 15 s.

**Resume points.** `id: <seq>` is written only on the last frame of a log event and only when no
text message is open, so resuming never splits a message. Reconnecting with that id yields exactly
the remaining frames (a property test).

**Announcement.** `GET /agui/agents/{agentId}/capabilities` returns `transport:{streaming:true,
resumable:true}`. A standard client that ignores it still gets conforming runs from the run
endpoint.

## Capabilities document

`GET /agui/agents/{agentId}/capabilities` → `AgentCapabilities`:

- `identity` from the live A2A card (name, description, version);
- `transport{streaming:true, resumable:true}`;
- `humanInTheLoop{interrupts:true}`;
- `multiAgent{subagents:true}`;
- `custom["https://agents.vymalo.com/a2a/extensions/release-channels/v1"] = {defaultChannel,
  channels, revisions}` only when the card advertises the extension (ADR 0008), read live;
- `custom["https://a2ui.org/a2a-extension/a2ui/v0.9.1"] = {supportedCatalogIds}` only when the card
  advertises A2UI (ADR 0013).

## `vymalo.*` schemas

Activity types, metadata keys and custom capability keys carry the `vymalo.` prefix or a URI we
own, as the spec asks of vendor keys. A client that knows none of them still sees conforming runs.

### Activity contents

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$defs": {
    "vymalo.status": {
      "type": "object",
      "required": ["status"],
      "additionalProperties": false,
      "properties": {
        "status": { "enum": ["submitted", "working", "input_required", "auth_required", "completed", "failed", "canceled"] },
        "detail": { "type": "string" }
      }
    },
    "vymalo.artifact": {
      "type": "object",
      "required": ["name"],
      "additionalProperties": false,
      "properties": {
        "name": { "type": "string" },
        "mimeType": { "type": "string" },
        "uri": { "type": "string", "description": "Rendered as a link only when absolute http(s)" },
        "text": { "type": "string" }
      }
    },
    "vymalo.error": {
      "type": "object",
      "required": ["message", "retryable"],
      "additionalProperties": false,
      "properties": {
        "message": { "type": "string" },
        "retryable": { "type": "boolean" }
      }
    },
    "vymalo.action": {
      "type": "object",
      "required": ["surfaceId", "name"],
      "additionalProperties": false,
      "properties": {
        "surfaceId": { "type": "string" },
        "name": { "type": "string" },
        "sourceComponentId": { "type": "string" },
        "context": { "type": "object" }
      }
    }
  }
}
```

`a2ui-surface` is the ecosystem's type, not ours: `content` is `{a2ui_operations: [A2UI message]}`
as sent by the agent (ADR 0013).

### Metadata and state

| Where | Key | Shape |
|---|---|---|
| Any attributed event (`TEXT_MESSAGE_START`, `ACTIVITY_SNAPSHOT`, `SUBAGENT_STARTED`) | `metadata["vymalo.actor"]` | `{type: "user" \| "agent" \| "system", name, revision?}`; `revision` is the ADR 0008 echo |
| `RUN_ERROR` | `metadata["vymalo.problem"]` | `{type, title, detail?}` |
| `STATE_SNAPSHOT.snapshot` | `thread` | `{state: "queued" \| "working" \| "blocked" \| "done" \| "failed" \| "cancelled", title, target: {agentId, release?}}` |
| Interrupt `responseSchema` | — | `{type:"object", required:["text"], properties:{text:{type:"string"}}}` |

## Worked example

`ask.events.json` for a viewer (the golden `docs/api/examples/agui/ask.agui.json` is generated from
it in a later slice):

```text
RUN_STARTED          {threadId, runId:"run-1", protocolVersion:"1.0"}
STATE_SNAPSHOT       {snapshot:{thread:{state:"queued", target:{agentId:"plain"}}}}
TEXT_MESSAGE_START   {messageId:"evt-1", role:"user", metadata:{"vymalo.actor":{type:"user", name:"alice@example.com"}}}
TEXT_MESSAGE_CONTENT {messageId:"evt-1", delta:"ask about branches"}
TEXT_MESSAGE_END     {messageId:"evt-1"}                                                      id: 1
SUBAGENT_STARTED     {subagentRunId:"sub-2", name:"plain"}
ACTIVITY_SNAPSHOT    {messageId:"evt-2", activityType:"vymalo.status", content:{status:"working"}, subagentRunId:"sub-2"}   id: 2
ACTIVITY_SNAPSHOT    {messageId:"evt-3", activityType:"vymalo.status", content:{status:"input_required", detail:"Which branch?"}}
SUBAGENT_FINISHED    {subagentRunId:"sub-2", outcome:{type:"suspended", interruptIds:["int-3"]}}  id: 3
STATE_SNAPSHOT       {snapshot:{thread:{state:"blocked"}}}
RUN_FINISHED         {runId:"run-1", outcome:{type:"interrupt", interrupts:[{id:"int-3", reason:"input_required", message:"Which branch?"}]}}   id: 4
RUN_STARTED          {threadId, runId:"run-5", protocolVersion:"1.0"}
STATE_SNAPSHOT       {snapshot:{thread:{state:"queued"}}}
TEXT_MESSAGE_START/CONTENT/END {messageId:"evt-5", role:"user", delta:"main"}              id: 5
SUBAGENT_STARTED     {subagentRunId:"sub-2", name:"plain"}          (the same A2A task continues)
ACTIVITY_SNAPSHOT    {messageId:"evt-6", activityType:"vymalo.status", content:{status:"working"}}         id: 6
ACTIVITY_SNAPSHOT    {messageId:"evt-7", activityType:"vymalo.artifact", content:{name:"result", text:"answered: main", uri:"https://github.com/acme/demo/pull/1"}}   id: 7
ACTIVITY_SNAPSHOT    {messageId:"evt-8", activityType:"vymalo.status", content:{status:"completed"}}
SUBAGENT_FINISHED    {subagentRunId:"sub-2"}                                                   id: 8
STATE_SNAPSHOT       {snapshot:{thread:{state:"done"}}}
RUN_FINISHED         {runId:"run-5", outcome:{type:"success"}}                                 id: 9
```
