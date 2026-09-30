//! Logging setup: `tracing` to stdout, as JSON or text, filtered by `RUST_LOG`, with the
//! process's role and instance on **every** line.
//!
//! Role and instance are what tell two replicas' lines apart once a log collector merges them.
//! A root span would carry them for the code that runs inside it, but the dispatcher's workers
//! are spawned tasks that do not inherit it, so the fields are added by the event formatter
//! instead ([`WithProcessFields`]): it wraps the stock formatter and puts them in front of
//! whatever it writes. In JSON they are the first two keys of the line, `{"role":"worker",
//! "instance":"w1","timestamp":...}`; in text the line starts `role=worker instance=w1 `.

use std::fmt;

use tracing::{Dispatch, Event, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::{Format, FormatEvent, FormatFields, JsonFields, Writer};
use tracing_subscriber::fmt::{FmtContext, MakeWriter};
use tracing_subscriber::registry::LookupSpan;

use crate::config::LogFormat;

/// The filter when `RUST_LOG` is not set: `info`, and the MCP library's own chatter (a line per
/// request at `info`/`debug`) at `warn`. `RUST_LOG` replaces it whole.
const DEFAULT_FILTER: &str = "info,rmcp=warn";

/// Who is logging: the fields put on every line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFields {
    /// `ORCH_ROLE`, as `all`, `control-plane` or `worker`.
    pub role: &'static str,
    /// The instance id: the lease owner of this process's outbox claims.
    pub instance: String,
}

/// An event formatter that puts [`ProcessFields`] in front of what `inner` writes. Without
/// fields (the configuration did not parse, so there is no role yet) it is `inner` unchanged.
struct WithProcessFields<F> {
    inner: F,
    fields: Option<ProcessFields>,
    json: bool,
}

/// `value` as a JSON string literal.
fn json_string(value: &str) -> String {
    // Serialising a string cannot fail.
    serde_json::to_string(value).unwrap_or_else(|_| String::from("\"\""))
}

impl<S, N, F> FormatEvent<S, N> for WithProcessFields<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'writer> FormatFields<'writer> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let Some(fields) = &self.fields else {
            return self.inner.format_event(ctx, writer, event);
        };
        if !self.json {
            write!(writer, "role={} instance={} ", fields.role, fields.instance)?;
            return self.inner.format_event(ctx, writer, event);
        }
        // The stock JSON formatter writes one object and a newline. Format into a buffer and
        // splice the two keys in after the opening brace; anything that is not an object
        // (it always is) passes through unchanged rather than being corrupted.
        let mut line = String::new();
        self.inner
            .format_event(ctx, Writer::new(&mut line), event)?;
        match line.strip_prefix('{') {
            Some(rest) => {
                let separator = if rest.starts_with('}') { "" } else { "," };
                write!(
                    writer,
                    "{{\"role\":{},\"instance\":{}{separator}{rest}",
                    json_string(fields.role),
                    json_string(&fields.instance),
                )
            }
            None => writer.write_str(&line),
        }
    }
}

/// The subscriber for `format`, writing to `make_writer` and filtered by `filter`.
fn dispatch<W>(
    format: LogFormat,
    fields: Option<ProcessFields>,
    filter: EnvFilter,
    make_writer: W,
) -> Dispatch
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(make_writer);
    match format {
        LogFormat::Json => Dispatch::new(
            builder
                .fmt_fields(JsonFields::new())
                .event_format(WithProcessFields {
                    inner: Format::default().json(),
                    fields,
                    json: true,
                })
                .finish(),
        ),
        LogFormat::Text => Dispatch::new(
            builder
                .event_format(WithProcessFields {
                    inner: Format::default(),
                    fields,
                    json: false,
                })
                .finish(),
        ),
    }
}

/// Installs the global subscriber: `format` on stdout, filtered by `RUST_LOG` (default `info,rmcp=warn`),
/// with `fields` on every line. Keeps the subscriber already installed, if any.
pub fn init(format: LogFormat, fields: Option<ProcessFields>) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let _ =
        tracing::dispatcher::set_global_default(dispatch(format, fields, filter, std::io::stdout));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;

    /// A `MakeWriter` that appends to a shared buffer.
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Buf {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buf {
        type Writer = Buf;

        fn make_writer(&'a self) -> Buf {
            self.clone()
        }
    }

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(
                self.0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
            )
            .unwrap()
        }
    }

    fn fields(instance: &str) -> Option<ProcessFields> {
        Some(ProcessFields {
            role: "worker",
            instance: instance.to_owned(),
        })
    }

    /// Runs `emit` under a subscriber configured like the service's and returns what it wrote.
    fn logged(format: LogFormat, fields: Option<ProcessFields>, emit: impl FnOnce()) -> String {
        let buf = Buf::default();
        let subscriber = dispatch(format, fields, EnvFilter::new("info"), buf.clone());
        tracing::dispatcher::with_default(&subscriber, emit);
        buf.text()
    }

    #[test]
    fn every_json_line_starts_with_role_and_instance() {
        let out = logged(LogFormat::Json, fields("w1"), || {
            tracing::info!(answer = 42, "first");
            let span = tracing::info_span!("outbox", id = "row-1");
            let _guard = span.enter();
            tracing::warn!("second");
        });
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        for line in &lines {
            assert!(
                line.starts_with(r#"{"role":"worker","instance":"w1","#),
                "{line}"
            );
        }
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["role"], "worker");
        assert_eq!(first["instance"], "w1");
        assert_eq!(first["level"], "INFO");
        assert_eq!(first["fields"]["message"], "first");
        assert_eq!(first["fields"]["answer"], 42);
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["role"], "worker");
        assert_eq!(second["fields"]["message"], "second");
        assert_eq!(second["span"]["name"], "outbox");
        assert_eq!(second["span"]["id"], "row-1");
    }

    #[test]
    fn the_default_filter_keeps_the_mcp_librarys_chatter_at_warn() {
        let buf = Buf::default();
        let subscriber = dispatch(
            LogFormat::Json,
            None,
            EnvFilter::new(DEFAULT_FILTER),
            buf.clone(),
        );
        tracing::dispatcher::with_default(&subscriber, || {
            tracing::info!(target: "rmcp::transport", "a line per request");
            tracing::warn!(target: "rmcp::transport", "a real problem");
            tracing::info!(target: "orch_app", "ours");
        });
        let out = buf.text();
        assert!(!out.contains("a line per request"), "{out}");
        assert!(
            out.contains("a real problem") && out.contains("ours"),
            "{out}"
        );
    }

    #[test]
    fn an_instance_with_quotes_and_backslashes_stays_valid_json() {
        let instance = r#"pod "a"\b"#;
        let out = logged(LogFormat::Json, fields(instance), || {
            tracing::info!("hello");
        });
        let line: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(line["instance"], instance);
        assert_eq!(line["fields"]["message"], "hello");
    }

    #[test]
    fn without_fields_the_line_is_the_stock_one() {
        let out = logged(LogFormat::Json, None, || tracing::info!("early"));
        let line: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert!(line.get("role").is_none() && line.get("instance").is_none());
        assert_eq!(line["fields"]["message"], "early");
    }

    #[test]
    fn a_text_line_starts_with_role_and_instance() {
        let out = logged(LogFormat::Text, fields("w1"), || {
            tracing::info!("hello text");
        });
        assert!(out.starts_with("role=worker instance=w1 "), "{out}");
        assert!(out.contains("hello text"), "{out}");
        assert_eq!(out.lines().count(), 1, "{out}");
    }

    #[test]
    fn a_text_line_without_fields_has_no_prefix() {
        let out = logged(LogFormat::Text, None, || tracing::info!("early"));
        assert!(!out.contains("role="), "{out}");
        assert!(out.contains("early"), "{out}");
    }
}
