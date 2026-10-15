//! The log of the service **with `RUST_LOG` unset** holds nothing of what a stranger's MCP server was
//! lent or said (ADR 0046, slice 5a review): not the credential, not the secret in the path of the
//! person's URL, whatever the server answers and whatever `rmcp`, the MCP library, would have logged
//! about it. The subscriber is built as `logging::init` builds it (`orch_log_filter`).
//!
//! Before the fix the default was `info,rmcp=warn`, and a server that answers a `GET` or a `DELETE`
//! with a 500 made the library log ERROR and WARN lines with the full URL.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

#[tokio::test]
async fn the_default_filter_logs_no_secret_of_a_hostile_server() {
    let run = support::hostile_logs::run(None).await;
    let leaked = support::hostile_logs::leaks(&run.logs);
    assert!(leaked.is_empty(), "secrets in the log: {leaked:#?}");
    assert!(
        !run.logs.contains(" rmcp::"),
        "the MCP library logs at the default filter:\n{}",
        run.logs
    );
}
