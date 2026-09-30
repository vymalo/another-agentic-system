use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{Event, Job, ThreadId, ThreadRecord, UserId};

use crate::{
    AgentBinding, BindingUpdate, Commit, CommitOutcome, Lease, NewThreadRecord, OutboxFinal,
    OutboxId, OutboxItem, OutboxKind, OutboxStats, OutboxStatus, StoreError, ThreadStore,
};

struct StoredEvent {
    event: Event,
    key: Option<String>,
}

struct ThreadEntry {
    record: ThreadRecord,
    events: Vec<StoredEvent>,
    binding: AgentBinding,
}

#[derive(Default)]
struct Inner {
    threads: HashMap<ThreadId, ThreadEntry>,
    /// Insertion order is the `ord` of the Postgres outbox.
    outbox: Vec<OutboxItem>,
    /// Failures the next `commit`s return instead of running (fault injection).
    commit_faults: std::collections::VecDeque<StoreError>,
    /// Failures the next `create_thread`s return instead of running (fault injection).
    create_faults: std::collections::VecDeque<StoreError>,
}

/// A [`ThreadStore`] in process memory. Clones share the data, which lets tests run two
/// application instances "on the same database".
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemoryStore")
    }
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next `n` commits fail with what `error` builds (a `StoreError` is not `Clone`),
    /// before anything is written: `|| StoreError::VersionConflict` exhausts the optimistic
    /// loop, `|| StoreError::unavailable(..)` simulates an outage. A running dispatcher commits
    /// too and would consume them: use it where nothing else writes.
    pub fn fail_next_commits(&self, n: usize, error: impl Fn() -> StoreError) {
        let mut inner = self.lock();
        for _ in 0..n {
            inner.commit_faults.push_back(error());
        }
    }

    /// The next `n` `create_thread`s fail with what `error` builds. Only the API creates
    /// threads, so this is safe next to a running dispatcher.
    pub fn fail_next_creates(&self, n: usize, error: impl Fn() -> StoreError) {
        let mut inner = self.lock();
        for _ in 0..n {
            inner.create_faults.push_back(error());
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn add(ts: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| ts.checked_add(d).ok())
        .unwrap_or(ts)
}

fn apply_binding(binding: &mut AgentBinding, update: &BindingUpdate) {
    if let Some(task) = &update.task_id {
        binding.task_id = Some(task.clone());
    }
    if let Some(state) = update.task_state {
        binding.task_state = Some(state);
    }
    if let Some(rev) = &update.revision {
        binding.revision = Some(rev.clone());
    }
}

/// Appends the commit's events, sets state and version, inserts outbox rows and applies the
/// binding. The caller has already checked the version and the idempotency keys.
fn write_commit(
    inner: &mut Inner,
    thread: ThreadId,
    commit: Commit,
) -> Option<(ThreadRecord, Vec<Event>)> {
    let entry = inner.threads.get_mut(&thread)?;
    let mut stored = Vec::with_capacity(commit.events.len());
    for new in commit.events {
        entry.record.last_seq += 1;
        let event = Event {
            seq: entry.record.last_seq,
            thread_id: thread,
            at: new.at,
            actor: new.actor,
            body: new.body,
        };
        entry.events.push(StoredEvent {
            event: event.clone(),
            key: new.idempotency_key,
        });
        stored.push(event);
    }
    entry.record.state = commit.new_state;
    if let Some(job) = commit.job {
        entry.record.job = job;
    }
    entry.record.version += 1;
    entry.record.updated_at = commit.now;
    if let Some(update) = &commit.binding {
        apply_binding(&mut entry.binding, update);
    }
    let record = entry.record.clone();
    for row in commit.outbox {
        inner.outbox.push(OutboxItem {
            id: row.id,
            thread_id: thread,
            kind: row.payload.kind(),
            payload: row.payload,
            status: OutboxStatus::Pending,
            attempts: 0,
            sent_at: None,
            next_attempt_at: commit.now,
            lease_owner: None,
            lease_until: None,
            last_error: None,
            created_at: commit.now,
        });
    }
    Some((record, stored))
}

/// Whether `lease` is the current claim of `row`.
fn holds(row: &OutboxItem, lease: &Lease) -> bool {
    row.id == lease.id
        && row.status == OutboxStatus::Inflight
        && row.lease_owner.as_deref() == Some(lease.owner.as_str())
        && row.attempts == lease.attempt
}

fn leased<'a>(inner: &'a mut Inner, lease: &Lease) -> Option<&'a mut OutboxItem> {
    inner.outbox.iter_mut().find(|r| holds(r, lease))
}

impl ThreadStore for MemoryStore {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn create_thread(
        &self,
        new: NewThreadRecord,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        let mut inner = self.lock();
        if let Some(fault) = inner.create_faults.pop_front() {
            return Err(fault);
        }
        if inner.threads.contains_key(&new.id) {
            return Err(StoreError::corrupt("thread id already exists"));
        }
        let record = ThreadRecord {
            id: new.id,
            owner: new.owner,
            title: new.title,
            target: new.target.clone(),
            state: first.new_state,
            job: Job::default(),
            version: 1,
            last_seq: 0,
            created_at: new.now,
            updated_at: new.now,
        };
        let binding = AgentBinding {
            thread_id: new.id,
            agent_id: new.target.agent_id,
            context_id: new.context_id,
            task_id: None,
            task_state: None,
            revision: None,
        };
        inner.threads.insert(
            new.id,
            ThreadEntry {
                record,
                events: Vec::new(),
                binding,
            },
        );
        let (mut record, events) = write_commit(&mut inner, new.id, first)
            .ok_or_else(|| StoreError::corrupt("thread vanished"))?;
        // Creation is version 1 whatever the first commit wrote.
        if let Some(entry) = inner.threads.get_mut(&new.id) {
            entry.record.version = 1;
        }
        record.version = 1;
        Ok((record, events))
    }

    async fn get_thread(
        &self,
        owner: Option<&UserId>,
        id: ThreadId,
    ) -> Result<Option<ThreadRecord>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .threads
            .get(&id)
            .filter(|e| owner.is_none_or(|o| &e.record.owner == o))
            .map(|e| e.record.clone()))
    }

    async fn list_threads(
        &self,
        owner: &UserId,
        before: Option<ThreadId>,
        limit: u32,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        let inner = self.lock();
        if let Some(cursor) = before
            && inner
                .threads
                .get(&cursor)
                .is_none_or(|e| &e.record.owner != owner)
        {
            return Ok(Vec::new());
        }
        let mut all: Vec<&ThreadRecord> = inner
            .threads
            .values()
            .map(|e| &e.record)
            .filter(|r| &r.owner == owner)
            .filter(|r| before.is_none_or(|b| r.id < b))
            .collect();
        all.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(all.into_iter().take(limit as usize).cloned().collect())
    }

    async fn commit(
        &self,
        thread: ThreadId,
        expected_version: i64,
        commit: Commit,
    ) -> Result<CommitOutcome, StoreError> {
        let mut inner = self.lock();
        if let Some(fault) = inner.commit_faults.pop_front() {
            return Err(fault);
        }
        let entry = inner.threads.get(&thread).ok_or(StoreError::NotFound)?;
        if let Some(lease) = &commit.lease
            && !inner
                .outbox
                .iter()
                .any(|r| r.thread_id == thread && holds(r, lease))
        {
            return Ok(CommitOutcome::Fenced);
        }
        if entry.record.version != expected_version {
            return Err(StoreError::VersionConflict);
        }
        let duplicate = commit
            .events
            .iter()
            .filter_map(|e| e.idempotency_key.as_deref())
            .any(|key| entry.events.iter().any(|s| s.key.as_deref() == Some(key)));
        if duplicate {
            return Ok(CommitOutcome::Duplicate);
        }
        let (thread, events) =
            write_commit(&mut inner, thread, commit).ok_or(StoreError::NotFound)?;
        Ok(CommitOutcome::Applied { thread, events })
    }

    async fn list_events(
        &self,
        thread: ThreadId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .threads
            .get(&thread)
            .map(|e| {
                e.events
                    .iter()
                    .filter(|s| s.event.seq > after)
                    .take(limit as usize)
                    .map(|s| s.event.clone())
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn get_binding(&self, thread: ThreadId) -> Result<Option<AgentBinding>, StoreError> {
        Ok(self.lock().threads.get(&thread).map(|e| e.binding.clone()))
    }

    async fn claim_outbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> Result<Vec<OutboxItem>, StoreError> {
        let mut inner = self.lock();
        let mut claimed = Vec::new();
        for i in 0..inner.outbox.len() {
            if claimed.len() >= limit as usize {
                break;
            }
            let row = &inner.outbox[i];
            let due = match row.status {
                OutboxStatus::Pending => row.next_attempt_at <= now,
                OutboxStatus::Inflight => row.lease_until.is_some_and(|until| until <= now),
                OutboxStatus::Delivered | OutboxStatus::Dead | OutboxStatus::Skipped => false,
            };
            if !due {
                continue;
            }
            let blocked = match row.kind {
                OutboxKind::Cancel => false,
                OutboxKind::Delegate => inner.outbox[..i].iter().any(|older| {
                    older.thread_id == row.thread_id
                        && older.kind == OutboxKind::Delegate
                        && older.status.is_open()
                }),
            };
            if blocked {
                continue;
            }
            let row = &mut inner.outbox[i];
            row.status = OutboxStatus::Inflight;
            row.lease_owner = Some(owner.to_owned());
            row.lease_until = Some(add(now, lease));
            row.attempts += 1;
            claimed.push(row.clone());
        }
        Ok(claimed)
    }

    async fn renew_lease(&self, lease: &Lease, until: Timestamp) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        Ok(leased(&mut inner, lease)
            .map(|r| r.lease_until = Some(until))
            .is_some())
    }

    async fn mark_sent(
        &self,
        lease: &Lease,
        binding: BindingUpdate,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        let Some(row) = leased(&mut inner, lease) else {
            return Ok(false);
        };
        row.sent_at = Some(now);
        let thread = row.thread_id;
        if let Some(entry) = inner.threads.get_mut(&thread) {
            apply_binding(&mut entry.binding, &binding);
        }
        Ok(true)
    }

    async fn retry_outbox(
        &self,
        lease: &Lease,
        next_attempt_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        Ok(leased(&mut inner, lease)
            .map(|r| {
                r.status = OutboxStatus::Pending;
                r.next_attempt_at = next_attempt_at;
                r.lease_owner = None;
                r.lease_until = None;
                r.last_error = Some(error);
            })
            .is_some())
    }

    async fn complete_outbox(
        &self,
        lease: &Lease,
        outcome: OutboxFinal,
        _now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        Ok(leased(&mut inner, lease)
            .map(|r| {
                match outcome {
                    OutboxFinal::Delivered => r.status = OutboxStatus::Delivered,
                    OutboxFinal::Dead { error } => {
                        r.status = OutboxStatus::Dead;
                        r.last_error = Some(error);
                    }
                    OutboxFinal::Skipped => r.status = OutboxStatus::Skipped,
                }
                r.lease_owner = None;
                r.lease_until = None;
            })
            .is_some())
    }

    async fn skip_unsent_delegates(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        let mut inner = self.lock();
        let mut n = 0;
        for row in inner.outbox.iter_mut().filter(|r| {
            r.thread_id == thread && r.kind == OutboxKind::Delegate && r.sent_at.is_none()
        }) {
            let skippable = match row.status {
                OutboxStatus::Pending => true,
                OutboxStatus::Inflight => row.lease_until.is_some_and(|until| until < now),
                OutboxStatus::Delivered | OutboxStatus::Dead | OutboxStatus::Skipped => false,
            };
            if skippable {
                row.status = OutboxStatus::Skipped;
                row.lease_owner = None;
                row.lease_until = None;
                n += 1;
            }
        }
        Ok(n)
    }

    async fn release_leases(&self, owner: &str, now: Timestamp) -> Result<u32, StoreError> {
        let mut inner = self.lock();
        let mut n = 0;
        for row in inner.outbox.iter_mut().filter(|r| {
            r.status == OutboxStatus::Inflight && r.lease_owner.as_deref() == Some(owner)
        }) {
            row.lease_until = Some(now);
            n += 1;
        }
        Ok(n)
    }

    async fn outbox_stats(&self, now: Timestamp) -> Result<OutboxStats, StoreError> {
        let inner = self.lock();
        let mut stats = OutboxStats::default();
        for row in &inner.outbox {
            // The instant a due row became due; `None` when it is not due.
            let due_since = match row.status {
                OutboxStatus::Pending if row.next_attempt_at <= now => Some(row.next_attempt_at),
                OutboxStatus::Inflight => row.lease_until.filter(|until| *until <= now),
                OutboxStatus::Pending
                | OutboxStatus::Delivered
                | OutboxStatus::Dead
                | OutboxStatus::Skipped => None,
            };
            if let Some(since) = due_since {
                stats.due += 1;
                stats.oldest_due_at = Some(stats.oldest_due_at.map_or(since, |o| o.min(since)));
            } else if row.status == OutboxStatus::Pending {
                stats.waiting += 1;
            } else if row.status == OutboxStatus::Inflight {
                stats.leased += 1;
            }
        }
        Ok(stats)
    }

    async fn get_outbox(&self, id: OutboxId) -> Result<Option<OutboxItem>, StoreError> {
        Ok(self.lock().outbox.iter().find(|r| r.id == id).cloned())
    }

    async fn list_open_outbox(&self, thread: ThreadId) -> Result<Vec<OutboxItem>, StoreError> {
        Ok(self
            .lock()
            .outbox
            .iter()
            .filter(|r| r.thread_id == thread && r.status.is_open())
            .cloned()
            .collect())
    }
}
