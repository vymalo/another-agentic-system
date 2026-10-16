//! What the lock file must not go back to.
//!
//! The adapter's `rpc_http` is one `reqwest` client, and so one `hyper-util` connection pool, for every call to
//! an agent: `SendStreamingMessage` and `SubscribeToTask` hold an SSE response open for as long as the task
//! runs, and `GetTask`, `CancelTask` and `ListTasks` are short calls on the same pool. **hyper 1.11.1 has a
//! race** (`client::dispatch`, fixed in 1.12.0 with `want` 0.3.2): the sender takes the receiver's "I want a
//! request" signal and then queues its request, and a receiver that polls the still empty queue in between
//! signals "I want" again. The signal stays after the request is taken, so the connection reports itself ready
//! while it is serving that request. `hyper-util`'s legacy pool then takes back an HTTP/1.1 connection whose
//! response is still streaming, and the next call that checks it out is queued behind the stream, with no error,
//! for as long as the stream lives.
//!
//! *Reproduced* 2026-10-16 in another-agentic-platform (PR #28; its `crates/controller/tests/dependencies.rs`
//! is this file's model) against a bare kube-apiserver v1.35.8, whose watches are such streams: 5 stalls in 4000
//! services with hyper 1.11.1, none in 8000 with hyper 1.12.0 and `want` 0.3.2. *Unverified here*: no stall has
//! been seen in the orchestrator, and what it would look like is read from the code, not observed. The
//! connections at risk are HTTP/1.1 ones to one agent (a plain `http://` agent, as in `compose.yaml` and in a
//! cluster, or a TLS one that does not negotiate h2). The symptom would be a call that gets no answer while a
//! task of that agent streams: a `CancelTask` or a `GetTask`, or the opening of the next stream, ending only in
//! the `call_timeout` (30 s) that bounds them, and the stream itself going on. The card client (`card_http`, a
//! pool of its own and whole short responses), the model client (`stream: false`) and the MCP and
//! credential-broker clients (`pool_max_idle_per_host(0)`: no connection is kept for the next request) are not
//! exposed. This test is the lock file's half of the fix, in the crate whose client keeps streams open on a pool.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::Path;

/// The versions of `name` in the workspace's `Cargo.lock`, as numbers (a pre-release or build suffix dropped).
fn locked(name: &str) -> Vec<Vec<u64>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut found = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == format!("name = \"{name}\"") {
            let version = lines
                .next()
                .unwrap()
                .trim_start_matches("version = \"")
                .trim_end_matches('"');
            let numbers = version.split(['-', '+']).next().unwrap();
            found.push(numbers.split('.').map(|n| n.parse().unwrap()).collect());
        }
    }
    assert!(!found.is_empty(), "{name} is not in {}", path.display());
    found
}

/// Whether `version` is of the line that `floor` starts (the same major, or the same minor for a `0.x`): a lock may
/// hold an older line of the same crate for another dependent, which this fix does not reach.
fn same_line(version: &[u64], floor: &[u64; 3]) -> bool {
    version[0] == floor[0] && (floor[0] != 0 || version[1] == floor[1])
}

#[test]
fn hyper_is_a_version_whose_client_does_not_report_a_busy_connection_as_ready() {
    for (name, floor) in [("hyper", [1, 12, 0]), ("want", [0, 3, 2])] {
        let line: Vec<Vec<u64>> = locked(name)
            .into_iter()
            .filter(|version| same_line(version, &floor))
            .collect();
        assert!(
            !line.is_empty(),
            "{name} is locked, but in no version of the line {floor:?} belongs to: expected one, see the doc of this file"
        );
        for found in line {
            assert!(
                found.as_slice() >= floor.as_slice(),
                "{name} {found:?} is locked, and {floor:?} is the first release that withdraws a stale `want`: with \
                 anything older the connection of a streaming call can go back to the pool while the stream is still \
                 open, and the next call to that agent can wait behind it (see the doc of this file). \
                 `cargo update -p {name} --precise {}`",
                floor.map(|n| n.to_string()).join(".")
            );
        }
    }
}
