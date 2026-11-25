# ADR 0092 — A window is a port on the server side of a run, and the orchestrator serves its tools

- **Status:** **Proposed (2026-11-24).** Slice 1 of the windows epic ([#339](https://github.com/vymalo/another-agentic-system/issues/339)). Nothing is built; slices 2 to 10 are listed below. It amends no invariant.
- **Why it is proposed and not accepted:** it settles a seam and a set of rules, and it names what a later slice must decide with the platform (decision 5). No owner decision is needed for the seam itself, which is why it can be written now.

## Context

The epic asks for **windows**: a view onto something a run is producing — a page it serves, an app it is building — that a person can watch and an agent can drive, which survives the client closing and the phone locking.

`main` already carries almost all of the machinery, and this ADR's job is to say which machinery and in what shape:

- **a built-in tool set on the thread-tools relay.** `docs/api/thread-tools-v1.md:425-447` is explicit: `tools/list` answers the **built-in** tools first and then each provider's, `tools/call` offers a name to the built-ins first, and the built-in set is a **closed enum**. `ask_agent`, `turn_output` and `get_ui_catalog` are built that way today (`orchestrator/crates/surface-thread-tools/src/tools.rs`). A window tool needs **no new server**.
- **a browser in the agent's own image.** `obscura` is pinned at 0.2.4 (upstream `1fccab2`, [ADR 0057](0057-a-browser-agent-an-adam-folder-with-obscura-as-its-sidecar.md), [adam ADR 0033](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0033-files-from-mcp-results-are-shared-files.md)) and now ships **inside the coder image** at `/opt/obscura/bin`, where the coder's own instructions already tell it to `obscura fetch <url> --screenshot shot.png`. `--allow-private-network` is what lets it read `127.0.0.1`.
- **run pods that cannot be reached from outside.** The platform's run pods are deny-by-default with **no ingress rule at all** (egress is DNS, the egress proxy on 3128 and `extraEgress`; no rule names an address range — the platform's D15c work). There is no path from the orchestrator to `127.0.0.1:<port>` inside a pod.

Those three facts decide the shape, and the third one decides the most.

## Decision

1. **One port, on the server side of the run.** `Window` — `open`, `look`, `act`, `close` — lives beside the run, and its **state is the run's**, never the client's. A client that closes, or a phone that locks, loses nothing: it re-reads the window when it comes back. This is the same split as child sessions: the thing exists in the log and on the run's side, and a client is a view of it.
2. **The tools are built-ins of `thread-tools/v1`, served by the orchestrator.** `window_open`, `window_look`, `window_act` and `window_close` join the closed enum. They are listed to a thread's **main caller** only, like the other built-ins, and each call is a **step** of the thread with its input and output ([ADR 0030](0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)), so the person can read what the agent did to the window.
3. **`look` is a screenshot first, and a frame stream only after measurement.** A window is looked at by asking for **one frame** — a raster the run produced — and not by streaming. A stream is a bandwidth and a lifecycle decision nobody has measured yet, so it is not taken here; the epic's slice 5 draws a window from screenshots for exactly this reason.
4. **The guard is on every URL, and it is [ADR 0046](0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md)'s rule.** A window may not open a private, loopback or link-local address **from the orchestrator's side**, and it may not be talked into one on the run's side either: the URL a tool accepts is checked before anything fetches it, and the check is on the resolved form, not the text. `--allow-private-network` exists for the *agent's own* loopback inside its pod (§5) and is never handed to a caller.
5. **How a dev server in a run is reached: it is not, today — and that is the decision.** With run pods carrying no ingress rule, the honest options are these two, and this ADR **takes the first and names the second as what a later slice must ask the platform for**:
   - **(a) capture inside the run.** For a page the run serves, the run's own browser (`obscura`, in the agent's image) loads `127.0.0.1:<port>` and hands the frame back as a **file artifact** ([ADR 0032](0032-files-from-agents-live-in-an-artifact-store.md)). It needs no ingress, no network rule and no platform change, and it is what slice 6 does.
   - **(b) a decided ingress path.** A live window onto a running dev server needs the platform to open **one port of one run pod to the orchestrator only** — a NetworkPolicy rule and a per-run Service. That is the platform's decision and its own card, not this ADR's; a frame stream (decision 3) would need it too.
6. **A locked phone loses nothing, and that is a property of decision 1, not a feature.** The run and its window go on; the client catches up from the log and the run's state. What *does* reach the person while the client is closed is a **notification**, under the notification epic's own opt-in, and this ADR adds nothing to that mechanism.
7. **The client draws it sandboxed, always.** A window's frame reaches a client as an **image**, and any interactive surface is a **separate origin** with `sandbox="allow-scripts"` and a CSP with `connect-src 'none'` — no credential of the person's is ever reachable from the framed content, and a frame cannot call back into the API. This is the same rule as a UI catalog surface ([ADR 0023](0023-ui-component-catalog-as-an-a2a-extension.md)) and it is not optional.
8. **The capability is advertised, never assumed.** `GET /api/config` says whether this deployment serves windows and which adapters it has (a headless one, and later a preview one), so a client draws the feature only where it exists, and an agent is told the tools rather than discovering they are absent.

## The three premises, verified before writing this

1. **A built-in server can be served by the relay** — **yes, and it needs no diagrams-epic slice**: the relay's built-in set is a closed enum answered before providers (`docs/api/thread-tools-v1.md:425-447`), so windows' tools are built-ins, exactly as `ask_agent` and `turn_output` are. `verified 2026-11-24` on `main`.
2. **obscura's tools can be wrapped by a port, and the pin moved** — `0.2.4` (`1fccab2`) per ADR 0057/0033, and the binary now ships in the coder image at `/opt/obscura/bin` (workspace image `1.98.1-cfd2917`), with `--allow-private-network` required for loopback. `verified 2026-11-24` in `another-adam-rs`.
3. **How the orchestrator reaches a dev server in a run pod** — **it does not, today**: the platform's run pods carry no ingress rule. `verified 2026-11-24` against the platform's run-pod networking (D15c) and `deploy/operator/templates/networkpolicy.yaml`'s `ingress: []` discipline. Decision 5 is the consequence.

## Slices

Following the epic's table ([#339](https://github.com/vymalo/another-agentic-system/issues/339)), one PR per row:

| Slice | What a person or operator can do after it | Repo | Depends on |
|---|---|---|---|
| 1. The design | Read this ADR and [`docs/api/windows-v1.md`](../api/windows-v1.md) | sys | the three premises above |
| 2. Port and testkit | A `Window` port with a conformance testkit, a fake adapter, no network | sys | 1 |
| 3. The tools | An agent opens, looks at, acts on and closes a window through MCP tools served by the orchestrator, on the fake adapter | sys | 2 |
| 4. The headless adapter | The tools drive obscura; the browser agent's folder keeps working | sys | 3 |
| 5. The viewer | The web draws a window read-only in a sandboxed frame, from screenshots; the client can close and come back | sys | 3; the artifacts preview runner |
| 6. Web preview | A window onto a page the run serves, **captured inside the run** (decision 5a) | sys, adam | 4; the path decided in decision 5 |
| 7. Phone frame | A Flutter app built for the web is shown in a phone frame | sys | 6 |
| 8. A locked phone | The run and its window go on while the client is closed; a notification says when the agent needs the person | sys | 5; the notification opt-in |
| 9. How agents use it | The coder's instructions and tool descriptions teach a model to use the window tools well | adam | 3; after the parallel tool calls part C merges |
| 10. Later | An Android package build in a run, shown in the viewer | sys, adam | 7 |

## Consequences

- **The epic's riskiest assumption is already answered, and cheaply.** The card feared its prerequisite (a built-in server over the relay) might not be settled; it is, on `main`, so slices 2–5 have no blocking dependency outside their own repo.
- **The ceiling is named, not hidden.** A *live* window onto a dev server is impossible today, and decision 5a gets slices 6 and 7 working without pretending otherwise. A person reading the feature will see sampled frames, not a live view, until the platform opens an ingress path.
- **Two repositories, one seam.** Slices 6, 9 and 10 touch `adam-rs` as well; the port is the contract between them, which is why the testkit (slice 2) comes before any adapter.

## Alternatives rejected

- **A separate MCP server for windows.** It would duplicate the relay's auth, its step reporting and its listing rules for no benefit; the built-in enum already exists and is the cheaper road (premise 1).
- **A live stream in slice 1's design.** Nobody has measured it, and it needs the platform change of decision 5b; designing a stream against an ingress path that does not exist would invent an interface for a network nobody agreed to.
- **Driving the person's own browser.** It would put a window's content in the person's origin and require their credentials; the epic puts third-party sign-in out of scope, and decision 7's sandbox is the opposite choice.
