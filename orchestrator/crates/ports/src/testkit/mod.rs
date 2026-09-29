//! Conformance testkit (ADR 0009): every [`ThreadStore`](crate::ThreadStore) and
//! [`Wakeup`](crate::Wakeup) implementation must pass it.
//!
//! ```ignore
//! async fn make() -> Option<MyStore> { /* None skips, e.g. when a database URL is unset */ }
//! orch_ports::thread_store_conformance!(make);
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

pub mod thread_store;
pub mod wakeup;

/// Generates one `#[tokio::test]` per `ThreadStore` conformance case. `$make` is an
/// `async fn() -> Option<S>` returning a fresh, isolated store (`None` skips the suite).
/// The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! thread_store_conformance {
    ($make:path) => {
        $crate::thread_store_conformance!(@cases $make;
            ping create_get_roundtrip event_data_roundtrip owner_isolation list_newest_first_before_limit
            commit_contiguous_seq version_conflict_writes_nothing duplicate_key_writes_nothing
            concurrent_writers_keep_seq_contiguous list_events_after_limit
            claim_once_and_concurrent_claimers lease_expiry_reclaim delegate_ordering_per_thread
            retry_not_claimable_before_due complete_outcomes mark_sent_is_atomic
            skip_unsent_delegates release_leases binding_applied_with_commit
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::thread_store::$case(store).await,
                    None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
                }
            }
        )*
    };
}

/// Generates the `Wakeup` conformance tests. `$make` is an `async fn() -> Option<W>`.
#[macro_export]
macro_rules! wakeup_conformance {
    ($make:path) => {
        #[tokio::test]
        async fn wakeup_delivers_thread_topic() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::delivers_thread_topic(w).await,
                None => eprintln!("skipped: no wakeup available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn wakeup_delivers_outbox_topic_to_every_subscriber() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::delivers_outbox_to_every_subscriber(w).await,
                None => eprintln!("skipped: no wakeup available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
    };
}
