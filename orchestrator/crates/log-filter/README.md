# orch-log-filter

The log filter of the orchestrator's processes: what `RUST_LOG` means, in one place, so that the binary and
the tests that search the logs for a secret use the same function.

```rust
let filter = orch_log_filter::env_filter(std::env::var("RUST_LOG").ok().as_deref());
```

## The rule

`rmcp`, the MCP library, logs what it sees, and a server a person typed can make it say what it likes:

| Level | What `rmcp` logs | What a stranger's server can put in it |
|---|---|---|
| `info` | `Service initialized as client peer_info=...` | the server's own `instructions`, which can echo the `Authorization` it was sent |
| `warn`, `error` | a failed request (`fail to get common stream`, `HTTP 500`), a message before the handshake (`Received unexpected message`) | the **URL** of the request (a person's URL can hold a secret in its path), any message the server sends |
| `debug`, `trace` | every message | everything |

The orchestrator's own lines (`orch_tools_mcp`, `orch_surface_thread_tools`) say what happened with no URL and
no word the server chose, so the library's are off:

| `RUST_LOG` | The filter |
|---|---|
| unset or empty | `info,rmcp=off` (`DEFAULT_FILTER`) |
| `info` (the chart's `logLevel`) | `info,rmcp=off` |
| `trace` | `trace,rmcp=off` |
| names `rmcp`: a directive whose target is `rmcp` or starts with `rmcp::` (`info,rmcp=warn`) | as written. The operator asked for the library's lines; where people add servers this puts a stranger's words and the URL of a failed request in the logs |
| does not parse | `info,rmcp=off` |

## API at a glance

| Item | What |
|---|---|
| `DEFAULT_FILTER` | `"info,rmcp=off"` |
| `effective(Option<&str>) -> String` | the filter as text, for the value of `RUST_LOG` |
| `env_filter(Option<&str>) -> EnvFilter` | the `tracing-subscriber` filter for it |
| `env_filter_from_env() -> EnvFilter` | `env_filter` of the process's `RUST_LOG` |

Used by the binary (`bin/orchestrator/src/logging.rs`) and, as a dev-dependency, by the sentinel tests of
`orch-surface-thread-tools` (`user_relay_secret.rs`, `user_relay_logs_*.rs`).

## Tests

`cargo test -p orch-log-filter`: the rule for each shape of `RUST_LOG` (unset, empty, a bare level, an
operator's filter, one that names `rmcp` in four ways, one that does not parse) and, through a real
subscriber, that no event of target `rmcp::...` reaches the log at any level unless the operator named it.
