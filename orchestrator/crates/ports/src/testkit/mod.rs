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
pub mod tool_server;
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
            the_binding_adopts_the_agents_context_once
            skip_unsent_delegates a_commit_can_skip_the_unsent_delegates_it_supersedes
            steer_rows_claim_beside_an_inflight_delegate
            a_requeued_steer_waits_behind_the_delegation_in_flight
            unsent_steers_are_skipped_with_the_unsent_delegates
            release_leases outbox_stats binding_applied_with_commit
            stale_attempt_is_fenced commit_after_another_owner_reclaims_is_fenced
            commit_after_complete_is_fenced expired_unclaimed_lease_still_commits
            job_roundtrip job_tools_roundtrip job_is_written_with_the_state gate_events_roundtrip ui_catalog_roundtrip
            delegate_rows_keep_their_mentions steer_rows_keep_their_mentions
            ui_catalog_event_by_digest agent_step_roundtrip thread_titled_roundtrip
            title_rows_are_unordered_and_roundtrip thread_described_roundtrip
            a_fork_starts_with_the_description_it_is_given
            inbox_dedupes_by_source_and_key inbox_claims_are_leases_and_lapse
            inbox_claimers_never_share_a_row inbox_parks_and_rearms_in_one_commit
            inbox_park_finds_a_watch_that_appeared inbox_commit_is_fenced_and_marks_applied
            inbox_stale_lease_writes_nothing parked_rows_expire timer_is_claimed_only_when_due
            a_replayed_commit_arms_no_second_timer inbox_retry_complete_and_release
            create_thread_arms_timers_and_watches watches_are_first_come
            inbox_counts_only_the_claims_that_failed
            inbox_only_commit_finishes_the_row_and_leaves_the_thread_alone
            verify_rows_are_unordered_and_keep_their_task_on_the_row
            ask_rows_are_unordered_and_keep_their_task_on_the_row ask_events_roundtrip
            a_commit_can_finish_the_claimed_row_with_what_it_writes
            fork_copies_the_parents_log_up_to_the_cut
            a_fork_commits_its_own_events_and_outbox_after_the_copy a_fork_at_zero_copies_nothing
            a_refused_fork_writes_nothing list_hides_edits_unless_asked fork_family_follows_edits
            a_shared_thread_is_found_by_its_nonce a_revoked_share_is_not_found
            a_reshared_thread_is_found_by_its_new_nonce_only a_fork_of_a_shared_thread_is_private
            a_new_thread_is_private a_nonce_belongs_to_one_thread a_refused_commit_writes_no_share
            a_new_thread_is_on_top_of_the_rail a_thread_is_placed_on_top_before_or_after_another
            a_block_moves_with_its_parent pin_and_unpin_go_to_the_top_of_their_sections
            archived_threads_are_listed_only_when_asked unarchiving_keeps_the_place
            eject_lands_after_the_former_block a_bad_anchor_or_a_nested_row_is_refused
            arranging_is_the_owners_alone rail_pages_never_split_a_block
            an_arrangement_that_changes_nothing_writes_nothing
            a_rank_that_would_pass_the_cap_re_spreads_the_list
            ties_of_rank_are_broken_by_newest_first
            a_thread_is_nested_under_a_top_level_thread_of_its_owner
            a_fork_can_be_made_nested_under_its_parent
            delete_removes_the_thread_and_everything_that_hangs_on_it
            delete_removes_the_timers_of_the_thread_whatever_their_status
            delete_takes_the_edits_with_the_thread_and_keeps_the_forks
            delete_that_misses_an_edit_is_a_conflict_and_deletes_nothing
            nested_children_take_the_place_of_a_deleted_parent
            nested_children_keep_the_section_of_a_deleted_parent
            delete_is_the_owners_alone_and_a_second_delete_is_not_found
            a_version_conflict_deletes_nothing
            purges_are_claimed_under_a_lease_and_finished
            purge_claimers_never_share_a_row
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
            a_message_with_no_context_starts_one
            keys_are_unique_within_a_turn get_task_matches_the_live_stream
            follow_up_after_input_required_continues_the_task
            resubscribe_while_running_yields_the_rest_with_the_same_keys
            finished_or_unknown_task_is_not_found a_turn_outlives_its_stream
            cancel_running_then_cancel_finished_is_refused failed_task_carries_its_message
            find_task_by_message_never_names_a_wrong_task
            unreachable_send_has_a_clean_public_detail
            a_steer_is_refused_by_an_agent_that_does_not_list_the_extension
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
            an_unknown_endpoint_is_not_configured
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
            a_copy_is_a_file_of_its_own copying_twice_is_the_same_as_once
            concurrent_copies_of_one_file_leave_each_whole
            copying_a_missing_file_is_not_found a_copy_to_another_hash_is_refused
            delete_prefix_removes_every_file_of_the_thread_and_no_other
            delete_prefix_twice_is_the_same_as_once
            delete_prefix_removes_a_thread_of_many_files
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

/// Generates one `#[tokio::test]` per `ToolServerClient` conformance case. `$make` is an
/// `async fn() -> Option<F>` returning a fresh, isolated
/// [`ToolServerFixture`](tool_server::ToolServerFixture) (`None` skips the suite). Each case gives
/// up after 20 s. The calling crate needs `tokio` (with `macros`, `rt` and `time`) as a
/// dev-dependency.
#[macro_export]
macro_rules! tool_server_conformance {
    ($make:path) => {
        $crate::tool_server_conformance!(@cases $make;
            lists_the_tools_with_their_schemas the_credentials_reach_the_server
            an_echo_round_trip is_error_is_passed_through a_wrong_bearer_is_unauthenticated
            an_unreachable_server_is_unreachable a_slow_call_times_out_within_the_limit
            a_dropped_call_leaves_the_client_usable an_unknown_tool_is_a_remote_error
            a_result_over_the_bound_is_cut no_error_or_debug_text_shows_a_secret
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::tool_server::$case(fixture).await,
                    None => eprintln!("skipped: no tool server available"),
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
