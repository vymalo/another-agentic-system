# agui-conformance

Reads the AG-UI golden streams through the reference client and fails on any error or warning. The
Rust side checks that every event is valid against the vendored AG-UI schema
(`orch_agui_proto::testkit`) and that the projection's streams follow the ordering rules in a model
of the reference consumer; this is the real consumer, `@ag-ui/client` **1.0.0** (pinned, with a
lockfile), so the two cannot drift apart unnoticed.

```sh
npm --prefix tools/agui-conformance ci      # once per clone
node tools/agui-conformance/check.mjs       # every docs/api/examples/agui/*.agui.json
```

## What it does

For each `docs/api/examples/agui/<name>.agui.json` (an array of `{id?, event}`, written by
`orch-agui-projection` and, for the `run-*` and `connect-*` files, captured over real HTTP by `orch-e2e`; see [`docs/api/examples`](../../docs/api/examples/README.md)) the frames
are framed as SSE the way the surface writes them (`id:` and `data:` lines, LF, a keepalive
comment after each run) and read through the reference `HttpAgent` pipeline:

`runHttpRequest` → `transformHttpEventStream` (`parseSSEStream`) → `enforceEvents` →
`transformChunks` → `verifyEvents` → `defaultApplyEvents`

1. **The whole stream**, as the connect endpoint sends it (several runs on one stream), through
   `connectAgent`. What the client ends up holding (messages, state, pending interrupts, run
   outcomes, subagent invocations) must equal [`expected/<name>.json`](expected/). After an
   intended change of the projection, review the golden diff and then `npm run update`.
2. **Each run alone**, through `runAgent`, as the run endpoint sends one; it must end as it did
   in the stream.

`enforceEvents` reports what it strips or drops as console warnings (an unknown member, an event
it does not know); any warning fails the check. A **self-test** runs first: streams that break the
protocol (an event before `RUN_STARTED`, a message or subagent left open at `RUN_FINISHED`, a run
inside a run, an event after the run, an unknown member) must be rejected, so a green run means the
harness can say no.

## Notes

- Node 22 or newer; CI uses Node 24 (`.github/workflows/docs.yml`, job `AG-UI goldens`).
- The connect stream is our extension of the transport
  ([`docs/api/agui.md`](../../docs/api/agui.md)); `connectAgent` is the reference client's own hook
  for a stream that carries several runs, and its `connect` is overridden here to read the canned
  SSE response, which is all the extension asks of a client.
- Bumping `@ag-ui/client` is a reviewed change: update `package.json` and the lockfile together,
  and expect `expected/*.json` to need review.
