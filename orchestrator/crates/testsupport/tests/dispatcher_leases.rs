//! The two dispatcher configurations the tests share differ in the lease, and only the lease.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use orch_testsupport::{crashing_dispatcher, fast_dispatcher};

/// A lease that lapses under a worker that is alive hands its row to a new claim, which reads the
/// task back from a snapshot: a status that only the first stream carried (the `working` before an
/// artifact) is never logged, and a test that pins the shape of a log fails once in many runs. A
/// stall of a database or a machine of a second must not do that, so the shared lease is far longer.
#[test]
fn the_shared_lease_outlasts_any_stall_of_a_machine() {
    let shared = fast_dispatcher();
    assert!(
        shared.lease >= Duration::from_secs(30),
        "a lease of {:?} lapses under a worker that is alive when the database stalls",
        shared.lease
    );
    assert!(
        shared.heartbeat < Duration::from_secs(1),
        "the heartbeat is still exercised by every test"
    );
}

/// The test that kills an instance waits for its lease to lapse: it must lapse within a second or
/// so, with room for a few heartbeats, and nothing else about the dispatcher changes.
#[test]
fn the_crashing_lease_lapses_fast_and_changes_nothing_else() {
    let shared = fast_dispatcher();
    let crashing = crashing_dispatcher();
    assert!(
        crashing.lease <= Duration::from_secs(1),
        "{:?}",
        crashing.lease
    );
    assert!(
        crashing.lease >= crashing.heartbeat * 4,
        "a few heartbeats fit in the lease"
    );
    assert_eq!(crashing.heartbeat, shared.heartbeat);
    assert_eq!(crashing.poll_interval, shared.poll_interval);
    assert_eq!(crashing.concurrency, shared.concurrency);
    assert_eq!(crashing.max_attempts, shared.max_attempts);
    assert_eq!(crashing.backoff_base, shared.backoff_base);
}
