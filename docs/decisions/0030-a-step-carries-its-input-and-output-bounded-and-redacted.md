# ADR 0030 — A step carries its input and output, bounded and redacted

- **Status:** accepted (2026-10-02), on the owner's answer of the same day (question 5 of the feedback of
  2026-10-02: recorded by default, redacted and capped, with a switch to turn it off). Amends
  [ADR 0025](0025-nested-steps-events-carry-their-source-path.md) (a step is its label and its detail) and
  [ADR 0024](0024-mcp-tools-attached-per-conversation.md) (what the relay logs of a call).

## Context

"When I click on e.g. `search__web_search`, nothing. Not a small collapsible block nicely telling me: params ->
output" (owner, 2026-10-02). In the exported researcher thread each of the three `search__web_search` steps is a start
and an end, four milliseconds apart, whose `data` is `{id, kind, label, path, phase, state}`: nothing exists to show on
click. A tool step is the place a person looks when an agent did the wrong thing, and the question there is what it was
asked and what it said.

- A step has a short `detail` (at most 1000 characters, one human line) and nothing else
  ([`steps-v1.md`](../api/steps-v1.md)). The adapter ignores members it does not know, so a step can grow additively.
- The agent's own record of the call (adam-rs ADR 0007, decision 3) forbids putting a tool's output in the step's
  `detail`: that line is for people, and a result is not one.
- Tool arguments and results are untrusted and can hold credentials (a token in a command line, a key in a header, a
  password in a URL) and be large (a page of search results, a build log). The log is the chat: it is read by everyone
  who may read the thread, exported, and kept.
- The orchestrator will report the calls it relays for an agent (ADR 0024) as steps of its own. An earlier plan said
  those calls' arguments and results are not logged.

## Decision

1. **A step may carry `input` and `output`** (an additive revision of `steps/v1`, still v1; an orchestrator that
   ignores them reads the step as before, and an agent that sends none still works):
   - `input`: a JSON object, what the tool was called with, logged **once per step**, with its start (or with the first
     report that has one);
   - `output`: `{text, truncated?, bytes?, error?}`, what the tool returned, logged with the step's **end**; on a failed
     step `text` is the error the tool returned and `error` is `true`.
   `detail` stays the short human line. They are members of the `agent_step` event's data, so no new event kind and no
   migration; the AG-UI `vymalo.step` activity carries them as logged.
2. **They are bounded at the core's door** ([`StepReport::sanitize`](../../orchestrator/crates/core/src/step.rs),
   `StepLedger`), whatever the agent did:
   - `input`: at most **4 KiB** serialized. Strings over 512 characters are cut; an input still over the bound is
     replaced by `{"_cut": true, "bytes": n}`;
   - `output.text`: at most **8 KiB**, keeping its head and its tail (errors are at the end) with a line that says how
     many bytes are not kept, and `truncated` and `bytes` set;
   - **2 MiB per job** of both together, remembered in the job ledger so every replica decides the same. Past it the
     members are dropped and the event says `ioDropped: true`; the step is always kept. The worst case for the log is
     +2 MiB per job, where a call per step at the per-step bounds would be 24 MiB;
   - control characters other than line break and tab are removed.
3. **They are redacted twice.** The agent redacts what it knows exactly (its own secrets, adam-coder's `Redactor`). The
   core redacts what it recognises, in a pure function (`orch_core::redact`, no I/O, linear-time patterns): the values
   under keys that end in a credential's name (`token`, `password`, `api_key`, `authorization`, `secret`, `cookie`,
   `private_key` and the like), and text shaped like a bearer or basic credential, a JWT, a GitHub token, an `sk-` key,
   an AWS key id, a Slack token, a private key block, a password in a URL, or a `password=` / `token=` pair. Redaction
   runs before the cut, so a cut cannot leave the front half of a secret. **It is a filter, not a guarantee**: a secret in
   a shape no rule knows passes through. That is why the next two points exist.
4. **It is on by default and can be turned off** (the owner's default). Until the YAML configuration exists
   (plan 10, S8/S9; the key will be `steps.recordToolIo`), the switch is the environment variable
   `ORCH_STEPS_RECORD_IO` (`true` by default; `false`, `0`, `no`, `off` turn it off; anything else is refused at startup,
   exit 78). Off, a step is its label and detail only, as before this ADR. The switch acts in `orch-app`, before the core
   sees the input, because the core is pure and has no configuration (invariant 5).
5. **The parse is lenient.** An `input` that is not an object, or an `output` without a string `text`, is dropped and
   **the step is kept**, unlike the members that identify the step (a bad `id`, `label` or `state` is not a step). A
   bad payload never costs the step.
6. **Steps the orchestrator reports itself** (the relay of ADR 0024, an agent asked through `ask_agent`) fill `input` and
   `output` from the call they relay, through the same door with the same bounds and redaction. **This overrides the
   plan that said arguments and results of relayed calls are not logged** (plan 05, section 2.2): they are, redacted
   and capped, unless the switch is off.

## Consequences

- The web can open a tool step and show what was asked and what came back (S3). The contract it relies on:
  `input` is an object of at most 4 KiB or the `_cut` marker; `output.text` is plain text of at most 8 KiB, with
  `truncated`, `bytes` and `error` when they apply; both are untrusted and drawn as text.
- The log grows by at most 2 MiB per job, and the export carries the members (the format is unchanged, version 1,
  additive). A deployment that does not want tool arguments in its log sets the switch; a deployment that keeps them
  must treat the log as sensitive, which the chat already is.
- A secret in an unknown shape can reach the log. The ADR does not hide it: the filter's corpus test pins what it
  catches and what it lets through, and a new shape is a rule and a test.
- Agents must opt in by sending the members: adam-rs (its ADR 0011) is the first. Every other agent is unchanged.
- The relay (ADR 0024, not built) and `ask_agent` (ADR 0026) have a requirement: fill both members under these rules.
- Required elsewhere: the core types and `redact`, the adapter's lenient parse, the projection's activity content,
  `chat-api.yaml`, [`steps-v1.md`](../api/steps-v1.md), [`agui.md`](../api/agui.md), the goldens, the web (S3) and
  adam-rs (A1).

## Alternatives rejected

- **A separate event (`step_io`) per call.** It would double the events of a step and need a migration; two members of
  one event keep a step's story in one place and the coalescing rules unchanged.
- **Recording nothing, or only on failure.** The owner wants the arguments and the result of every call to open on
  demand, not only of the ones that failed.
- **Storing the bytes outside the log and keeping a reference** (the artifact store, plan 10 section 3.3). It is the
  right home for files, and it would make a step's input unavailable to a reader of the log alone; at 12 KiB a step it
  is not needed.
- **Redacting only in the agent.** An agent does not know what it does not know, and the orchestrator reads agents it
  does not own.
- **A hard failure when a payload is malformed.** A bad payload would cost a step the person needs to see; dropping the
  member loses the least.
