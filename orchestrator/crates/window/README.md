# orch-window

The `Window` port (ADR 0092, [`docs/api/windows-v1.md`](../../../docs/api/windows-v1.md)): a view onto something a run is
producing. Slice 2 of the windows epic: the port, its URL guard, a conformance testkit and a fake adapter. No network, no
real adapter yet.

## API at a glance

| Item | What |
|---|---|
| `Window` | `open`, `look`, `act`, `close`, each scoped to a `ThreadId` |
| `Action`, `Frame`, `WindowId`, `Opened` | the closed action set; a frame handed over by artifact reference |
| `WindowError` | `WindowsOff`, `NotMainCaller`, `WindowUnknown`, `UrlRefused`, `TooManyWindows`, `Invalid` |
| `check_url(url, resolved)` | the guard every adapter applies to the **resolved** addresses before connecting: refuses private, loopback, link-local, metadata, CGNAT and unique-local addresses and their embedded-IPv4 forms (uses `orch_ports::is_public_address`), non-http(s), over-long URLs and a name that resolved to nothing |
| constants | `MAX_FRAME_BYTES` (2 MiB), `MAX_WINDOWS_PER_THREAD` (4), `MAX_ACTIONS_PER_CALL` (8), `WINDOW_TTL_SECS` (900), `MAX_URL_BYTES` (2048) |

## Testkit (feature `testkit`)

`window_conformance!(make, expire = path)` generates one test per case: the guard's address classes through the public
`open`, the URL length, the per-thread cap (also under concurrent opens), idempotent close, windows scoped to a thread,
`look` of an expired window, the action bound, and frames by reference. `FakeWindow` passes it; `FakeWindow::unguarded()`
exists only so the guard's cases can be seen red (`tests/fake.rs` holds that as a `should_panic` test).

Not covered by the kit: that an adapter resolves names itself and judges every address it got (the fake's name table
checks `check_url`, but a real adapter needs its own test); the loopback exception of ADR 0092 decision 5a.

Run: `cargo test -p orch-window`.
