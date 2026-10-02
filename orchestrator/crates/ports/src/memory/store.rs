use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    EditLink, Event, EventBody, EventKind, ForkKind, ForkNode, ForkedFrom, Job, ThreadId,
    ThreadRecord, UserId,
};

use crate::{
    AgentBinding, BindingUpdate, Commit, CommitOutcome, ForkOrigin, InboxFinal, InboxId, InboxItem,
    InboxLease, InboxStatus, Lease, NewInbox, NewThreadRecord, OutboxFinal, OutboxId, OutboxItem,
    OutboxKind, OutboxStats, OutboxStatus, Parking, Received, StoreError, TIMER_SOURCE,
    ThreadStore,
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
    /// Insertion order is the order the inbox is claimed in among rows due at the same time.
    inbox: Vec<InboxItem>,
    /// `watches.key` to the thread; the first thread to watch a key keeps it.
    watches: HashMap<String, ThreadId>,
    /// Failures the next `commit`s return instead of running (fault injection).
    commit_faults: std::collections::VecDeque<StoreError>,
    /// Failures the next `create_thread`s return instead of running (fault injection).
    create_faults: std::collections::VecDeque<StoreError>,
    /// Commits refused because the outbox or inbox claim they carried was no longer held.
    fenced_commits: usize,
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

    /// Every outbox row of `thread`, whatever its status, oldest first: what a test looks at to see
    /// what was asked and how each request ended (the port reads only the open ones).
    pub fn outbox_of(&self, thread: ThreadId) -> Vec<OutboxItem> {
        self.lock()
            .outbox
            .iter()
            .filter(|r| r.thread_id == thread)
            .cloned()
            .collect()
    }

    /// How many commits have been refused so far because the claim they carried was lost: what
    /// a test waits for to know that a worker whose claim was taken over has tried to write.
    pub fn fenced_commits(&self) -> usize {
        self.lock().fenced_commits
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

/// Finishes the thread's unsent `delegate` rows as `skipped`: those `pending`, and those
/// `inflight` whose lease expired before `now`. Returns the count.
fn skip_unsent(inner: &mut Inner, thread: ThreadId, now: Timestamp) -> u32 {
    let mut n = 0;
    for row in inner
        .outbox
        .iter_mut()
        .filter(|r| r.thread_id == thread && r.kind == OutboxKind::Delegate && r.sent_at.is_none())
    {
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
    n
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
    if let Some(title) = commit.title {
        entry.record.title = title;
    }
    if let Some(description) = commit.description {
        entry.record.description = Some(description).filter(|d| !d.is_empty());
    }
    entry.record.version += 1;
    entry.record.updated_at = commit.now;
    if let Some(update) = &commit.binding {
        apply_binding(&mut entry.binding, update);
    }
    let record = entry.record.clone();
    for key in commit.watches {
        inner
            .watches
            .entry(key.as_str().to_owned())
            .or_insert(thread);
        // The same step re-arms what arrived before the watch existed.
        for row in inner.inbox.iter_mut().filter(|r| {
            r.status == InboxStatus::Parked && r.correlation.as_deref() == Some(key.as_str())
        }) {
            row.status = InboxStatus::Pending;
            row.available_at = commit.now;
            row.parked_at = None;
            // Parking was a wait, not a failure: the claims so far do not count.
            row.refunded = row.attempts;
        }
    }
    for timer in commit.timers {
        let key = timer.idempotency_key(thread);
        if inner
            .inbox
            .iter()
            .any(|r| r.source == TIMER_SOURCE && r.idempotency_key == key)
        {
            continue;
        }
        inner.inbox.push(new_row(
            NewInbox {
                id: timer.id,
                source: TIMER_SOURCE.to_owned(),
                idempotency_key: key,
                payload: timer.payload(thread),
                correlation: None,
            },
            timer.due(commit.now),
            commit.now,
        ));
    }
    if let Some(lease) = &commit.inbox
        && let Some(row) = held_inbox(inner, lease)
    {
        row.status = InboxStatus::Applied;
        row.lease_owner = None;
        row.lease_until = None;
    }
    if let (Some(lease), Some(outcome)) = (&commit.lease, commit.finishes_outbox.clone())
        && let Some(row) = leased(inner, lease)
    {
        finish_row(row, outcome);
    }
    // The abandoned job's unsent delegations are finished before this commit's own rows exist,
    // in the same step (ADR 0036).
    if commit.skip_unsent_delegates {
        skip_unsent(inner, thread, commit.now);
    }
    for row in commit.outbox {
        inner.outbox.push(OutboxItem {
            id: row.id,
            thread_id: thread,
            kind: row.payload.kind(),
            payload: row.payload,
            status: OutboxStatus::Pending,
            attempts: 0,
            sent_at: None,
            task_id: None,
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

/// Whether `lease` is the current claim of `row`.
fn inbox_holds(row: &InboxItem, lease: &InboxLease) -> bool {
    row.id == lease.id
        && row.status == InboxStatus::Inflight
        && row.lease_owner.as_deref() == Some(lease.owner.as_str())
        && row.attempts == lease.attempt
}

fn held_inbox<'a>(inner: &'a mut Inner, lease: &InboxLease) -> Option<&'a mut InboxItem> {
    inner.inbox.iter_mut().find(|r| inbox_holds(r, lease))
}

/// A new `pending` row, due at `available_at`.
fn new_row(new: NewInbox, available_at: Timestamp, now: Timestamp) -> InboxItem {
    InboxItem {
        id: new.id,
        source: new.source,
        idempotency_key: new.idempotency_key,
        kind: new.payload.kind().to_owned(),
        payload: serde_json::to_value(&new.payload).unwrap_or(serde_json::Value::Null),
        correlation: new.correlation,
        status: InboxStatus::Pending,
        available_at,
        attempts: 0,
        refunded: 0,
        lease_owner: None,
        lease_until: None,
        parked_at: None,
        last_error: None,
        created_at: now,
    }
}

/// Ends a claimed row as `outcome` and lets go of the claim.
fn finish_row(r: &mut OutboxItem, outcome: OutboxFinal) {
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
}

fn leased<'a>(inner: &'a mut Inner, lease: &Lease) -> Option<&'a mut OutboxItem> {
    inner.outbox.iter_mut().find(|r| holds(r, lease))
}

/// Inserts the thread `new` and writes its first commit; for a fork, after the parent's events up
/// to the cut have been copied in.
fn insert_thread(
    inner: &mut Inner,
    new: NewThreadRecord,
    fork: Option<ForkOrigin>,
    first: Commit,
) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
    // A thread that does not exist yet has no inbox row to be applied.
    let first = Commit {
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        skip_unsent_delegates: false,
        ..first
    };
    if inner.threads.contains_key(&new.id) {
        return Err(StoreError::corrupt("thread id already exists"));
    }
    let copied: Vec<StoredEvent> = match fork {
        None => Vec::new(),
        Some(origin) => {
            let parent = inner
                .threads
                .get(&origin.parent)
                .filter(|e| e.record.owner == new.owner)
                .ok_or(StoreError::NotFound)?;
            if parent.record.last_seq < origin.cut || origin.cut < 0 {
                return Err(StoreError::corrupt("the cut is beyond the parent's log"));
            }
            parent
                .events
                .iter()
                .filter(|s| s.event.seq <= origin.cut)
                .map(|s| StoredEvent {
                    event: Event {
                        thread_id: new.id,
                        ..s.event.clone()
                    },
                    key: None,
                })
                .collect()
        }
    };
    let record = ThreadRecord {
        id: new.id,
        owner: new.owner,
        title: new.title,
        description: new.description.filter(|d| !d.is_empty()),
        target: new.target.clone(),
        state: first.new_state,
        job: Job::default(),
        version: 1,
        forked_from: fork.map(|o| ForkedFrom {
            thread_id: Some(o.parent),
            seq: o.cut,
            kind: o.kind,
        }),
        last_seq: fork.map_or(0, |o| o.cut),
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
            events: copied,
            binding,
        },
    );
    let (mut record, events) =
        write_commit(inner, new.id, first).ok_or_else(|| StoreError::corrupt("thread vanished"))?;
    // Creation is version 1 whatever the first commit wrote.
    if let Some(entry) = inner.threads.get_mut(&new.id) {
        entry.record.version = 1;
    }
    record.version = 1;
    Ok((record, events))
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
        insert_thread(&mut inner, new, None, first)
    }

    async fn fork_thread(
        &self,
        new: NewThreadRecord,
        origin: ForkOrigin,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        let mut inner = self.lock();
        if let Some(fault) = inner.create_faults.pop_front() {
            return Err(fault);
        }
        insert_thread(&mut inner, new, Some(origin), first)
    }

    async fn fork_family(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> Result<Vec<ForkNode>, StoreError> {
        let inner = self.lock();
        let mine = |id: &ThreadId| inner.threads.get(id).filter(|e| &e.record.owner == owner);
        // A link to follow: an edit whose parent still exists.
        let edit_parent = |e: &ThreadEntry| {
            e.record
                .forked_from
                .filter(|f| f.kind == ForkKind::Edit)
                .and_then(|f| f.thread_id.filter(|p| mine(p).is_some()))
        };
        let Some(mut root) = mine(&thread) else {
            return Ok(Vec::new());
        };
        for _ in 0..inner.threads.len() {
            match edit_parent(root).and_then(|p| mine(&p)) {
                Some(parent) => root = parent,
                None => break,
            }
        }
        let mut family = vec![root.record.id];
        let mut next = 0;
        while next < family.len() {
            let parent = family[next];
            next += 1;
            for e in inner.threads.values() {
                if &e.record.owner == owner
                    && edit_parent(e) == Some(parent)
                    && !family.contains(&e.record.id)
                {
                    family.push(e.record.id);
                }
            }
        }
        let mut nodes: Vec<ForkNode> = family
            .iter()
            .filter_map(mine)
            .map(|e| ForkNode {
                id: e.record.id,
                link: edit_parent(e)
                    .zip(e.record.forked_from)
                    .map(|(parent, from)| {
                        let message = e
                            .events
                            .iter()
                            .map(|s| &s.event)
                            .find(|ev| ev.seq > from.seq && ev.kind() == EventKind::UserMessage)
                            .map_or(from.seq + 2, |ev| ev.seq);
                        EditLink {
                            parent,
                            cut: from.seq,
                            message,
                        }
                    }),
                created: e.record.created_at,
            })
            .collect();
        nodes.sort_by_key(|n| (n.created, n.id));
        nodes.truncate(1000);
        Ok(nodes)
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
        include_edits: bool,
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
            .filter(|r| include_edits || r.forked_from.is_none_or(|f| f.kind != ForkKind::Edit))
            .filter(|r| before.is_none_or(|b| r.id < b))
            .collect();
        all.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(all.into_iter().take(limit as usize).cloned().collect())
    }

    async fn list_all_threads(
        &self,
        before: Option<ThreadId>,
        limit: u32,
        include_edits: bool,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        let inner = self.lock();
        if let Some(cursor) = before
            && !inner.threads.contains_key(&cursor)
        {
            return Ok(Vec::new());
        }
        let mut all: Vec<&ThreadRecord> = inner
            .threads
            .values()
            .map(|e| &e.record)
            .filter(|r| include_edits || r.forked_from.is_none_or(|f| f.kind != ForkKind::Edit))
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
            inner.fenced_commits += 1;
            return Ok(CommitOutcome::Fenced);
        }
        if let Some(lease) = &commit.inbox
            && !inner.inbox.iter().any(|r| inbox_holds(r, lease))
        {
            inner.fenced_commits += 1;
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
        if commit.only_finishes_inbox() && entry.record.state == commit.new_state {
            // Nothing to write to the thread: the row is finished and the thread left alone.
            let record = entry.record.clone();
            if let Some(lease) = &commit.inbox
                && let Some(row) = held_inbox(&mut inner, lease)
            {
                row.status = InboxStatus::Applied;
                row.lease_owner = None;
                row.lease_until = None;
            }
            return Ok(CommitOutcome::Applied {
                thread: record,
                events: Vec::new(),
            });
        }
        let (thread, events) =
            write_commit(&mut inner, thread, commit).ok_or(StoreError::NotFound)?;
        Ok(CommitOutcome::Applied { thread, events })
    }

    async fn ui_catalog_event(
        &self,
        thread: ThreadId,
        digest: &str,
    ) -> Result<Option<Event>, StoreError> {
        let inner = self.lock();
        Ok(inner.threads.get(&thread).and_then(|e| {
            e.events
                .iter()
                .rev()
                .map(|s| &s.event)
                .find(|event| {
                    matches!(&event.body, EventBody::UiCatalog(data) if data.digest == digest)
                })
                .cloned()
        }))
    }

    async fn latest_events(
        &self,
        thread: ThreadId,
        kind: EventKind,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .threads
            .get(&thread)
            .map(|e| {
                e.events
                    .iter()
                    .rev()
                    .filter(|s| s.event.kind() == kind)
                    .take(limit as usize)
                    .map(|s| s.event.clone())
                    .collect()
            })
            .unwrap_or_default())
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
                OutboxKind::Cancel
                | OutboxKind::Verify
                | OutboxKind::Title
                | OutboxKind::Description => false,
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

    async fn mark_verify_sent(
        &self,
        lease: &Lease,
        task_id: String,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        Ok(leased(&mut inner, lease)
            .map(|row| {
                row.sent_at = Some(now);
                row.task_id = Some(task_id);
            })
            .is_some())
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
            .map(|r| finish_row(r, outcome))
            .is_some())
    }

    async fn skip_unsent_delegates(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        let mut inner = self.lock();
        Ok(skip_unsent(&mut inner, thread, now))
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

    async fn receive(&self, row: NewInbox, now: Timestamp) -> Result<Received, StoreError> {
        let mut inner = self.lock();
        if inner
            .inbox
            .iter()
            .any(|r| r.source == row.source && r.idempotency_key == row.idempotency_key)
        {
            return Ok(Received::Duplicate);
        }
        let id = row.id;
        inner.inbox.push(new_row(row, now, now));
        Ok(Received::Stored { id })
    }

    async fn claim_inbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> Result<Vec<InboxItem>, StoreError> {
        let mut inner = self.lock();
        let mut due: Vec<usize> = (0..inner.inbox.len())
            .filter(|&i| {
                let row = &inner.inbox[i];
                match row.status {
                    InboxStatus::Pending => row.available_at <= now,
                    InboxStatus::Inflight => row.lease_until.is_some_and(|until| until <= now),
                    InboxStatus::Parked
                    | InboxStatus::Applied
                    | InboxStatus::Expired
                    | InboxStatus::Dead => false,
                }
            })
            .collect();
        due.sort_by_key(|&i| (inner.inbox[i].available_at, inner.inbox[i].id));
        let mut claimed = Vec::new();
        for i in due.into_iter().take(limit as usize) {
            let row = &mut inner.inbox[i];
            row.status = InboxStatus::Inflight;
            row.lease_owner = Some(owner.to_owned());
            row.lease_until = Some(add(now, lease));
            row.attempts += 1;
            claimed.push(row.clone());
        }
        Ok(claimed)
    }

    async fn park_inbox(&self, lease: &InboxLease, now: Timestamp) -> Result<Parking, StoreError> {
        let mut inner = self.lock();
        let Some(index) = inner.inbox.iter().position(|r| inbox_holds(r, lease)) else {
            return Ok(Parking::Lost);
        };
        let watched = inner.inbox[index]
            .correlation
            .as_ref()
            .is_some_and(|key| inner.watches.contains_key(key));
        let row = &mut inner.inbox[index];
        row.lease_owner = None;
        row.lease_until = None;
        // Either way the claim ends without a failure: it does not count against the limit.
        row.refunded = row.attempts;
        if watched {
            row.status = InboxStatus::Pending;
            row.available_at = now;
            Ok(Parking::Rearmed)
        } else {
            row.status = InboxStatus::Parked;
            row.parked_at = Some(now);
            Ok(Parking::Parked)
        }
    }

    async fn retry_inbox(
        &self,
        lease: &InboxLease,
        available_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        Ok(held_inbox(&mut inner, lease)
            .map(|r| {
                r.status = InboxStatus::Pending;
                r.available_at = available_at;
                r.lease_owner = None;
                r.lease_until = None;
                r.last_error = Some(error);
            })
            .is_some())
    }

    async fn complete_inbox(
        &self,
        lease: &InboxLease,
        outcome: InboxFinal,
        _now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        Ok(held_inbox(&mut inner, lease)
            .map(|r| {
                match outcome {
                    InboxFinal::Applied => r.status = InboxStatus::Applied,
                    InboxFinal::Dead { error } => {
                        r.status = InboxStatus::Dead;
                        r.last_error = Some(error);
                    }
                }
                r.lease_owner = None;
                r.lease_until = None;
            })
            .is_some())
    }

    async fn expire_parked_inbox(
        &self,
        parked_at_or_before: Timestamp,
        _now: Timestamp,
    ) -> Result<u32, StoreError> {
        let mut inner = self.lock();
        let mut n = 0;
        for row in inner.inbox.iter_mut().filter(|r| {
            r.status == InboxStatus::Parked
                && r.parked_at.is_some_and(|at| at <= parked_at_or_before)
        }) {
            row.status = InboxStatus::Expired;
            n += 1;
        }
        Ok(n)
    }

    async fn release_inbox_leases(&self, owner: &str, now: Timestamp) -> Result<u32, StoreError> {
        let mut inner = self.lock();
        let mut n = 0;
        for row in inner.inbox.iter_mut().filter(|r| {
            r.status == InboxStatus::Inflight && r.lease_owner.as_deref() == Some(owner)
        }) {
            row.lease_until = Some(now);
            // Handed back, not failed: this claim does not count against the attempt limit.
            row.refunded += 1;
            n += 1;
        }
        Ok(n)
    }

    async fn get_watch(&self, key: &str) -> Result<Option<ThreadId>, StoreError> {
        Ok(self.lock().watches.get(key).copied())
    }

    async fn get_inbox(&self, id: InboxId) -> Result<Option<InboxItem>, StoreError> {
        Ok(self.lock().inbox.iter().find(|r| r.id == id).cloned())
    }

    async fn find_inbox(
        &self,
        source: &str,
        idempotency_key: &str,
    ) -> Result<Option<InboxItem>, StoreError> {
        Ok(self
            .lock()
            .inbox
            .iter()
            .find(|r| r.source == source && r.idempotency_key == idempotency_key)
            .cloned())
    }
}
