//! Runs the `AccountErase` conformance testkit against the in-memory adapter, and proves that the
//! kit can fail: each deliberately broken adapter below must fail the case that names its fault.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::{ThreadId, UserId};
use orch_ports::memory::MemoryAccountErase;
use orch_ports::testkit::account_erase::{self, EraseWorld, Footprint, KeptRow, LateResult};
use orch_ports::{AccountErase, EraseError, EraseOutcome};

async fn make() -> Option<MemoryAccountErase> {
    Some(MemoryAccountErase::new(b"a deployment secret"))
}

orch_ports::account_erase_conformance!(make);

#[tokio::test]
async fn clones_share_the_world() {
    let world = make().await.unwrap();
    let clone = world.clone();
    let person = UserId::new("alice@example.test");
    world.seed(&person, true).await;
    assert_eq!(clone.footprint(&person).await.threads, 2);
    assert_eq!(clone.people(), vec![person]);
}

#[tokio::test]
async fn the_pseudonym_depends_on_the_deployment_secret() {
    let person = UserId::new("alice@example.test");
    let mut owners = Vec::new();
    for key in [&b"one"[..], &b"two"[..]] {
        let world = MemoryAccountErase::new(key);
        world.seed(&person, true).await;
        world
            .erase(&person, Timestamp::from_second(1_700_000_000).unwrap(), 90)
            .await
            .unwrap();
        owners.push(world.ledger().await[0].owner.clone());
    }
    assert_ne!(owners[0], owners[1]);
}

// ---------------------------------------- deliberately broken adapters: the kit must go red

#[derive(Clone, Copy)]
enum Fault {
    /// An erase that also erases everybody else.
    ErasesTheNeighbour,
    /// An erase that does not look at running jobs.
    IgnoresARunningJob,
    /// An erase that leaves records behind (here: seeds them again).
    LeavesATrace,
    /// A world that applies a late result for an erased thread.
    AppliesALateResult,
}

struct Broken {
    inner: MemoryAccountErase,
    fault: Fault,
}

fn broken(fault: Fault) -> Broken {
    Broken {
        inner: MemoryAccountErase::new(b"a deployment secret"),
        fault,
    }
}

impl AccountErase for Broken {
    async fn erase(
        &self,
        account: &UserId,
        now: Timestamp,
        keep_days: u32,
    ) -> Result<EraseOutcome, EraseError> {
        match self.fault {
            Fault::ErasesTheNeighbour => {
                for other in self.inner.people() {
                    if &other != account {
                        let _ = self.inner.erase(&other, now, keep_days).await;
                    }
                }
                self.inner.erase(account, now, keep_days).await
            }
            Fault::IgnoresARunningJob => {
                self.inner.finish_running(account).await;
                self.inner.erase(account, now, keep_days).await
            }
            Fault::LeavesATrace => {
                let outcome = self.inner.erase(account, now, keep_days).await;
                if outcome.is_ok() {
                    self.inner.seed(account, false).await;
                }
                outcome
            }
            Fault::AppliesALateResult => self.inner.erase(account, now, keep_days).await,
        }
    }
}

impl EraseWorld for Broken {
    async fn seed(&self, person: &UserId, usage: bool) {
        self.inner.seed(person, usage).await;
    }

    async fn set_running(&self, person: &UserId) {
        self.inner.set_running(person).await;
    }

    async fn finish_running(&self, person: &UserId) {
        self.inner.finish_running(person).await;
    }

    async fn footprint(&self, person: &UserId) -> Footprint {
        self.inner.footprint(person).await
    }

    async fn threads_of(&self, person: &UserId) -> Vec<ThreadId> {
        self.inner.threads_of(person).await
    }

    async fn ledger(&self) -> Vec<KeptRow> {
        self.inner.ledger().await
    }

    async fn late_result(&self, thread: ThreadId) -> LateResult {
        match self.fault {
            Fault::AppliesALateResult => LateResult::Applied,
            _ => self.inner.late_result(thread).await,
        }
    }

    async fn late_dropped(&self) -> u64 {
        self.inner.late_dropped().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "another person's records are untouched")]
async fn an_erase_that_takes_the_neighbour_fails_the_kit() {
    account_erase::everything_of_the_person_is_gone_and_nothing_of_another_is_touched(broken(
        Fault::ErasesTheNeighbour,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "a running job must refuse the erase")]
async fn an_erase_that_ignores_a_running_job_fails_the_kit() {
    account_erase::a_person_with_a_running_job_is_refused_and_nothing_changes(broken(
        Fault::IgnoresARunningJob,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "everything of the person is gone")]
async fn an_erase_that_leaves_records_fails_the_kit() {
    account_erase::everything_of_the_person_is_gone_and_nothing_of_another_is_touched(broken(
        Fault::LeavesATrace,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "a late result for an erased thread was not dropped")]
async fn a_late_result_that_is_applied_fails_the_kit() {
    account_erase::a_late_result_for_an_erased_thread_is_dropped_and_counted(broken(
        Fault::AppliesALateResult,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unbroken_wrapper_passes_the_late_result_case() {
    account_erase::a_late_result_for_an_erased_thread_is_dropped_and_counted(
        MemoryAccountErase::new(b"k"),
    )
    .await;
}
