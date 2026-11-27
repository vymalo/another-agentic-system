# Windows (v1)

A **window** is a view onto something a run is producing — a page it serves, an app it is building — that a person can
watch and an agent can drive. This page is the contract: the `Window` port, the four tools the orchestrator serves, what
a frame is, what a client may draw, and what is refused.

Design: [ADR 0092](../decisions/0092-a-window-is-a-port-on-the-runs-side-and-the-orchestrator-serves-its-tools.md).
**Status: proposed 2026-11-24. Built so far: the port, its testkit and a fake adapter (slice 2), and the four tools on the
fake adapter (slice 3, 2026-11-28; [what slice 3 does not do](#what-slice-3-built-and-what-it-did-not)).** No real adapter exists, so
no agent can open a real page. Slice 1 of the windows epic
([#339](https://github.com/vymalo/another-agentic-system/issues/339)); slices 2 to 10 are in the ADR.

## Constants

| Name | Value | Why |
|---|---|---|
| `MAX_FRAME_BYTES` | 2 MiB | One frame is a picture, not a film. A larger frame is refused, not scaled |
| `MAX_WINDOWS_PER_THREAD` | 4 | A person watches a page or two, not a desktop |
| `MAX_ACTIONS_PER_CALL` | 8 | One `window_act` is a short script (a click, a few keys), never a session |
| `WINDOW_TTL_SECS` | 900 | A window left alone closes itself; the run does not hold a browser for an hour |
| `MAX_URL_BYTES` | 2048 | The URL a `window_open` may name |

## The port

The port lives **on the server side of the run** (ADR 0092 decision 1). Its four calls are the whole surface:

| Call | Input | Answer |
|---|---|---|
| `open` | a URL, or a page the run serves | a window id, and the first frame |
| `look` | a window id | one frame (decision 3: a screenshot, not a stream) |
| `act` | a window id and up to `MAX_ACTIONS_PER_CALL` actions | the frame after the actions |
| `close` | a window id | nothing; idempotent |

An action is `click {x, y}`, `type {text}`, `scroll {dy}` or `key {name}`, in that closed set. A window's **state is the
run's**: closing the client does not close the window, and a client that comes back reads the current frame.

## The tools

Four built-ins of [`thread-tools/v1`](thread-tools-v1.md), served by the orchestrator (ADR 0092 decision 2) — listed to a
thread's **main caller** only, and each call is a step of the thread with its input and output.

### `window_open`

```json
{ "url": "http://127.0.0.1:5173/", "label": "the dev server" }
```

Answers `{ "window": "w-3f2a", "frame": { … } }`. The URL is checked before anything fetches it (ADR 0046's rule):
**private, loopback and link-local addresses are refused from the orchestrator's side**, and `127.0.0.1` is accepted
only when the deployment's adapter captures **inside the run** (ADR 0092 decision 5a), which is a property of the
adapter, not of the caller.

### `window_look`

```json
{ "window": "w-3f2a" }
```

Answers `{ "frame": { … } }`, or `{ "frame": null, "state": "closed" }` for a window that expired.

### `window_act`

```json
{ "window": "w-3f2a", "actions": [ { "click": { "x": 120, "y": 48 } }, { "type": { "text": "hello" } } ] }
```

Answers the frame after the actions. Over `MAX_ACTIONS_PER_CALL` is `invalid`, never a truncation.

### `window_close`

```json
{ "window": "w-3f2a" }
```

Answers `{ "closed": true }`; **idempotent**, so a retry is safe and an unknown id is `closed: false` with the same
success.

## A frame

```json
{ "mediaType": "image/png", "width": 1280, "height": 800, "scale": 1, "url": "artifact:<id>", "at": "2026-11-24T20:15:04Z" }
```

The bytes are an **artifact** ([ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md)): a frame is
handed over by reference, cut at `MAX_FRAME_BYTES`, and never inlined into a message or a step.

## The client side

- **An image, sandboxed.** A frame is drawn as an image. Any interactive surface served to a client is a **separate
  origin** with `sandbox="allow-scripts"` and a CSP whose `connect-src` is `'none'` (ADR 0092 decision 7): framed content
  cannot reach the API and cannot touch a person's credential. There is no `allow-same-origin`.
- **Closing is not losing.** The client re-reads the window when it returns; the run kept going (ADR 0092 decision 6).
- **The capability is the deployment's.** `GET /api/config` says whether windows are served and which adapters exist;
  a client draws nothing where `windows` is absent.

## Errors, and which wins

| Code | When |
|---|---|
| `windows_off` | The deployment serves no windows (`GET /api/config` said so; a call still arriving is refused here) |
| `not_main_caller` | The caller is not the thread's `main` |
| `window_unknown` | `look`, `act`, `close` naming a window this thread has not got (or one that expired) |
| `url_refused` | `open`: the URL is private, loopback or link-local **and the adapter cannot capture inside the run**, or it is over `MAX_URL_BYTES`, or not `http(s)` |
| `too_many_windows` | `open` over `MAX_WINDOWS_PER_THREAD` |
| `invalid` | Actions outside the closed set, or over `MAX_ACTIONS_PER_CALL`; a frame over `MAX_FRAME_BYTES` |

Checked left to right in that table: a caller that is not `main` learns nothing about URLs, and a deployment with
windows off learns nothing about windows.

## What slice 3 built, and what it did not

Built: `WindowTools` in `orchestrator/crates/surface-thread-tools` (`src/window.rs`), a provider of the thread's endpoint over the `Window`
port; the binary mounts it with the **fake adapter** of `orch-window` only in a build with the Cargo feature `window-fake` (off by
default and not in the image that ships); `GET /api/config` then has `windows: {"adapters": ["fake"]}` and a client draws nothing where it
is absent. Proved by `crates/surface-thread-tools/tests/windows.rs` (open, look, act and close; every code; an asked agent refused
**before any adapter call**, shown by a counting adapter) and by `dev/windows-e2e.sh` on the stack.

What the code does where this page is silent or says otherwise:

- **A provider, not a variant of the closed enum** (ADR 0092 decision 2 says "join the closed enum"): the built-in set's `call` takes only the
  application, and the tools need the adapter, which the composition root picks. The listing and the order are the same: after the built-ins.
- **`windows_off` is never answered.** With no provider mounted the name is unknown (`-32602`); with it mounted, windows are on.
- **`look` of a window that is gone is an answer** (`{"frame": null, "state": "closed"}`), as its section says, not `window_unknown`; `act`
  on one is `window_unknown`; `close` of one is `{"closed": false}`.
- **Every failure is a tool error** (`isError: true`) whose text is `<code>: <reason>`, as `request_upload`'s are; the arguments that do not
  parse are `invalid`. `unavailable` is the tool's own: a call that took over 30 seconds.
- **`label` is read and dropped**: the port has no place for it (at most 200 bytes).
- **The tier of the message is not read.** The access tier and the network tier of a message are not in the thread token, the orchestrator
  enforces neither (ADR 0061, and ADR 0062 is accepted and unbuilt), so a window tool is not gated by them. What gates a call today is the
  caller (`main` only), the adapter's URL guard, the per-thread cap and the action bound. A tier gate needs a decision on what each tier
  means for a window, and a way to read the message's tier from a call: neither is made here.
- **No step is recorded by the orchestrator** for a window call (ADR 0092 decision 2 asks for one): the relay's step machinery is private to
  the relay, and what the agent reports of its own tool calls is its own.

## What the server logs

The tool call and its outcome, **as a step** — the tool's name, its input, its answer — cut and redacted as any step is
([ADR 0030](../decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)). A frame's **bytes are never
logged**, only its artifact reference; a URL is logged as the step's input like any other argument, and a credential a
page might contain is never in the step.

## Limitations (what this is not)

- **Not a remote desktop.** One window, one page, a short script of actions.
- **Not live.** A frame is sampled (ADR 0092 decision 3); a stream would need the platform's ingress path
  (decision 5b), which does not exist.
- **Not a browser for third-party sign-in.** Signing in on a person's behalf is the credentials epic's, and is out of
  scope here.
- **Not per-client.** A window belongs to the run and the thread; two clients see the same window, not two.
