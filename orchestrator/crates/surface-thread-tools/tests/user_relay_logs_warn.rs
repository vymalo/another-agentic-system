//! `RUST_LOG=info,rmcp=warn`, what the default filter was before the slice 5a review: an operator who
//! names the library at `warn` gets its ERROR and WARN lines (a failed stream, a failed end of
//! session), and they hold **no URL and no word of the server's**, because the client of the servers
//! people name takes both out of its errors (`orch-tools-mcp`, `wire.rs`). The default filter does not
//! rely on that alone (it switches the library off); this is the second wall.
//!
//! A process of its own, as every file that captures the log.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

#[tokio::test]
async fn naming_the_library_at_warn_is_clean_because_its_errors_hold_no_url_and_no_body() {
    let run = support::hostile_logs::run(Some("info,rmcp=warn")).await;
    assert!(
        run.logs.lines().any(|line| line.contains(" rmcp::")),
        "the library logged nothing at warn: the scenario does not provoke it\n{}",
        run.logs
    );
    let leaked = support::hostile_logs::leaks(&run.logs);
    assert!(
        leaked.is_empty(),
        "secrets in the library's lines: {leaked:#?}"
    );
}
