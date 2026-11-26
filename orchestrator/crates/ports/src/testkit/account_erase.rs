//! `AccountErase` conformance cases: what the account route relies on from any place that erases
//! what a person owns (ADR 0098), stated once and run against the in-memory adapter and every
//! adapter after it.
//!
//! An implementation supplies a fresh world as an [`EraseWorld`]: the adapter itself, and the means
//! to seed a person's records of every class, to count what is left, to read the kept ledger rows and
//! to deliver a late result (`async fn() -> Option<W>`; `None` skips the suite). Each case uses
//! people of its own and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how the pseudonym is made (only that it is not the key, does
//! not contain it and differs between people), how the files are queued, and the wording of an error.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{ThreadId, UserId};
use uuid::Uuid;

use crate::{AccountErase, EraseError};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// The keep period the cases ask for.
const KEEP_DAYS: u32 = 90;

/// How many of each class of record a person has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    pub threads: u64,
    pub files: u64,
    pub prompts: u64,
    pub notes: u64,
    pub devices: u64,
    pub servers: u64,
}

/// One row of the usage ledger as the fixture reads it: who it names, its counts, and until when it
/// is kept (`None` while its owner has not erased the account). It carries no thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptRow {
    pub owner: String,
    pub counts: u64,
    pub kept_until: Option<Timestamp>,
}

/// What became of a result that reached the system for a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateResult {
    Applied,
    Dropped,
}

/// The adapter under test and the means to see into the world it erases from.
pub trait EraseWorld: AccountErase {
    /// Gives `person` two threads (three files), two prompts, three memory notes, one device and two
    /// tool servers, and, when `usage`, two ledger rows (counts 10 and 5) that name the person.
    fn seed(&self, person: &UserId, usage: bool) -> impl Future<Output = ()> + Send;
    /// One of `person`'s threads starts working.
    fn set_running(&self, person: &UserId) -> impl Future<Output = ()> + Send;
    /// Every thread of `person` stops working.
    fn finish_running(&self, person: &UserId) -> impl Future<Output = ()> + Send;
    /// What `person` has, per class.
    fn footprint(&self, person: &UserId) -> impl Future<Output = Footprint> + Send;
    /// The ids of `person`'s threads.
    fn threads_of(&self, person: &UserId) -> impl Future<Output = Vec<ThreadId>> + Send;
    /// Every row of the ledger, whoever it names.
    fn ledger(&self) -> impl Future<Output = Vec<KeptRow>> + Send;
    /// A result arrives for `thread`: applied when the thread exists, else dropped and counted.
    fn late_result(&self, thread: ThreadId) -> impl Future<Output = LateResult> + Send;
    /// How many late results were dropped.
    fn late_dropped(&self) -> impl Future<Output = u64> + Send;
}

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

fn person() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

fn now() -> Timestamp {
    Timestamp::from_second(1_700_000_000).expect("a time")
}

fn kept_until() -> Timestamp {
    now()
        .checked_add(SignedDuration::from_hours(i64::from(KEEP_DAYS) * 24))
        .expect("a time")
}

fn rows_of(rows: &[KeptRow], person: &UserId) -> Vec<KeptRow> {
    rows.iter()
        .filter(|row| row.owner == person.as_str())
        .cloned()
        .collect()
}

fn assert_seeded(footprint: Footprint) {
    assert!(
        footprint.threads > 0
            && footprint.files > 0
            && footprint.prompts > 0
            && footprint.notes > 0
            && footprint.devices > 0
            && footprint.servers > 0,
        "the fixture seeded nothing of a class: {footprint:?}"
    );
}

/// Everything of the person is gone, the counts say how much, and another person's records of every
/// class (and ledger rows) are as they were.
pub async fn everything_of_the_person_is_gone_and_nothing_of_another_is_touched<W: EraseWorld>(
    world: W,
) {
    within(async {
        let (a, b) = (person(), person());
        world.seed(&a, true).await;
        world.seed(&b, true).await;
        let before_a = world.footprint(&a).await;
        let before_b = world.footprint(&b).await;
        assert_seeded(before_a);
        let rows_b = rows_of(&world.ledger().await, &b);
        assert!(!rows_b.is_empty(), "the fixture seeded no ledger rows");

        let outcome = world.erase(&a, now(), KEEP_DAYS).await.expect("the erase");
        assert_eq!(outcome.threads_erased, before_a.threads, "threads erased");
        assert_eq!(outcome.files_pending, before_a.files, "files pending");
        assert_eq!(
            world.footprint(&a).await,
            Footprint::default(),
            "everything of the person is gone"
        );
        assert!(
            world.threads_of(&a).await.is_empty(),
            "the person still has threads"
        );
        assert_eq!(
            world.footprint(&b).await,
            before_b,
            "another person's records are untouched"
        );
        assert_eq!(
            rows_of(&world.ledger().await, &b),
            rows_b,
            "another person's ledger rows are untouched"
        );
    })
    .await;
}

/// A person with a running job is refused, with the number of threads that work, and nothing is
/// changed; once the work ends the erase goes through.
pub async fn a_person_with_a_running_job_is_refused_and_nothing_changes<W: EraseWorld>(world: W) {
    within(async {
        let a = person();
        world.seed(&a, true).await;
        world.set_running(&a).await;
        let before = world.footprint(&a).await;
        let ledger = world.ledger().await;
        let thread = world.threads_of(&a).await[0];

        let refused = world.erase(&a, now(), KEEP_DAYS).await;
        assert_eq!(
            refused,
            Err(EraseError::Busy { running: 1 }),
            "a running job must refuse the erase"
        );
        assert_eq!(
            world.footprint(&a).await,
            before,
            "a refused erase changed the records"
        );
        assert_eq!(
            world.ledger().await,
            ledger,
            "a refused erase changed the ledger"
        );
        assert_eq!(
            world.late_result(thread).await,
            LateResult::Applied,
            "a refused erase removed a thread"
        );

        world.finish_running(&a).await;
        world
            .erase(&a, now(), KEEP_DAYS)
            .await
            .expect("the erase once the work ended");
        assert_eq!(world.footprint(&a).await, Footprint::default());
    })
    .await;
}

/// Another person's running job does not stop this person's erase.
pub async fn a_neighbours_running_job_does_not_block_the_erase<W: EraseWorld>(world: W) {
    within(async {
        let (a, b) = (person(), person());
        world.seed(&a, false).await;
        world.seed(&b, false).await;
        world.set_running(&b).await;
        world
            .erase(&a, now(), KEEP_DAYS)
            .await
            .expect("the neighbour's job is not this person's");
        assert_eq!(world.footprint(&a).await, Footprint::default());
        assert_eq!(world.footprint(&b).await.threads, 2);
    })
    .await;
}

/// Only counts remain: no record of any class, and the ledger rows kept under a pseudonym that is not
/// the key, with the same counts and the time they are kept until.
pub async fn only_the_minimal_kept_records_remain<W: EraseWorld>(world: W) {
    within(async {
        let (a, b) = (person(), person());
        world.seed(&a, true).await;
        world.seed(&b, true).await;
        let counts_a: u64 = rows_of(&world.ledger().await, &a)
            .iter()
            .map(|r| r.counts)
            .sum();
        assert!(counts_a > 0, "the fixture seeded no counts");

        let outcome_a = world.erase(&a, now(), KEEP_DAYS).await.expect("erase a");
        assert_eq!(outcome_a.kept_until, Some(kept_until()), "keptUntil");
        assert_eq!(world.footprint(&a).await, Footprint::default());

        let ledger = world.ledger().await;
        assert!(
            rows_of(&ledger, &a).is_empty(),
            "a ledger row still names the person"
        );
        let kept: Vec<&KeptRow> = ledger
            .iter()
            .filter(|row| row.owner != b.as_str())
            .collect();
        assert_eq!(kept.len(), 2, "the counts of the person are kept");
        assert_eq!(
            kept.iter().map(|r| r.counts).sum::<u64>(),
            counts_a,
            "the kept counts changed"
        );
        for row in &kept {
            assert!(
                !row.owner.contains(a.as_str()),
                "the key is in a kept row's owner"
            );
            assert_eq!(row.kept_until, Some(kept_until()), "a kept row's time");
        }
        let pseudonyms_a: BTreeSet<&str> = kept.iter().map(|r| r.owner.as_str()).collect();
        assert_eq!(pseudonyms_a.len(), 1, "one person, one pseudonym");

        world.erase(&b, now(), KEEP_DAYS).await.expect("erase b");
        let all: BTreeSet<String> = world.ledger().await.into_iter().map(|r| r.owner).collect();
        assert_eq!(all.len(), 2, "two people share a pseudonym");
        assert!(
            world.ledger().await.iter().all(|r| r.owner != b.as_str()),
            "a ledger row still names the second person"
        );
    })
    .await;
}

/// A person who never used anything has no kept counts and no `kept_until`.
pub async fn a_person_without_usage_has_no_kept_until<W: EraseWorld>(world: W) {
    within(async {
        let a = person();
        world.seed(&a, false).await;
        let outcome = world.erase(&a, now(), KEEP_DAYS).await.expect("the erase");
        assert_eq!(outcome.kept_until, None, "keptUntil without usage rows");
        assert_eq!(world.footprint(&a).await, Footprint::default());
        assert!(
            world.ledger().await.is_empty(),
            "the erase made ledger rows"
        );
    })
    .await;
}

/// A result that arrives for an erased thread is dropped and counted; one for a thread that exists
/// is applied and not counted.
pub async fn a_late_result_for_an_erased_thread_is_dropped_and_counted<W: EraseWorld>(world: W) {
    within(async {
        let (a, b) = (person(), person());
        world.seed(&a, false).await;
        world.seed(&b, false).await;
        let erased = world.threads_of(&a).await;
        let alive = world.threads_of(&b).await;
        assert!(!erased.is_empty() && !alive.is_empty());
        let counted = world.late_dropped().await;

        world.erase(&a, now(), KEEP_DAYS).await.expect("the erase");
        for thread in &erased {
            assert_eq!(
                world.late_result(*thread).await,
                LateResult::Dropped,
                "a late result for an erased thread was not dropped"
            );
        }
        assert_eq!(
            world.late_dropped().await,
            counted + erased.len() as u64,
            "the dropped late results are not counted"
        );
        assert_eq!(
            world.late_result(alive[0]).await,
            LateResult::Applied,
            "a result for a thread that exists was dropped"
        );
        assert_eq!(
            world.late_dropped().await,
            counted + erased.len() as u64,
            "a result that was applied was counted as dropped"
        );
    })
    .await;
}

/// A second erase is `NotFound` and touches nobody; so is the erase of a person who never had anything.
pub async fn a_second_erase_is_not_found_and_touches_nobody<W: EraseWorld>(world: W) {
    within(async {
        let (a, b) = (person(), person());
        world.seed(&a, true).await;
        world.seed(&b, true).await;
        world.erase(&a, now(), KEEP_DAYS).await.expect("the erase");
        let before_b = world.footprint(&b).await;
        let ledger = world.ledger().await;

        assert_eq!(
            world.erase(&a, now(), KEEP_DAYS).await,
            Err(EraseError::NotFound),
            "a second erase"
        );
        assert_eq!(
            world.erase(&person(), now(), KEEP_DAYS).await,
            Err(EraseError::NotFound),
            "a person who never had anything"
        );
        assert_eq!(world.footprint(&b).await, before_b);
        assert_eq!(
            world.ledger().await,
            ledger,
            "a refused erase changed the ledger"
        );
    })
    .await;
}

/// A keep of zero days is refused and nothing is changed.
pub async fn a_keep_of_zero_days_is_refused_and_changes_nothing<W: EraseWorld>(world: W) {
    within(async {
        let a = person();
        world.seed(&a, true).await;
        let before = world.footprint(&a).await;
        let ledger = world.ledger().await;
        assert_eq!(
            world.erase(&a, now(), 0).await,
            Err(EraseError::NoKeep),
            "0 is not a value"
        );
        assert_eq!(world.footprint(&a).await, before);
        assert_eq!(world.ledger().await, ledger);
    })
    .await;
}
