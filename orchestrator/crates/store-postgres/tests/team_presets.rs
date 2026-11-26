//! `TeamPresetStore` on `PgStore` against a real Postgres (ADR 0074): the conformance suite, and
//! what it cannot say about this adapter, which is how it is built.
//!
//! **This suite cannot skip.** The other Postgres tests print "skipped" when `ORCH_TEST_DATABASE_URL`
//! is unset, because they test a store a developer may not have; this one is the evidence that the
//! preset store's owner predicate is in the query, so a run without a database **fails** instead of
//! passing with nothing done. `the_suite_ran_every_case_and_skipped_none` adds a positive count: N
//! cases ran, none skipped.
//!
//! The table's CHECKs refuse what the rules would never write, the owner's advisory lock is held by a
//! transaction of the test and an insert is seen to wait, and a listing uses the `(owner, id)` index.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_core::{AgentId, AgentTarget, Timestamp, UserId};
use orch_core::{
    DEFAULT_MAX_HANDOFFS, DEFAULT_PUSH_TOKENS, HandoffApproval, MemberHistory, TeamMemberInput,
    TeamPreset, TeamPresetId, TeamPresetInput,
};
use orch_ports::{TEAM_PRESET_PAGE, TeamPresetStore, TeamPresetStoreError};
use orch_store_postgres::sql::{query, query_as, query_scalar};
use orch_store_postgres::{PgStore, TEAM_OWNER_LOCK_CLASS};
use support::TestDb;
use uuid::Uuid;

/// The number of cases of `orch_ports::team_preset_store_conformance!`: asserted below, so that a
/// case added to the macro without one here is caught.
const CASES: usize = 24;

/// The store of this test, or a failure: this suite has no "skipped" branch.
async fn store() -> PgStore {
    let db = TestDb::new().await.expect(
        "ORCH_TEST_DATABASE_URL is unset: the team preset conformance suite cannot skip, so it \
         fails here instead of passing with nothing done",
    );
    db.store().await
}

fn owner() -> UserId {
    UserId::new(&format!("{}@example.test", Uuid::now_v7().simple()))
}

fn id() -> TeamPresetId {
    TeamPresetId::from_entropy(*Uuid::now_v7().as_bytes())
}

fn at() -> Timestamp {
    Timestamp::from_second(1_700_000_000).expect("a time")
}

fn member(name: &str) -> TeamMemberInput {
    TeamMemberInput {
        name: name.to_owned(),
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        role: "writes it".to_owned(),
        history: MemberHistory::AgentKeeps,
    }
}

fn preset(name: &str) -> TeamPreset {
    TeamPreset::new(
        id(),
        TeamPresetInput {
            name: name.to_owned(),
            title: format!("The team {name}"),
            lead: "lead".to_owned(),
            members: vec![member("lead"), member("helper")],
            charter: "Ship it together.".to_owned(),
            max_handoffs: DEFAULT_MAX_HANDOFFS,
            handoff_approval: HandoffApproval::Ask,
            checkpoints: Default::default(),
            push_tokens: DEFAULT_PUSH_TOKENS,
            summary: false,
        },
    )
    .expect("a valid preset")
}

macro_rules! case {
    ($name:ident) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn $name() {
            orch_ports::testkit::teams::$name(store().await).await;
        }
    };
}

case!(insert_then_get_round_trips);
case!(a_missing_preset_is_none);
case!(every_field_round_trips);
case!(bounds_are_refused_with_words);
case!(another_owner_never_reads_or_writes_it);
case!(ids_are_global);
case!(names_are_unique_per_owner);
case!(a_listing_is_the_owners_ordered_by_id);
case!(pages_follow_the_id);
case!(a_page_is_positive_and_bounded);
case!(replace_with_the_current_revision_replaces_whole);
case!(a_stale_replace_is_a_conflict_and_changes_nothing);
case!(a_rename_to_a_taken_name_is_name_taken_and_changes_nothing);
case!(delete_removes_the_preset_and_the_second_delete_is_not_found);
case!(a_stale_delete_is_a_conflict);
case!(a_write_after_a_delete_is_not_found_and_brings_nothing_back);
case!(revisions_are_never_given_twice);
case!(at_most_fifty_per_owner);
case!(concurrent_inserts_never_pass_the_limit);
case!(concurrent_inserts_of_one_id_have_one_winner);
case!(concurrent_replaces_from_one_revision_have_one_winner);
case!(concurrent_inserts_of_one_name_have_one_winner);
case!(the_order_of_refusals);
case!(a_summary_says_the_lead_and_the_member_count);

/// Every case of the suite ran: the count is positive, and it is every case.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_suite_ran_every_case_and_skipped_none() {
    let mut ran = 0usize;
    macro_rules! run {
        ($name:ident) => {{
            eprintln!("team preset case {}:", stringify!($name));
            orch_ports::testkit::teams::$name(store().await).await;
            ran += 1;
        }};
    }
    run!(insert_then_get_round_trips);
    run!(a_missing_preset_is_none);
    run!(every_field_round_trips);
    run!(bounds_are_refused_with_words);
    run!(another_owner_never_reads_or_writes_it);
    run!(ids_are_global);
    run!(names_are_unique_per_owner);
    run!(a_listing_is_the_owners_ordered_by_id);
    run!(pages_follow_the_id);
    run!(a_page_is_positive_and_bounded);
    run!(replace_with_the_current_revision_replaces_whole);
    run!(a_stale_replace_is_a_conflict_and_changes_nothing);
    run!(a_rename_to_a_taken_name_is_name_taken_and_changes_nothing);
    run!(delete_removes_the_preset_and_the_second_delete_is_not_found);
    run!(a_stale_delete_is_a_conflict);
    run!(a_write_after_a_delete_is_not_found_and_brings_nothing_back);
    run!(revisions_are_never_given_twice);
    run!(at_most_fifty_per_owner);
    run!(concurrent_inserts_never_pass_the_limit);
    run!(concurrent_inserts_of_one_id_have_one_winner);
    run!(concurrent_replaces_from_one_revision_have_one_winner);
    run!(concurrent_inserts_of_one_name_have_one_winner);
    run!(the_order_of_refusals);
    run!(a_summary_says_the_lead_and_the_member_count);
    assert!(ran > 0, "the suite ran no case at all");
    assert_eq!(ran, CASES, "the suite ran {ran} of {CASES} cases");
}

// --------------------------------------------------------------------------------- the table

/// The table's CHECKs refuse a row the rules would never write: a `members` array of one or nine, a
/// cap out of 6 to 20, a budget out of 4 000 to 100 000, and an approval whose seconds disagree with
/// its mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tables_checks_refuse_what_the_rules_refuse() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    let store = db.store().await;
    let owner = owner();
    let stored = store
        .insert_team_preset(&owner, &preset("checks"), at())
        .await
        .expect("insert");
    let held = stored.preset().id().as_str().to_owned();
    let members = serde_json::to_string(&vec![member("only")]).expect("json");
    let rows: Vec<(&str, String, Option<&'static str>)> = vec![
        (
            "UPDATE user_team_presets SET members = $2::text::jsonb WHERE id = $1",
            members,
            Some("user_team_presets_members_check"),
        ),
        (
            "UPDATE user_team_presets SET max_handoffs = 21 WHERE id = $1",
            String::new(),
            Some("user_team_presets_max_handoffs_check"),
        ),
        (
            "UPDATE user_team_presets SET push_tokens = 3999 WHERE id = $1",
            String::new(),
            Some("user_team_presets_push_tokens_check"),
        ),
        (
            // the row is `ask` with no seconds: giving it seconds is the disagreement
            "UPDATE user_team_presets SET handoff_after_seconds = 30 WHERE id = $1",
            String::new(),
            Some("user_team_presets_after_seconds_match_mode"),
        ),
    ];
    for (sql, argument, constraint) in rows {
        let result = if argument.is_empty() {
            query(sql).bind(&held).execute(store.pool()).await
        } else {
            query(sql)
                .bind(&held)
                .bind(argument)
                .execute(store.pool())
                .await
        };
        let error = result.expect_err(sql);
        let words = format!("{error:?}");
        assert!(
            constraint.is_none_or(|c| words.contains(c)),
            "{sql}: {words}"
        );
    }
}

/// The limit of fifty is kept under the owner's advisory lock: a transaction that holds
/// `pg_advisory_xact_lock(0x7470_7273, hashtext(owner))` makes an insert of that owner wait, and the
/// insert finishes once the lock is released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_insert_waits_for_the_owners_lock() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    let store = db.store().await;
    let owner = owner();
    let pool = db.pool("orch-test-lock", 2).await;
    let mut tx = pool.begin().await.expect("begin");
    query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(TEAM_OWNER_LOCK_CLASS)
        .bind(owner.as_str())
        .execute(&mut *tx)
        .await
        .expect("take the lock");

    let waiting = store.clone();
    let who = owner.clone();
    let task = tokio::spawn(async move {
        waiting
            .insert_team_preset(&who, &preset("behind-the-lock"), at())
            .await
    });
    wait_until_someone_waits_for(&pool, &owner).await;
    assert!(!task.is_finished(), "the insert did not wait for the lock");
    tx.rollback().await.expect("release");
    let stored = task.await.expect("task").expect("insert");
    assert_eq!(stored.preset().name(), "behind-the-lock");
}

/// Waits until a writer is seen waiting for `owner`'s lock (`pg_locks`), so that the test never
/// sleeps for a fixed time and never races the insert.
async fn wait_until_someone_waits_for(pool: &orch_store_postgres::PgPool, owner: &UserId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: i64 = query_scalar(
                "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
                 AND classid::bigint = $1 AND objsubid = 2 \
                 AND objid::bigint = (hashtext($2)::bigint & 4294967295)",
            )
            .bind(i64::from(TEAM_OWNER_LOCK_CLASS))
            .bind(owner.as_str())
            .fetch_one(pool)
            .await
            .expect("read pg_locks");
            if waiting > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a writer waits for the lock of the owner");
}

/// A row that is no preset is `Corrupt`, says nothing of it, and a listing of the owner still
/// works: it reads the array's length and never parses a member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_that_is_not_a_preset_is_corrupt_and_does_not_break_the_listing() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    let store = db.store().await;
    let owner = owner();
    let stored = store
        .insert_team_preset(&owner, &preset("broken"), at())
        .await
        .expect("insert");
    let held = stored.preset().id().as_str().to_owned();
    // the members are not members: the row is still a row (the CHECK cannot read them)
    query("UPDATE user_team_presets SET members = '[1, 2]'::jsonb WHERE id = $1")
        .bind(&held)
        .execute(store.pool())
        .await
        .expect("corrupt the row");
    let read = store
        .get_team_preset(&owner, stored.preset().id())
        .await
        .expect_err("a row that is no preset");
    match &read {
        TeamPresetStoreError::Corrupt(words) => {
            assert!(
                !words.contains(&held),
                "the error names the preset: {words}"
            );
            assert!(!words.contains(owner.as_str()));
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
    // the listing still answers, and does not parse the members
    let page = store
        .list_team_presets(&owner, None, TEAM_PRESET_PAGE as u32)
        .await
        .expect("list");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].member_count, 2);
}

/// The listing uses the `(owner, id)` index: with sequential scans forbidden, the plan is an index
/// scan of `user_team_presets_owner_id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_listing_uses_the_owner_index() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    // the store migrates this test's schema: the table has to exist before it is asked a plan
    let store = db.store().await;
    let owner = owner();
    let mut tx = store.pool().begin().await.expect("begin");
    query("SET LOCAL enable_seqscan = off")
        .execute(&mut tx)
        .await
        .expect("disable sequential scans");
    let plan: Vec<(String,)> = query_as(
        "EXPLAIN SELECT id, name, title, lead, jsonb_array_length(members), revision, updated_at \
         FROM user_team_presets WHERE owner = $1 AND ($2::text IS NULL OR id > $2) \
         ORDER BY id LIMIT $3",
    )
    .bind(owner.as_str())
    .bind(None::<String>)
    .bind(5_i64)
    .fetch_all(&mut tx)
    .await
    .expect("explain");
    tx.rollback().await.expect("done");
    let plan: String = plan
        .into_iter()
        .map(|(line,)| line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("user_team_presets_owner_id"),
        "the listing does not use the owner's index:\n{plan}"
    );
}

/// The revision is one number of one sequence for the whole table: two people's writes never share
/// one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revision_is_one_number_of_one_sequence() {
    let store = store().await;
    let a = store
        .insert_team_preset(&owner(), &preset("a"), at())
        .await
        .expect("insert")
        .revision();
    let b = store
        .insert_team_preset(&owner(), &preset("b"), at())
        .await
        .expect("insert")
        .revision();
    assert_ne!(a, b);
    let next: i64 = query_scalar("SELECT last_value FROM user_team_preset_revisions")
        .fetch_one(store.pool())
        .await
        .expect("the sequence");
    assert!(u64::try_from(next).expect("positive") >= a.get().max(b.get()));
}
