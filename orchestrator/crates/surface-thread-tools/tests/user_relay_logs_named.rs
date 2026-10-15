//! The control of the two files beside it: **an operator who names `rmcp`** (`RUST_LOG=info,rmcp=info`)
//! asks for the library's lines, gets them whole, and the scenario that is clean without that is not
//! (ADR 0046, slice 5a review). It proves the other two prove something: the hostile server does make
//! the library log what it was sent, and the capture does see it.
//!
//! This is documented as the operator's choice (`orch-log-filter`, the binary's README, the chart's):
//! where people add servers, naming `rmcp` at `info` or below puts a stranger's words in the logs (the
//! `instructions` of its handshake, which it can use to say the lent bearer back). At `warn` the
//! library's lines are free of the URL and of the server's body since the client of the servers people
//! name takes them out of its errors (`orch-tools-mcp`, `wire.rs`): `info,rmcp=warn`, what the default
//! was before, is clean too (`user_relay_logs_warn.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

#[tokio::test]
async fn naming_the_library_at_info_brings_back_its_lines_and_what_they_hold() {
    let run = support::hostile_logs::run(Some("info,rmcp=info")).await;
    let from_the_library: Vec<&str> = run
        .logs
        .lines()
        .filter(|line| line.contains(" rmcp::"))
        .collect();
    assert!(
        !from_the_library.is_empty(),
        "the library logged nothing at info: the scenario does not provoke it\n{}",
        run.logs
    );
    // `Service initialized as client peer_info=...` holds the server's `instructions`
    let leaked = support::hostile_logs::leaks(&run.logs);
    assert!(
        !leaked.is_empty(),
        "the library's lines hold none of the secrets: the other two files prove nothing\n{}",
        run.logs
    );
    assert!(
        leaked.iter().all(|line| line.contains(" rmcp::")),
        "a line of ours holds a secret: {leaked:#?}"
    );
}
