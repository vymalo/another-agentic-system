//! The same with **`RUST_LOG=info`**, which is what the chart renders for `logLevel: info` and which
//! replaces the binary's default whole: the filter is `info,rmcp=off`, and nothing a stranger's MCP
//! server was lent or said is in the log, `rmcp::` lines included (ADR 0046, slice 5a review).
//!
//! Before the fix `RUST_LOG=info` turned the library's `info` on: `Service initialized as client
//! peer_info=...` holds the server's own `instructions`, and a server can use them to say the lent
//! bearer back.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

#[tokio::test]
async fn rust_log_info_logs_no_secret_of_a_hostile_server() {
    let run = support::hostile_logs::run(Some("info")).await;
    let leaked = support::hostile_logs::leaks(&run.logs);
    assert!(leaked.is_empty(), "secrets in the log: {leaked:#?}");
    assert!(
        !run.logs.contains(" rmcp::"),
        "the MCP library logs at RUST_LOG=info:\n{}",
        run.logs
    );
}
