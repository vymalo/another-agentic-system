//! What the process tests share about the address the binary listens on.
//!
//! A test that picks a port with `bind("127.0.0.1:0")`, drops the listener and tells the binary to
//! listen there leaves a window in which another test (the tests of a binary run in parallel, a
//! dozen binaries at a time) is handed the same port. Its binary and ours then race for one address:
//! one of them cannot listen and exits, and a test that waits for `/readyz` is answered by the
//! other's binary, with a database that is not its own. So a test asks the binary for port 0, which the
//! operating system gives to one process only, and reads from the binary's log which address it bound
//! (`orchestrator listening`, the `addr` of the line), and what answers there is then its own binary.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::cell::RefCell;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

/// The address a test tells the binary to listen on when it does not have to name its own address
/// to the binary beforehand: the operating system picks a free port when the binary binds it.
pub const ANY_PORT: &str = "127.0.0.1:0";

/// The exit code of a binary that cannot bind its listen address (`EX_OSERR`).
pub const EX_OSERR: i32 = 71;

/// How long a binary has to connect, migrate and bind: the pool of the database waits 30 s for a
/// connection, and the machine may be loaded.
const BIND_WITHIN: Duration = Duration::from_secs(90);

/// A process under test, as far as waiting for its address goes.
pub trait Process {
    /// Everything it has written so far (standard output and standard error).
    fn log(&self) -> String;
    /// Its exit status, once it has exited.
    fn exited(&mut self) -> Option<ExitStatus>;
}

/// The address a line of the binary's log says it listens on: the `addr` of `orchestrator listening`,
/// in the JSON format (`{"fields":{"message":"orchestrator listening","addr":"127.0.0.1:41207",...}}`)
/// and in the text format (`orchestrator listening addr=127.0.0.1:41207 agents=1`). `None` for any
/// other line.
pub fn listening_addr(line: &str) -> Option<String> {
    if !line.contains("orchestrator listening") {
        return None;
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
        return json["fields"]["addr"].as_str().map(str::to_owned);
    }
    line.split_whitespace()
        .find_map(|word| word.strip_prefix("addr="))
        .map(str::to_owned)
}

/// The address of the broker's **run routes** (ADR 0084 decision 5), which the binary logs on its
/// internal listener as `the run routes listen addr=…`: a line of its own, so a test that waits for
/// the main listener never accepts it and a test that waits for it never accepts the main one.
pub fn run_routes_listening_addr(line: &str) -> Option<String> {
    if !line.contains("the run routes listen") {
        return None;
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
        return json["fields"]["addr"].as_str().map(str::to_owned);
    }
    line.split_whitespace()
        .find_map(|word| word.strip_prefix("addr="))
        .map(str::to_owned)
}

/// Waits until `run` logs the address it listens on. `Err` is its exit status, when it exits first
/// (a binary that cannot bind its address says nothing of the sort and exits `EX_OSERR`).
///
/// The borrow of `run` is never held across a wait.
pub async fn listening<P: Process>(run: &RefCell<P>) -> Result<String, ExitStatus> {
    listening_to(run, "where it listens", listening_addr).await
}

/// [`listening`] for a binary that was told [`ANY_PORT`]: no other process can hold the address it is
/// given, so exiting before it listens is a failure of the test.
pub async fn bound<P: Process>(run: &RefCell<P>) -> String {
    match listening(run).await {
        Ok(addr) => addr,
        Err(status) => panic!(
            "the binary exited ({status:?}) before it listened; log:\n{}",
            run.borrow().log()
        ),
    }
}

/// Waits until `run` logs the address of its **run routes** (ADR 0084 decision 5). `Err` is its
/// exit status, when it exits first: a `server.internal` the configuration refuses stops it with
/// exit 78 before either listener is bound.
pub async fn run_routes_listening<P: Process>(run: &RefCell<P>) -> Result<String, ExitStatus> {
    listening_to(
        run,
        "where its run routes listen",
        run_routes_listening_addr,
    )
    .await
}

/// [`run_routes_listening`] for a binary that was told [`ANY_PORT`], as [`bound`] is for
/// [`listening`].
pub async fn run_routes_bound<P: Process>(run: &RefCell<P>) -> String {
    match run_routes_listening(run).await {
        Ok(addr) => addr,
        Err(status) => panic!(
            "the binary exited ({status:?}) before its run routes listened; log:\n{}",
            run.borrow().log()
        ),
    }
}

/// Both waits: one deadline, the process's own exit ending it early, and no borrow of `run` held
/// across a sleep. `what` is the failure's words and `parse` the line that answers.
async fn listening_to<P>(
    run: &RefCell<P>,
    what: &str,
    parse: fn(&str) -> Option<String>,
) -> Result<String, ExitStatus>
where
    P: Process,
{
    let deadline = Instant::now() + BIND_WITHIN;
    loop {
        {
            let mut run = run.borrow_mut();
            if let Some(addr) = run.log().lines().find_map(parse) {
                return Ok(addr);
            }
            if let Some(status) = run.exited() {
                return Err(status);
            }
            assert!(
                Instant::now() < deadline,
                "the binary did not say {what} within {BIND_WITHIN:?}; {} bytes logged, exited={:?}, log:\n{}",
                run.log().len(),
                run.exited(),
                run.log()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
