//! Conformance testkit (ADR 0009): every [`ThreadStore`](crate::ThreadStore),
//! [`Wakeup`](crate::Wakeup), [`AgentClient`](crate::AgentClient), [`ChatModel`](crate::ChatModel),
//! [`AgentRegistry`](crate::AgentRegistry) and [`Authenticator`](crate::Authenticator)
//! implementation must pass it.
//!
//! ```ignore
//! async fn make() -> Option<MyStore> { /* None skips, e.g. when a database URL is unset */ }
//! orch_ports::thread_store_conformance!(make);
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

pub mod agent_client;
pub mod artifact_store;
pub mod authenticator;
pub mod bearer;
pub mod chat_model;
pub mod registry;
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
            concurrent_writers_keep_seq_contiguous list_events_after_limit latest_events_newest_first
            claim_once_and_concurrent_claimers lease_expiry_reclaim delegate_ordering_per_thread
            retry_not_claimable_before_due complete_outcomes mark_sent_is_atomic
            skip_unsent_delegates release_leases outbox_stats binding_applied_with_commit
            stale_attempt_is_fenced commit_after_another_owner_reclaims_is_fenced
            commit_after_complete_is_fenced expired_unclaimed_lease_still_commits
            job_roundtrip job_is_written_with_the_state gate_events_roundtrip ui_catalog_roundtrip
            ui_catalog_event_by_digest agent_step_roundtrip thread_titled_roundtrip
            title_rows_are_unordered_and_roundtrip
            inbox_dedupes_by_source_and_key inbox_claims_are_leases_and_lapse
            inbox_claimers_never_share_a_row inbox_parks_and_rearms_in_one_commit
            inbox_park_finds_a_watch_that_appeared inbox_commit_is_fenced_and_marks_applied
            inbox_stale_lease_writes_nothing parked_rows_expire timer_is_claimed_only_when_due
            a_replayed_commit_arms_no_second_timer inbox_retry_complete_and_release
            create_thread_arms_timers_and_watches watches_are_first_come
            inbox_counts_only_the_claims_that_failed
            inbox_only_commit_finishes_the_row_and_leaves_the_thread_alone
            verify_rows_are_unordered_and_keep_their_task_on_the_row
            a_commit_can_finish_the_claimed_row_with_what_it_writes
            fork_copies_the_parents_log_up_to_the_cut
            a_fork_commits_its_own_events_and_outbox_after_the_copy a_fork_at_zero_copies_nothing
            a_refused_fork_writes_nothing list_hides_edits_unless_asked fork_family_follows_edits
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
        async fn wakeup_delivers_inbox_topic() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::delivers_inbox_topic(w).await,
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
        #[tokio::test]
        async fn wakeup_delivers_live_text() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::delivers_live_text(w).await,
                None => eprintln!("skipped: no wakeup available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn wakeup_live_text_reaches_every_subscriber_in_order() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::live_reaches_every_subscriber(w).await,
                None => eprintln!("skipped: no wakeup available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn wakeup_live_text_does_not_disturb_topics() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::live_does_not_disturb_topics(w).await,
                None => eprintln!("skipped: no wakeup available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn wakeup_accepts_a_full_piece_of_live_text() {
            match $make().await {
                Some(w) => $crate::testkit::wakeup::live_accepts_a_full_piece(w).await,
                None => eprintln!("skipped: no wakeup available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
    };
}

/// Generates one `#[tokio::test]` per `AgentClient` conformance case. `$make` is an
/// `async fn() -> Option<F>` returning a fresh, isolated [`AgentFixture`](agent_client::AgentFixture)
/// (`None` skips the suite). Each case gives up after 10 s. The calling crate needs `tokio`
/// (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! agent_client_conformance {
    ($make:path) => {
        $crate::agent_client_conformance!(@cases $make;
            read_card_is_live_and_unreachable_is_transient first_envelope_names_the_task
            keys_are_unique_within_a_turn get_task_matches_the_live_stream
            follow_up_after_input_required_continues_the_task
            resubscribe_while_running_yields_the_rest_with_the_same_keys
            finished_or_unknown_task_is_not_found a_turn_outlives_its_stream
            cancel_running_then_cancel_finished_is_refused failed_task_carries_its_message
            find_task_by_message_never_names_a_wrong_task
            unreachable_send_has_a_clean_public_detail
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::agent_client::$case(fixture).await,
                    None => eprintln!("skipped: no agent available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `ChatModel` conformance case. `$make` is an
/// `async fn() -> Option<F>` returning a fresh, isolated [`ModelFixture`](chat_model::ModelFixture)
/// (`None` skips the suite). Each case gives up after 10 s. The calling crate needs `tokio`
/// (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! chat_model_conformance {
    ($make:path) => {
        $crate::chat_model_conformance!(@cases $make;
            the_answer_is_the_models_text an_endpoint_that_fails_is_transient
            nonsense_is_not_an_answer a_rate_limit_is_rate_limited a_refusal_is_permanent
            a_refused_credential_is_unauthenticated the_credential_is_never_in_an_error
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::chat_model::$case(fixture).await,
                    None => eprintln!("skipped: no model available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `ArtifactStore` conformance case. `$make` is an
/// `async fn() -> Option<S>` returning a fresh store (`None` skips the suite, for a bucket nobody
/// gave the test). Each case gives up after 60 s and uses thread ids of its own. The calling crate
/// needs `tokio` (with `macros` and `rt-multi-thread`) as a dev-dependency.
#[macro_export]
macro_rules! artifact_store_conformance {
    ($make:path) => {
        $crate::artifact_store_conformance!(@cases $make;
            put_then_get_round_trips an_empty_file_round_trips the_meta_is_kept_whole
            putting_twice_is_the_same_as_once the_same_content_under_another_name_is_one_file
            a_missing_key_is_none a_large_file_streams
            concurrent_puts_of_one_key_leave_one_whole_file
            concurrent_puts_of_different_files_keep_each
            delete_removes_the_file_and_only_that_file threads_do_not_share_a_file
            bytes_that_are_not_the_key_are_refused
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::artifact_store::$case(store).await,
                    None => eprintln!("skipped: no artifact store available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `AgentRegistry` conformance case. `$make` is an
/// `async fn() -> Option<F>` returning a fresh, isolated
/// [`RegistryFixture`](registry::RegistryFixture) (`None` skips the suite). Each case gives up
/// after 10 s. The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! agent_registry_conformance {
    ($make:path) => {
        $crate::agent_registry_conformance!(@cases $make;
            lists_in_order get_finds_by_id unknown_id_is_none a_change_shows_on_the_next_read
            down_lists_nothing_and_says_so get_while_down_is_unavailable_not_none
            recovers_when_up invalid_entries_never_listed an_id_is_listed_once
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::registry::$case(fixture).await,
                    None => eprintln!("skipped: no registry available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `Authenticator` conformance case. `$make` is an
/// `async fn() -> Option<F>` returning a fresh, isolated
/// [`AuthFixture`](authenticator::AuthFixture) (`None` skips the suite). Each case gives up after
/// 10 s. The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! authenticator_conformance {
    ($make:path) => {
        $crate::authenticator_conformance!(@cases $make;
            valid_credentials_name_the_user no_credentials_is_missing
            refused_credentials_are_invalid the_credential_is_never_in_an_error
            unavailable_is_closed_and_distinguishable ready_when_it_can_authenticate
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::authenticator::$case(fixture).await,
                    None => eprintln!("skipped: no authenticator available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per conformance case of an `Authenticator` that reads signed
/// bearer tokens (the policy of ADR 0033). `$make` is an `async fn() -> Option<F>` returning a
/// fresh, isolated [`TokenFixture`](bearer::TokenFixture) whose authenticator has never fetched
/// the issuer's keys (`None` skips the suite). Each case gives up after 10 s. The calling crate
/// needs `tokio` (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! bearer_token_conformance {
    ($make:path) => {
        $crate::bearer_token_conformance!(@cases $make;
            a_valid_token_names_the_user an_expired_token_is_refused
            a_token_not_yet_valid_is_refused the_issuer_must_match_exactly
            the_audience_must_be_the_configured_one exp_and_iat_are_required
            alg_none_is_refused hs256_signed_with_a_public_key_is_refused
            a_token_from_an_unpublished_key_is_refused
            an_unknown_kid_refetches_at_most_once_per_interval
            a_rotated_key_is_found_once_published
            the_issuer_down_is_unavailable_and_closed
            a_missing_or_malformed_bearer_is_unauthenticated
            email_verified_false_is_refused the_user_claim_is_the_configured_one
            roles_come_from_the_roles_claim the_token_is_never_in_an_error
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::bearer::$case(fixture).await,
                    None => eprintln!("skipped: no token issuer available"),
                }
            }
        )*
    };
}
