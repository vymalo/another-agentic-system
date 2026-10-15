//! The log filter of the orchestrator's processes (ADR 0046, slice 5a review).
//!
//! The binary reads `RUST_LOG` through [`env_filter`], and the tests that search the logs for a
//! secret build their subscriber with the same function, so the filter they prove clean is the one
//! the service runs with.
//!
//! # Why `rmcp` is off
//!
//! `rmcp`, the MCP library, logs what it sees: at `info` a line holding the server's own
//! `instructions` (`Service initialized as client`), at `warn` and `error` the **URL** of a request
//! that failed (a person's URL can carry a secret in its path) and the pre-handshake messages a
//! server sends, and at `debug` and `trace` every message. A server a person typed can make any of
//! these say what it likes, the credential the orchestrator lent it included. The orchestrator's own
//! lines say what happened (`orch_tools_mcp`, `orch_surface_thread_tools`), without a URL and
//! without a word the server chose, so the library's are off:
//!
//! - with `RUST_LOG` unset the filter is [`DEFAULT_FILTER`], `info,rmcp=off`;
//! - with `RUST_LOG` set, `rmcp=off` is **added** to the operator's filter, unless the filter names
//!   `rmcp` (a directive whose target is `rmcp` or starts with `rmcp::`), which is the operator
//!   saying they want the library's lines, and then it is theirs whole.
//!
//! A chart that renders `RUST_LOG=info` (its `logLevel`) is therefore `info,rmcp=off`. Naming
//! `rmcp` in a deployment that lets people add servers puts what a stranger's server says, and the
//! URL of a request that failed, in the logs.

use tracing_subscriber::EnvFilter;

/// The filter when `RUST_LOG` is not set (or is empty): `info`, and the MCP library off.
pub const DEFAULT_FILTER: &str = "info,rmcp=off";

/// The directive added to an operator's filter that does not name `rmcp`.
const RMCP_OFF: &str = "rmcp=off";

/// The directives of `spec`: split at the commas that are not inside `[...]` or `{...}` (a span's
/// field filter may hold commas).
fn directives(spec: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0_i32;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, c) in spec.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            ',' if depth <= 0 => {
                parts.push(&spec[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&spec[start..]);
    parts.into_iter()
}

/// Whether `directive` is about `rmcp`: its target (what is before `=`, `[` or `{`) is `rmcp` or a
/// path under it.
fn names_rmcp(directive: &str) -> bool {
    let target = directive
        .split(['=', '[', '{'])
        .next()
        .unwrap_or_default()
        .trim();
    target == "rmcp" || target.starts_with("rmcp::")
}

/// The filter the process runs with, as text, for `rust_log` (the value of `RUST_LOG`, if any):
/// [`DEFAULT_FILTER`] for none, the operator's filter as it is when it names `rmcp`, else the
/// operator's filter with `rmcp=off` added.
#[must_use]
pub fn effective(rust_log: Option<&str>) -> String {
    match rust_log.map(str::trim).filter(|spec| !spec.is_empty()) {
        None => DEFAULT_FILTER.to_owned(),
        Some(spec) if directives(spec).any(names_rmcp) => spec.to_owned(),
        Some(spec) => format!("{spec},{RMCP_OFF}"),
    }
}

/// The [`EnvFilter`] for `rust_log` ([`effective`]). A value that does not parse is the default,
/// as `EnvFilter::try_from_default_env` fell back before.
#[must_use]
pub fn env_filter(rust_log: Option<&str>) -> EnvFilter {
    EnvFilter::try_new(effective(rust_log)).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// [`env_filter`] of the `RUST_LOG` of the environment.
#[must_use]
pub fn env_filter_from_env() -> EnvFilter {
    env_filter(std::env::var("RUST_LOG").ok().as_deref())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex, PoisonError};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[test]
    fn no_filter_is_the_default_and_the_default_has_the_library_off() {
        assert_eq!(effective(None), "info,rmcp=off");
        assert_eq!(effective(Some("")), "info,rmcp=off");
        assert_eq!(effective(Some("  \n")), "info,rmcp=off");
    }

    #[test]
    fn an_operators_filter_that_does_not_name_the_library_gets_it_switched_off() {
        // the chart renders `logLevel: info` as RUST_LOG=info
        assert_eq!(effective(Some("info")), "info,rmcp=off");
        assert_eq!(effective(Some("trace")), "trace,rmcp=off");
        assert_eq!(
            effective(Some("warn,orch_app=debug")),
            "warn,orch_app=debug,rmcp=off"
        );
        // a name that only starts with the same letters is not the library
        assert_eq!(
            effective(Some("rmcp_other=debug")),
            "rmcp_other=debug,rmcp=off"
        );
        assert_eq!(
            effective(Some("info,my_rmcp::x=debug")),
            "info,my_rmcp::x=debug,rmcp=off"
        );
    }

    #[test]
    fn an_operator_who_names_the_library_has_their_filter_whole() {
        for spec in [
            "info,rmcp=warn",
            "info,rmcp=debug",
            "rmcp=trace",
            "info,rmcp::transport=debug",
            "info, rmcp = warn",
            "rmcp[span{field=1,other=2}]=info,warn",
        ] {
            assert_eq!(effective(Some(spec)), spec, "{spec}");
        }
    }

    #[test]
    fn a_filter_that_does_not_parse_is_the_default() {
        let filter = env_filter(Some("info,=,,[bad"));
        assert_eq!(filter.to_string(), env_filter(None).to_string());
    }

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl MakeWriter<'_> for Captured {
        type Writer = Captured;

        fn make_writer(&self) -> Captured {
            self.clone()
        }
    }

    /// What reaches the log under `rust_log`.
    fn logged(rust_log: Option<&str>) -> String {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(env_filter(rust_log))
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "rmcp::transport", "an error of the library");
            tracing::warn!(target: "rmcp::service", "a warning of the library");
            tracing::info!(target: "rmcp::service", "an info of the library");
            tracing::trace!(target: "rmcp::service", "a trace of the library");
            tracing::info!(target: "orch_app", "an info of ours");
            tracing::trace!(target: "orch_app", "a trace of ours");
        });
        String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn the_library_says_nothing_at_any_level_unless_the_operator_names_it() {
        for rust_log in [None, Some("info"), Some("debug"), Some("trace")] {
            let out = logged(rust_log);
            assert!(!out.contains("of the library"), "{rust_log:?}: {out}");
            assert!(out.contains("an info of ours"), "{rust_log:?}: {out}");
        }
        assert!(logged(Some("trace")).contains("a trace of ours"));
        // naming it is the operator's choice, and theirs whole
        let named = logged(Some("info,rmcp=warn"));
        assert!(
            named.contains("an error of the library") && named.contains("a warning of the library"),
            "{named}"
        );
        assert!(!named.contains("an info of the library"), "{named}");
    }
}
