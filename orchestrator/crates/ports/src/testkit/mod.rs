//! Conformance testkit (ADR 0009): every [`ThreadStore`](crate::ThreadStore),
//! [`Wakeup`](crate::Wakeup), [`AgentClient`](crate::AgentClient), [`ChatModel`](crate::ChatModel),
//! [`AgentRegistry`](crate::AgentRegistry), [`Authenticator`](crate::Authenticator),
//! [`SecretStore`](crate::SecretStore), [`VaultStore`](crate::VaultStore), [`UsageLedger`](crate::UsageLedger),
//! [`ThreadSearch`](crate::ThreadSearch),
//! [`UserToolServerStore`](crate::UserToolServerStore),
//! [`PendingAuthorizations`](crate::PendingAuthorizations),
//! [`TeamPresetStore`](crate::TeamPresetStore), [`PromptSigner`](crate::PromptSigner) and
//! [`CredentialBroker`](crate::CredentialBroker) implementation must pass it.
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
pub mod credential_broker;
pub mod pending_authorizations;
pub mod prompt_signer;
pub mod prompt_store;
pub mod registry;
pub mod run_leases;
pub mod secret_store;
pub mod teams;
pub mod thread_search;
pub mod thread_store;
pub mod tool_server;
pub mod usage_ledger;
pub mod user_tool_server_store;
pub mod vault_store;
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
            a_fair_claim_serves_the_person_with_fewer_jobs_running_first
            a_fair_claim_gives_one_person_every_slot a_fair_claim_interleaves_the_people
            a_fair_claim_counts_only_live_leases a_fair_claim_counts_running_rows_of_every_kind
            a_fair_claim_serves_cancel_rows_first
            a_cancel_row_takes_no_place_in_the_ranks_of_its_persons_rows
            a_fair_claim_keeps_the_per_thread_order
            fair_claimers_at_once_take_disjoint_rows_of_both_people
            a_fair_claim_is_not_blinded_by_a_long_queue
            a_fair_claim_takes_the_older_rows_among_equal_loads
            a_fair_claim_ranks_a_persons_rows_by_age_and_not_by_id
            a_fifo_claim_takes_the_oldest_rows_whoever_they_are_for
            retry_not_claimable_before_due complete_outcomes mark_sent_is_atomic
            the_binding_adopts_the_agents_context_once
            skip_unsent_delegates a_commit_can_skip_the_unsent_delegates_it_supersedes
            steer_rows_claim_beside_an_inflight_delegate
            a_requeued_steer_waits_behind_the_delegation_in_flight
            session_rows_ride_the_steer_lane_and_are_never_delegations
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
            a_threads_default_pick_is_its_owners_and_goes_with_the_thread
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
            a_child_thread_is_made_with_its_first_commit_and_says_whose_child_it_is
            a_session_has_one_child_and_a_second_open_writes_nothing
            a_child_is_one_per_parent_and_session_not_per_session_alone
            concurrent_opens_of_one_session_make_one_thread
            a_child_is_refused_a_foreign_parent_a_child_parent_and_no_parent
            only_open_child_thread_makes_a_child_and_a_fork_of_one_is_none
            child_threads_are_the_parents_children_oldest_first
            child_threads_lists_at_most_a_thousand
            children_are_nested_under_their_parent_in_the_rail
            deleting_a_parent_needs_its_children_named_and_takes_them
            a_child_is_deleted_alone_and_its_parent_stays
            a_delete_and_an_open_that_race_leave_no_orphan
            the_session_events_ledgers_and_observe_rows_round_trip
            a_commit_with_no_events_creates_a_thread
            the_skip_of_unsent_delegates_leaves_observe_rows
            the_skip_of_unsent_delegates_leaves_session_start_and_close_rows a_session_id_is_bounded
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
            a_tier_is_refused_by_an_agent_that_does_not_list_the_extension
            a_plain_message_under_a_cap_is_not_sent_to_an_agent_that_lists_tiers
            a_locale_never_holds_a_message_back
            a_pick_is_refused_by_an_agent_that_does_not_offer_it
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
            read_prefix_is_the_start_of_the_file read_prefix_of_a_missing_file_is_none
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

/// Generates one `#[tokio::test]` per conformance case of a store that **can presign** (ADR 0058):
/// what a presigned `PUT` allows ends up in the store, `head` says what is there, bytes that are
/// not the declared ones are never vouched for, and a presigned `GET` opens its key and makes the
/// answer carry the signed overrides. `$make` is an `async fn() -> Option<PresignFixture<S>>`
/// (`None` skips the suite): the store and the means to send it a request the way an agent does (a
/// `PUT`) and the way a browser does (a `GET`).
/// Each case gives up after 60 s. The calling crate needs `tokio` (with `macros` and
/// `rt-multi-thread`) as a dev-dependency.
#[macro_export]
macro_rules! artifact_presign_conformance {
    ($make:path) => {
        $crate::artifact_presign_conformance!(@cases $make;
            a_store_that_presigns_says_so a_presigned_put_stores_the_declared_file
            head_of_a_missing_file_is_none
            other_bytes_than_declared_are_refused_or_never_vouched_for
            a_slot_is_for_a_file_that_agrees_with_its_key
            a_presigned_get_serves_the_file_with_the_signed_overrides
            a_presigned_get_changes_only_what_it_was_asked_to
            a_presigned_get_opens_that_key_and_no_other
            a_presigned_get_is_for_a_request_that_is_right
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::artifact_store::$case(fixture).await,
                    None => eprintln!("skipped: no presigning artifact store available"),
                }
            }
        )*
    };
}

/// Generates the conformance case of a store that **cannot presign**: it says so and works as
/// before. `$make` is an `async fn() -> Option<S>` (`None` skips the suite).
#[macro_export]
macro_rules! artifact_no_presign_conformance {
    ($make:path) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_store_that_cannot_presign_says_so() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::artifact_store::a_store_that_cannot_presign_says_so(store)
                        .await
                }
                None => eprintln!("skipped: no artifact store available"),
            }
        }
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
            audience_round_trips
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

/// Generates one `#[tokio::test]` per `PromptSigner` conformance case (ADR 0087, the note of 2026-11-25).
/// `$make` is an `async fn() -> Option<S>` returning a fresh signer (`None` skips the suite), and `$unavailable` an
/// `async fn(&S)` after which that signer has no certificate it may sign with. Each case gives up after 20 s. The
/// calling crate needs `tokio` (with `macros`, `rt` and `time`) as a dev-dependency.
#[macro_export]
macro_rules! prompt_signer_conformance {
    ($make:path, unavailable = $unavailable:path) => {
        $crate::prompt_signer_conformance!(@cases $make;
            the_payload_is_attached_byte_for_byte
            the_protected_header_is_the_statements
            the_signature_verifies_under_the_leaf_key
            each_payload_gets_its_own_statement
            a_signer_that_signs_is_not_unavailable
        );
        #[tokio::test]
        async fn an_unavailable_signer_refuses_and_says_so() {
            match $make().await {
                Some(signer) => {
                    $crate::testkit::prompt_signer::an_unavailable_signer_refuses_and_says_so(
                        signer,
                        $unavailable,
                    )
                    .await
                }
                None => eprintln!("skipped: no prompt signer available"),
            }
        }
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(signer) => $crate::testkit::prompt_signer::$case(signer).await,
                    None => eprintln!("skipped: no prompt signer available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `SecretStore` conformance case (ADR 0046, the amendment of
/// 2026-10-11). `$make` is an `async fn() -> Option<S>` returning a fresh store (`None` skips the
/// suite). Each case uses owners of its own and gives up after 60 s. The calling crate needs `tokio`
/// (with `macros` and `rt-multi-thread`) as a dev-dependency.
///
/// One case needs a store that can be **broken**: `secret_store_conformance!(make, breaker = path)`
/// where `path` is an `async fn(&S)` that makes every later call of that store fail as an unreachable
/// store does (a Postgres adapter closes its pool; `MemorySecretStore` is taken down with
/// `set_down(true)`). Without a `breaker` the case `no_error_from_a_broken_store_shows_a_secret` is
/// skipped, and says so: it is the one that looks for the material in the errors that can hold text,
/// so an adapter that can be broken should pass it.
#[macro_export]
macro_rules! secret_store_conformance {
    ($make:path) => {
        $crate::secret_store_conformance!(@common $make);
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn no_error_from_a_broken_store_shows_a_secret() {
            eprintln!(
                "skipped: this adapter supplies no breaker (`secret_store_conformance!(make, \
                 breaker = path)`), so its errors are not looked into for a secret"
            );
        }
    };
    ($make:path, breaker = $breaker:path) => {
        $crate::secret_store_conformance!(@common $make);
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn no_error_from_a_broken_store_shows_a_secret() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::secret_store::no_error_from_a_broken_store_shows_a_secret(
                        store, $breaker,
                    )
                    .await
                }
                None => eprintln!("skipped: no secret store available"),
            }
        }
    };
    (@common $make:path) => {
        $crate::secret_store_conformance!(@cases $make;
            put_then_get_round_trips a_missing_secret_is_none
            material_reads_back_byte_for_byte put_replaces_and_gives_a_newer_version
            putting_identical_material_again_gives_a_new_version
            delete_removes_the_secret_and_is_idempotent
            rotate_with_the_stored_version_replaces
            rotate_with_a_stale_version_is_a_conflict_and_changes_nothing
            rotate_of_a_missing_secret_is_a_conflict
            concurrent_rotates_from_one_version_have_one_winner
            concurrent_puts_of_one_key_leave_one_whole_secret
            a_version_is_never_reused_after_a_delete
            owners_never_see_each_others_material
            a_server_and_its_pending_flow_are_separate_secrets
            no_error_or_debug_text_shows_a_secret
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::secret_store::$case(store).await,
                    None => eprintln!("skipped: no secret store available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `UsageLedger` conformance case (ADR 0065). `$make` is an
/// `async fn() -> Option<S>` returning a fresh, isolated store that is both a
/// [`ThreadStore`](crate::ThreadStore) and a [`UsageLedger`](crate::UsageLedger) (`None` skips the
/// suite): the ledger is written by the commits of the store it is read from. The calling crate
/// needs `tokio` (with `macros` and `rt`) as a dev-dependency.
///
/// Four cases play a log older than the ledger (the usage events of a build without it, or from
/// before the migration), so they need a store that can **forget its ledger**:
/// `usage_ledger_conformance!(make, forget = path)` where `path` is an `async fn(&S)` that empties
/// the ledger and marks no thread as gone through (a Postgres adapter deletes the rows of the ledger
/// and zeroes `threads.usage_through`; `MemoryStore` has `forget_usage_ledger`). Without a `forget`
/// those cases are skipped, and say so.
#[macro_export]
macro_rules! usage_ledger_conformance {
    ($make:path) => {
        $crate::usage_ledger_conformance!(@common $make);
        #[tokio::test]
        async fn a_commit_catches_up_with_the_log_the_ledger_has_not_been_through() {
            eprintln!("skipped: this adapter supplies no `forget` hook");
        }
        #[tokio::test]
        async fn backfill_folds_in_what_the_ledger_has_not_been_through_and_again_changes_nothing() {
            eprintln!("skipped: this adapter supplies no `forget` hook");
        }
        #[tokio::test]
        async fn a_commit_to_a_fork_an_older_build_wrote_to_catches_up_from_its_cut() {
            eprintln!("skipped: this adapter supplies no `forget` hook");
        }
        #[tokio::test]
        async fn a_catch_up_of_many_events_gives_what_folding_them_one_by_one_gives() {
            eprintln!("skipped: this adapter supplies no `forget` hook");
        }
    };
    ($make:path, forget = $forget:path) => {
        $crate::usage_ledger_conformance!(@common $make);
        #[tokio::test]
        async fn a_commit_catches_up_with_the_log_the_ledger_has_not_been_through() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::usage_ledger::a_commit_catches_up_with_the_log_the_ledger_has_not_been_through(
                        store, $forget,
                    )
                    .await
                }
                None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn backfill_folds_in_what_the_ledger_has_not_been_through_and_again_changes_nothing() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::usage_ledger::backfill_folds_in_what_the_ledger_has_not_been_through_and_again_changes_nothing(
                        store, $forget,
                    )
                    .await
                }
                None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn a_commit_to_a_fork_an_older_build_wrote_to_catches_up_from_its_cut() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::usage_ledger::a_commit_to_a_fork_an_older_build_wrote_to_catches_up_from_its_cut(
                        store, $forget,
                    )
                    .await
                }
                None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
        #[tokio::test]
        async fn a_catch_up_of_many_events_gives_what_folding_them_one_by_one_gives() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::usage_ledger::a_catch_up_of_many_events_gives_what_folding_them_one_by_one_gives(
                        store, $forget,
                    )
                    .await
                }
                None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
    };
    (@common $make:path) => {
        $crate::usage_ledger_conformance!(@cases $make;
            calls_add_up_and_a_total_replaces_them
            a_thread_created_with_usage_events_is_counted
            agents_and_tasks_are_counted_apart
            a_month_sums_the_threads_and_models_of_a_person
            the_month_is_the_utc_month_of_the_latest_event_that_changed_the_row
            people_are_kept_apart a_refused_commit_counts_nothing
            a_total_carries_its_parts_and_its_sandbox
            erasing_a_thread_keeps_its_usage_and_forgets_the_thread
            usage_after_an_erase_is_counted_on_top_of_what_was_kept
            a_fork_does_not_count_the_events_it_copied
            backfill_leaves_alone_what_the_ledger_is_through
            events_that_are_not_usage_change_nothing
            a_sweep_removes_only_the_months_before_the_limit
            a_sweep_takes_at_most_its_limit_oldest_first
            sweeping_again_changes_nothing
            a_sweep_removes_the_old_rows_of_erased_threads_too
            a_task_that_continues_after_its_row_was_swept_starts_again
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::usage_ledger::$case(store).await,
                    None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `ThreadSearch` conformance case (ADR 0068). `$make` is an
/// `async fn() -> Option<S>` returning a fresh, isolated store that is both a
/// [`ThreadStore`](crate::ThreadStore) and a [`ThreadSearch`](crate::ThreadSearch) (`None` skips the
/// suite): the index is written by the commits of the store it is read from. The calling crate needs
/// `tokio` (with `macros` and `rt`) as a dev-dependency.
///
/// Some cases play a log older than the index, so they need a store that can **forget its index**:
/// `thread_search_conformance!(make, forget = path)` where `path` is an `async fn(&S)` that deletes
/// the index and marks no thread as gone through (a Postgres adapter deletes `message_terms` and
/// clears the columns of `threads`; `MemoryStore` has `forget_search_index`). One more plays an index
/// made under an **older fold version**: `thread_search_conformance!(make, forget = path, age = path)`
/// where the second `async fn(&S)` marks every thread as indexed under a lower
/// [`FOLD_VERSION`](orch_core::search::FOLD_VERSION) (a Postgres adapter lowers `search_fold` and may
/// write keys no query can match; `MemoryStore` has `age_search_index`). Without the hooks those cases
/// are skipped, and say so.
#[macro_export]
macro_rules! thread_search_conformance {
    ($make:path) => {
        $crate::thread_search_conformance!(@common $make);
        $crate::thread_search_conformance!(@skipped
            a_commit_catches_up_with_the_log_the_index_has_not_been_through
            backfill_indexes_old_events_and_is_idempotent
            a_commit_catches_an_old_log_up_in_bounded_steps
            a_catch_up_does_not_index_a_message_a_later_one_replaced
            a_raised_fold_version_makes_a_thread_unindexed_until_it_is_indexed_again
        );
    };
    ($make:path, forget = $forget:path) => {
        $crate::thread_search_conformance!(@common $make);
        $crate::thread_search_conformance!(@forgetting $make, $forget);
        $crate::thread_search_conformance!(@skipped
            a_raised_fold_version_makes_a_thread_unindexed_until_it_is_indexed_again
        );
    };
    ($make:path, forget = $forget:path, age = $age:path) => {
        $crate::thread_search_conformance!(@common $make);
        $crate::thread_search_conformance!(@forgetting $make, $forget);
        #[tokio::test]
        async fn a_raised_fold_version_makes_a_thread_unindexed_until_it_is_indexed_again() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::thread_search::a_raised_fold_version_makes_a_thread_unindexed_until_it_is_indexed_again(
                        store, $age,
                    )
                    .await
                }
                None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
            }
        }
    };
    (@skipped $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                eprintln!("skipped: this adapter supplies no hook for it");
            }
        )*
    };
    (@forgetting $make:path, $forget:path) => {
        $crate::thread_search_conformance!(@with_forget $make, $forget;
            a_commit_catches_up_with_the_log_the_index_has_not_been_through
            backfill_indexes_old_events_and_is_idempotent
            a_commit_catches_an_old_log_up_in_bounded_steps
            a_catch_up_does_not_index_a_message_a_later_one_replaced
        );
    };
    (@with_forget $make:path, $forget:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::thread_search::$case(store, $forget).await,
                    None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
                }
            }
        )*
    };
    (@common $make:path) => {
        $crate::thread_search_conformance!(@cases $make;
            search_finds_title_description_and_message
            search_is_the_owners
            deleted_thread_leaves_no_hit
            fork_copied_events_do_not_hit_until_parent_deleted
            a_fork_has_the_message_the_parent_later_replaced
            edits_only_with_branches
            archived_filters
            replaced_final_message_hits_once
            partial_and_working_messages_never_hit
            the_newest_three_messages_are_shown_and_all_are_counted
            a_rename_is_searched_from_its_commit
            keys_roundtrip_cyrillic_cjk_digits
            paging_is_stable
            a_limit_of_zero_is_one_and_no_term_matches_nothing
            a_page_that_holds_every_result_has_no_more
            a_persons_second_message_does_not_erase_the_first
            a_title_that_matches_only_some_terms_is_not_a_title_hit
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::thread_search::$case(store).await,
                    None => eprintln!("skipped: no store available (ORCH_TEST_DATABASE_URL unset)"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `UserToolServerStore` conformance case (ADR 0046). `$make` is
/// an `async fn() -> Option<S>` returning a fresh store (`None` skips the suite). Each case uses
/// owners of its own and gives up after 60 s. The calling crate needs `tokio` (with `macros` and
/// `rt-multi-thread`) as a dev-dependency.
#[macro_export]
macro_rules! user_tool_server_store_conformance {
    ($make:path) => {
        $crate::user_tool_server_store_conformance!(@cases $make;
            insert_then_get_round_trips a_missing_server_is_none
            a_definition_holds_no_secret_and_starts_added
            list_returns_the_owners_servers_ordered_by_id another_owner_sees_nothing
            the_same_id_can_belong_to_two_owners
            insert_of_an_id_that_exists_is_refused_and_changes_nothing
            replace_with_the_current_revision_replaces_whole
            a_stale_replace_is_a_conflict_and_changes_nothing
            a_stale_set_state_is_a_conflict_and_changes_nothing
            a_write_after_a_delete_is_not_found_and_brings_nothing_back
            revisions_are_new_with_every_write_and_never_given_twice
            at_most_eight_servers_per_owner concurrent_inserts_never_pass_the_limit
            concurrent_inserts_of_one_id_have_one_winner
            concurrent_replaces_from_one_revision_have_one_winner
            a_replace_racing_a_delete_never_brings_the_server_back
            set_state_changes_the_state_only delete_removes_the_server_and_is_idempotent
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::user_tool_server_store::$case(store).await,
                    None => eprintln!("skipped: no user tool server store available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `PromptStore` conformance case (ADR 0070): what the API relies on from any store of
/// people's prompt libraries. `$make` is an `async fn() -> Option<S>` returning a fresh store (`None` skips the suite).
/// Each case uses owners of its own and gives up after 60 s. The calling crate needs `tokio` (with `macros` and
/// `rt-multi-thread`) as a dev-dependency.
#[macro_export]
macro_rules! prompt_store_conformance {
    ($make:path) => {
        $crate::prompt_store_conformance!(@cases $make;
            insert_then_get_round_trips a_missing_prompt_is_none
            a_private_prompt_is_nobody_elses ids_are_global names_are_unique_per_owner
            a_rename_to_a_taken_name_is_name_taken_and_changes_nothing
            mine_lists_only_own_ordered_by_id pages_follow_the_id
            a_shared_prompt_is_seen_by_its_audience_only everyone_is_everyone
            roles_match_byte_for_byte own_shared_prompts_are_not_in_shared
            nothing_of_the_author_reaches_another_viewer unsharing_hides_at_once
            replace_with_the_current_revision_replaces_whole
            a_stale_replace_is_a_conflict_and_changes_nothing
            a_write_after_a_delete_is_not_found_and_brings_nothing_back
            a_stale_delete_is_a_conflict revisions_are_never_given_twice
            at_most_a_hundred_per_owner the_order_of_refusals
            concurrent_inserts_never_pass_the_limit
            concurrent_inserts_of_one_id_have_one_winner
            concurrent_replaces_from_one_revision_have_one_winner
            a_page_is_never_larger_than_the_page_size every_allowed_text_of_a_variable_is_read_back
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::prompt_store::$case(store).await,
                    None => eprintln!("skipped: no prompt store available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `TeamPresetStore` conformance case (ADR 0074): what the API
/// relies on from any store of people's team presets. `$make` is an `async fn() -> Option<S>`
/// returning a fresh store (`None` skips the suite). Each case uses owners of its own and gives up
/// after 60 s. The calling crate needs `tokio` (with `macros`, `rt` and `rt-multi-thread`, or
/// `time`) as a dev-dependency.
#[macro_export]
macro_rules! team_preset_store_conformance {
    ($make:path) => {
        $crate::team_preset_store_conformance!(@cases $make;
            insert_then_get_round_trips a_missing_preset_is_none every_field_round_trips
            bounds_are_refused_with_words another_owner_never_reads_or_writes_it ids_are_global
            names_are_unique_per_owner a_listing_is_the_owners_ordered_by_id pages_follow_the_id
            a_page_is_positive_and_bounded replace_with_the_current_revision_replaces_whole
            a_stale_replace_is_a_conflict_and_changes_nothing
            a_rename_to_a_taken_name_is_name_taken_and_changes_nothing
            delete_removes_the_preset_and_the_second_delete_is_not_found
            a_stale_delete_is_a_conflict
            a_write_after_a_delete_is_not_found_and_brings_nothing_back
            revisions_are_never_given_twice at_most_fifty_per_owner
            concurrent_inserts_never_pass_the_limit
            concurrent_inserts_of_one_id_have_one_winner
            concurrent_replaces_from_one_revision_have_one_winner
            concurrent_inserts_of_one_name_have_one_winner the_order_of_refusals
            a_summary_says_the_lead_and_the_member_count
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::teams::$case(store).await,
                    None => eprintln!("skipped: no team preset store available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `PendingAuthorizations` conformance case (ADR 0046, slice 4c).
/// `$make` is an `async fn() -> Option<S>` returning a fresh store (`None` skips the suite). Each case
/// uses owners, servers and states of its own and gives up after 60 s. The calling crate needs `tokio`
/// (with `macros` and `rt-multi-thread`) as a dev-dependency.
///
/// One case needs a store that can be **broken**: `pending_authorizations_conformance!(make, breaker =
/// path)` where `path` is an `async fn(&S)` that makes every later call of that store fail as an
/// unreachable store does (a Postgres adapter closes its pool; `MemoryPendingAuthorizations` is
/// taken down with `set_down(true)`). Without a `breaker` the case
/// `a_broken_store_is_unavailable_and_names_nothing` is skipped, and says so.
#[macro_export]
macro_rules! pending_authorizations_conformance {
    ($make:path) => {
        $crate::pending_authorizations_conformance!(@common $make);
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_broken_store_is_unavailable_and_names_nothing() {
            eprintln!(
                "skipped: this adapter supplies no breaker (`pending_authorizations_conformance!(make, \
                 breaker = path)`), so its errors are not looked into for a state"
            );
        }
    };
    ($make:path, breaker = $breaker:path) => {
        $crate::pending_authorizations_conformance!(@common $make);
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_broken_store_is_unavailable_and_names_nothing() {
            match $make().await {
                Some(store) => {
                    $crate::testkit::pending_authorizations::a_broken_store_is_unavailable_and_names_nothing(
                        store, $breaker,
                    )
                    .await
                }
                None => eprintln!("skipped: no store of pending authorizations available"),
            }
        }
    };
    (@common $make:path) => {
        $crate::pending_authorizations_conformance!(@cases $make;
            a_put_state_is_taken_once_with_its_owner_and_server
            another_state_never_matches
            a_state_that_has_ended_takes_nothing
            concurrent_takes_of_one_state_have_one_winner
            a_later_put_for_a_server_replaces_the_earlier_one
            owners_are_kept_apart
            the_same_state_put_again_belongs_to_the_later_put
            a_put_forgets_the_states_that_have_ended
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(store) => $crate::testkit::pending_authorizations::$case(store).await,
                    None => eprintln!("skipped: no store of pending authorizations available"),
                }
            }
        )*
    };
}

/// Generates one `#[tokio::test]` per `CredentialBroker` conformance case (ADR 0046, the amendment
/// of 2026-10-11). `$make` is an `async fn() -> Option<F>` returning a fresh, isolated
/// [`BrokerFixture`](credential_broker::BrokerFixture) (`None` skips the suite): a broker, the
/// stores it is composed over, a clock and an authorization server (see the module's docs for what
/// the server must enforce). Each case uses owners of its own and gives up after 60 s. The calling
/// crate needs `tokio` (with `macros` and `rt-multi-thread`) as a dev-dependency.
#[macro_export]
macro_rules! credential_broker_conformance {
    ($make:path) => {
        $crate::credential_broker_conformance!(@cases $make;
            a_bearer_token_is_stored_then_leased
            an_api_key_is_stored_then_leased_in_its_header
            storing_again_replaces_the_credential
            a_revoked_or_never_given_credential_needs_reauth
            a_missing_server_is_unknown
            a_server_of_kind_none_or_not_oauth_is_unknown
            another_owners_server_is_unknown
            a_credential_is_not_lent_for_another_url
            a_credential_is_not_lent_to_a_definition_made_again
            oauth_tokens_are_not_lent_for_another_url
            oauth_tokens_are_not_lent_after_the_scopes_or_the_client_id_change
            the_oauth_happy_path
            an_expired_access_token_is_refreshed
            a_token_is_refreshed_thirty_seconds_before_it_runs_out
            concurrent_leases_refresh_once
            a_refresh_racing_a_replica_is_settled_by_rotate
            replicas_refreshing_together_present_the_refresh_token_once
            leases_waiting_on_a_failing_refresh_share_its_outcome
            invalid_grant_on_refresh_needs_reauth
            a_failing_authorization_server_is_unavailable_and_keeps_the_grant
            a_wrong_state_is_unknown_and_leaves_the_flow
            a_state_is_used_once
            concurrent_callbacks_spend_the_state_once
            an_expired_flow_is_unknown
            another_owners_state_is_unknown
            begin_oauth_replaces_a_pending_flow
            a_refused_code_needs_reauth_and_spends_the_state
            an_authorization_server_without_s256_is_refused
            a_callback_with_the_issuer_of_the_flow_completes_it
            a_callback_with_another_issuer_is_refused_and_spends_nothing
            a_callback_without_iss_is_refused_when_the_server_advertised_it
            a_callback_with_iss_is_refused_when_the_server_did_not_advertise_it
            a_server_that_takes_no_part_in_rfc_9207_is_served
            a_code_of_another_authorization_server_is_refused
            a_flow_is_for_the_definition_it_began_with
            a_rename_or_a_reordering_of_the_scopes_does_not_end_a_flow
            revoke_forgets_tokens_and_pending_flow_and_tells_the_authorization_server
            no_error_or_debug_text_shows_a_secret
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some(fixture) => $crate::testkit::credential_broker::$case(fixture).await,
                    None => eprintln!("skipped: no credential broker available"),
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

/// Generates one `#[tokio::test]` per [`RunLeases`](crate::RunLeases) conformance case (ADR 0084).
/// `$make` is an `async fn() -> Option<F>` returning a fresh broker whose deployment token for
/// `glm-5.3` is [`run_leases::token`](crate::testkit::run_leases::token); `None` skips the suite.
/// The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! run_leases_conformance {
    ($make:path) => {
        #[tokio::test]
        async fn binds_then_leases_to_the_injector() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::binds_then_leases_to_the_injector(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn refuses_every_agent_its_own_run_included() {
            match $make().await {
                Some(b) => {
                    $crate::testkit::run_leases::refuses_every_agent_its_own_run_included(&b).await
                }
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn leases_to_the_orchestrator_too() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::leases_to_the_orchestrator_too(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn refuses_an_unknown_run() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::refuses_an_unknown_run(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn refuses_an_expired_binding() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::refuses_an_expired_binding(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn unbinds() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::unbinds(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn binds_once() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::binds_once(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn refuses_a_binding_with_no_model() {
            match $make().await {
                Some(b) => $crate::testkit::run_leases::refuses_a_binding_with_no_model(&b).await,
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn refuses_a_model_with_no_credential() {
            match $make().await {
                Some(b) => {
                    $crate::testkit::run_leases::refuses_a_model_with_no_credential(&b).await
                }
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
        #[tokio::test]
        async fn a_lease_never_outlives_its_binding() {
            match $make().await {
                Some(b) => {
                    $crate::testkit::run_leases::a_lease_never_outlives_its_binding(&b).await
                }
                None => eprintln!("skipped: no run-lease broker available"),
            }
        }
    };
}

/// Generates one `#[tokio::test]` per `VaultStore` conformance case (ADR 0083, amended 2026-11-21;
/// decision row 114 of 2026-11-22). `$make` is an `async fn() -> Option<(S, FixedClock)>` returning
/// a fresh store **and the clock its 24-hour grace runs on** (`None` skips the suite). The clock is
/// the fixture's so a case proves the grace by moving it: no case sleeps. Each case uses owners and
/// names of its own and gives up after 60 s. The calling crate needs `tokio` (with `macros` and
/// `rt-multi-thread`) as a dev-dependency.
#[macro_export]
macro_rules! vault_store_conformance {
    ($make:path) => {
        $crate::vault_store_conformance!(@cases $make;
            put_then_get_round_trips a_name_is_written_at_v1_and_the_next_write_at_v2
            material_reads_back_byte_for_byte a_reference_the_store_does_not_hold_is_unknown
            a_superseded_version_reads_for_the_grace_and_is_refused_after
            the_current_version_never_expires purge_removes_only_the_versions_past_the_grace
            concurrent_puts_take_two_versions_and_lose_nothing owners_never_see_each_others_vault
            no_error_or_debug_text_shows_a_secret
        );
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                match $make().await {
                    Some((store, clock)) => $crate::testkit::vault_store::$case(store, clock).await,
                    None => eprintln!("skipped: no vault store available"),
                }
            }
        )*
    };
}
