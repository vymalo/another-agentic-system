//! The in-memory [`AccountErase`]: the port's contract over a small world of per-person records.
//!
//! The other in-memory stores cannot be erased together without a refactor, so this adapter keeps
//! its own world: for each person, threads (with their files), prompts, memory notes, push devices
//! and tool servers, and the rows of a usage ledger. A test seeds and reads it through
//! [`EraseWorld`], the fixture the conformance testkit is written against.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jiff::{SignedDuration, Timestamp};
use orch_core::{ThreadId, UserId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::testkit::account_erase::{EraseWorld, Footprint, KeptRow, LateResult};
use crate::{AccountErase, EraseError, EraseOutcome};

struct ThreadRec {
    id: ThreadId,
    files: u64,
    working: bool,
}

#[derive(Default)]
struct Records {
    threads: Vec<ThreadRec>,
    prompts: u64,
    notes: u64,
    devices: u64,
    servers: u64,
}

impl Records {
    fn is_empty(&self) -> bool {
        self.threads.is_empty()
            && self.prompts == 0
            && self.notes == 0
            && self.devices == 0
            && self.servers == 0
    }
}

#[derive(Default)]
struct World {
    people: BTreeMap<UserId, Records>,
    ledger: Vec<KeptRow>,
    late_dropped: u64,
}

/// The in-memory [`AccountErase`]. Cheap to clone: clones share the world.
#[derive(Clone)]
pub struct MemoryAccountErase {
    pseudonym_key: Arc<Vec<u8>>,
    world: Arc<Mutex<World>>,
}

impl MemoryAccountErase {
    /// An empty world whose pseudonyms are a hash of the person's key under `pseudonym_key`.
    pub fn new(pseudonym_key: &[u8]) -> Self {
        MemoryAccountErase {
            pseudonym_key: Arc::new(pseudonym_key.to_vec()),
            world: Arc::new(Mutex::new(World::default())),
        }
    }

    /// The people who have records.
    pub fn people(&self) -> Vec<UserId> {
        self.world().people.keys().cloned().collect()
    }

    fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pseudonym(&self, account: &UserId) -> String {
        let mut hash = Sha256::new();
        hash.update(self.pseudonym_key.as_slice());
        hash.update([0u8]);
        hash.update(account.as_str().as_bytes());
        let digest = hash.finalize();
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("erased-{hex}")
    }
}

impl AccountErase for MemoryAccountErase {
    async fn erase(
        &self,
        account: &UserId,
        now: Timestamp,
        keep_days: u32,
    ) -> Result<EraseOutcome, EraseError> {
        if keep_days == 0 {
            return Err(EraseError::NoKeep);
        }
        let until = now
            .checked_add(SignedDuration::from_hours(i64::from(keep_days) * 24))
            .map_err(|_| EraseError::NoKeep)?;
        let pseudonym = self.pseudonym(account);
        let mut world = self.world();
        let has_rows = world.ledger.iter().any(|row| row.owner == account.as_str());
        let has_records = world.people.get(account).is_some_and(|r| !r.is_empty());
        if !has_records && !has_rows {
            return Err(EraseError::NotFound);
        }
        let running = world
            .people
            .get(account)
            .map_or(0, |r| r.threads.iter().filter(|t| t.working).count());
        if running > 0 {
            return Err(EraseError::Busy { running });
        }
        let records = world.people.remove(account).unwrap_or_default();
        let mut kept = false;
        for row in &mut world.ledger {
            if row.owner == account.as_str() {
                row.owner.clone_from(&pseudonym);
                row.kept_until = Some(until);
                kept = true;
            }
        }
        Ok(EraseOutcome {
            threads_erased: records.threads.len() as u64,
            files_pending: records.threads.iter().map(|t| t.files).sum(),
            kept_until: kept.then_some(until),
        })
    }
}

impl EraseWorld for MemoryAccountErase {
    async fn seed(&self, person: &UserId, usage: bool) {
        let mut world = self.world();
        let records = world.people.entry(person.clone()).or_default();
        for files in [2, 1] {
            records.threads.push(ThreadRec {
                id: ThreadId(Uuid::now_v7()),
                files,
                working: false,
            });
        }
        records.prompts += 2;
        records.notes += 3;
        records.devices += 1;
        records.servers += 2;
        if usage {
            for counts in [10, 5] {
                world.ledger.push(KeptRow {
                    owner: person.as_str().to_owned(),
                    counts,
                    kept_until: None,
                });
            }
        }
    }

    async fn set_running(&self, person: &UserId) {
        if let Some(thread) = self
            .world()
            .people
            .get_mut(person)
            .and_then(|r| r.threads.first_mut())
        {
            thread.working = true;
        }
    }

    async fn finish_running(&self, person: &UserId) {
        if let Some(records) = self.world().people.get_mut(person) {
            for thread in &mut records.threads {
                thread.working = false;
            }
        }
    }

    async fn footprint(&self, person: &UserId) -> Footprint {
        let world = self.world();
        world
            .people
            .get(person)
            .map_or_else(Footprint::default, |r| Footprint {
                threads: r.threads.len() as u64,
                files: r.threads.iter().map(|t| t.files).sum(),
                prompts: r.prompts,
                notes: r.notes,
                devices: r.devices,
                servers: r.servers,
            })
    }

    async fn threads_of(&self, person: &UserId) -> Vec<ThreadId> {
        self.world()
            .people
            .get(person)
            .map_or_else(Vec::new, |r| r.threads.iter().map(|t| t.id).collect())
    }

    async fn ledger(&self) -> Vec<KeptRow> {
        self.world().ledger.clone()
    }

    async fn late_result(&self, thread: ThreadId) -> LateResult {
        let mut world = self.world();
        let live = world
            .people
            .values()
            .any(|r| r.threads.iter().any(|t| t.id == thread));
        if live {
            LateResult::Applied
        } else {
            world.late_dropped += 1;
            LateResult::Dropped
        }
    }

    async fn late_dropped(&self) -> u64 {
        self.world().late_dropped
    }
}
