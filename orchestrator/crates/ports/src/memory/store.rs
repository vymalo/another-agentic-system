use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    EditLink, Event, EventBody, EventKind, ForkKind, ForkNode, ForkedFrom, Job, NONCE_LEN,
    RankError, ThreadId, ThreadRecord, ThreadShare, UserId, between, spread,
};

use crate::{
    AgentBinding, ArchivedFilter, Arrangement, BindingUpdate, Commit, CommitOutcome, ForkOrigin,
    InboxFinal, InboxId, InboxItem, InboxLease, InboxStatus, Lease, ListOrder, NewInbox,
    NewThreadRecord, OutboxFinal, OutboxId, OutboxItem, OutboxKind, OutboxStats, OutboxStatus,
    Parking, Place, Received, SharingChange, StoreError, TIMER_SOURCE, ThreadListing, ThreadStore,
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
    /// Commits that win the race against the next `commit`s, one each (fault injection).
    interlopers: std::collections::VecDeque<(ThreadId, Commit)>,
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

    /// Before the next `commit`, `commit` is applied to `thread` at its version then, as by a
    /// concurrent writer that won the race: the commit that follows was decided on what it read
    /// before, so it conflicts when it is of the same thread and is decided again. What a test
    /// uses to put another writer between a read and a write. A running dispatcher commits too
    /// and would consume it: use it where nothing else writes.
    pub fn interleave_next_commit(&self, thread: ThreadId, commit: Commit) {
        self.lock().interlopers.push_back((thread, commit));
    }

    /// What [`ThreadStore::commit`] does, after the commits that were made to win its race.
    fn commit_now(
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
        if let Some(SharingChange::Set { nonce, .. }) = &commit.sharing
            && inner.threads.values().any(|e| {
                e.record.id != thread && e.record.share.as_ref().is_some_and(|s| s.nonce == *nonce)
            })
        {
            // The unique index of the Postgres store: a capability is one thread's.
            return Err(StoreError::corrupt("share nonce already in use"));
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

/// Finishes the thread's unsent `delegate` and `steer` rows as `skipped`: those `pending`, and
/// those `inflight` whose lease expired before `now`. Returns the count.
fn skip_unsent(inner: &mut Inner, thread: ThreadId, now: Timestamp) -> u32 {
    let mut n = 0;
    for row in inner.outbox.iter_mut().filter(|r| {
        r.thread_id == thread
            && matches!(r.kind, OutboxKind::Delegate | OutboxKind::Steer)
            && r.sent_at.is_none()
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
    match commit.sharing {
        Some(SharingChange::Set { level, nonce }) => {
            entry.record.share = Some(ThreadShare {
                level,
                nonce,
                shared_at: commit.now,
            });
        }
        Some(SharingChange::Clear) => entry.record.share = None,
        None => {}
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
        // A new thread, a fork included, is private: its share is nobody's but its own.
        sharing: None,
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
    if let Some(parent) = new.rail_parent {
        let top_level = inner
            .threads
            .get(&parent)
            .is_some_and(|e| e.record.owner == new.owner && e.record.rail_parent.is_none());
        if !top_level {
            return Err(StoreError::corrupt(
                "a thread is nested under a top-level thread of its owner's",
            ));
        }
    }
    let hidden = fork.is_some_and(|o| o.kind == ForkKind::Edit);
    let rail_rank = if new.rail_parent.is_some() || hidden {
        // a row the list does not rank: it takes the rank of the first, and burns no key
        lowest_rank(inner, &new.owner, None).unwrap_or_else(|| "i".to_owned())
    } else {
        new_rank(inner, &new.owner, new.id, Place::Top)?
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
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: new.rail_parent,
        rail_rank,
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

/// The owner's top-level rows, whatever they are (hidden ones too), as ranks are kept for all.
fn roots<'a>(inner: &'a Inner, owner: &'a UserId) -> impl Iterator<Item = &'a ThreadRecord> {
    inner
        .threads
        .values()
        .map(|e| &e.record)
        .filter(move |r| &r.owner == owner && r.rail_parent.is_none())
}

/// The lowest rank of the owner's top-level rows, leaving `except` out.
fn lowest_rank(inner: &Inner, owner: &UserId, except: Option<ThreadId>) -> Option<String> {
    roots(inner, owner)
        .filter(|r| Some(r.id) != except)
        .map(|r| &r.rail_rank)
        .min_by(|a, b| a.as_bytes().cmp(b.as_bytes()))
        .cloned()
}

/// Order of two rows of one section: by rank, ties newest first.
fn by_rank(a: &ThreadRecord, b: &ThreadRecord) -> std::cmp::Ordering {
    a.rail_rank
        .as_bytes()
        .cmp(b.rail_rank.as_bytes())
        .then_with(|| b.id.cmp(&a.id))
}

fn is_edit(r: &ThreadRecord) -> bool {
    r.forked_from.is_some_and(|f| f.kind == ForkKind::Edit)
}

/// The rows of one section of the owner's list in order: top-level, listed, not archived, pinned
/// or not as asked. `except` is the row being moved, which is nobody's neighbour.
fn section<'a>(
    inner: &'a Inner,
    owner: &'a UserId,
    pinned: bool,
    except: Option<ThreadId>,
) -> Vec<&'a ThreadRecord> {
    let mut rows: Vec<&ThreadRecord> = roots(inner, owner)
        .filter(|r| {
            !is_edit(r)
                && r.archived_at.is_none()
                && r.pinned_at.is_some() == pinned
                && Some(r.id) != except
        })
        .collect();
    rows.sort_by(|a, b| by_rank(a, b));
    rows
}

/// Writes the owner's top-level ranks again, evenly spread, in the order they have now.
fn respread(inner: &mut Inner, owner: &UserId) {
    let mut rows: Vec<(String, ThreadId)> = roots(inner, owner)
        .map(|r| (r.rail_rank.clone(), r.id))
        .collect();
    rows.sort_by(|a, b| {
        a.0.as_bytes()
            .cmp(b.0.as_bytes())
            .then_with(|| b.1.cmp(&a.1))
    });
    for ((_, id), rank) in rows.iter().zip(spread(rows.len())) {
        if let Some(entry) = inner.threads.get_mut(id) {
            entry.record.rail_rank = rank;
        }
    }
}

/// The key of the slot `place` names for `moving` among the owner's rows. Where no key fits (two
/// neighbours of one rank, or the cap), the owner's ranks are re-spread first.
fn new_rank(
    inner: &mut Inner,
    owner: &UserId,
    moving: ThreadId,
    place: Place,
) -> Result<String, StoreError> {
    for attempt in 0..2 {
        let (lower, upper) = match place {
            Place::Top => (None, lowest_rank(inner, owner, Some(moving))),
            Place::Before(anchor) | Place::After(anchor) => {
                let pinned = inner
                    .threads
                    .get(&anchor)
                    .is_some_and(|e| e.record.pinned_at.is_some());
                let rows = section(inner, owner, pinned, Some(moving));
                let at = rows
                    .iter()
                    .position(|r| r.id == anchor)
                    .ok_or(StoreError::Refused("bad_anchor"))?;
                let rank =
                    |i: Option<usize>| i.and_then(|i| rows.get(i)).map(|r| r.rail_rank.clone());
                if matches!(place, Place::Before(_)) {
                    (rank(at.checked_sub(1)), rank(Some(at)))
                } else {
                    (rank(Some(at)), rank(Some(at + 1)))
                }
            }
        };
        match between(lower.as_deref(), upper.as_deref()) {
            Ok(key) => return Ok(key),
            Err(RankError::Order | RankError::TooLong) if attempt == 0 => respread(inner, owner),
            Err(e) => return Err(StoreError::corrupt(e.to_string())),
        }
    }
    Err(StoreError::corrupt("no rank fits after a re-spread"))
}

/// [`ThreadStore::arrange_thread`] on the data.
fn arrange(
    inner: &mut Inner,
    owner: &UserId,
    id: ThreadId,
    change: Arrangement,
    now: Timestamp,
) -> Result<ThreadRecord, StoreError> {
    let rec = inner
        .threads
        .get(&id)
        .filter(|e| &e.record.owner == owner)
        .map(|e| e.record.clone())
        .ok_or(StoreError::NotFound)?;
    let nested = rec.rail_parent.is_some();
    let unnest = change.unnest && nested;
    if nested && !unnest && (change.pinned == Some(true) || change.place.is_some()) {
        return Err(StoreError::Refused("nested_row"));
    }
    if let Some(Place::Before(a) | Place::After(a)) = change.place {
        let usable = a != id
            && inner.threads.get(&a).is_some_and(|e| {
                &e.record.owner == owner
                    && e.record.rail_parent.is_none()
                    && e.record.archived_at.is_none()
                    && !is_edit(&e.record)
            });
        if !usable {
            return Err(StoreError::Refused("bad_anchor"));
        }
    }
    let want_pinned = change.pinned.unwrap_or(rec.pinned_at.is_some());
    let want_archived = change.archived.unwrap_or(rec.archived_at.is_some());
    let pin_changed = want_pinned != rec.pinned_at.is_some();
    let archive_changed = want_archived != rec.archived_at.is_some();

    // Where the row goes: nowhere new (`None`), or a slot.
    let in_place = !pin_changed && !unnest;
    let slot = match change.place {
        Some(place) if !(in_place && already_there(inner, owner, &rec, place)) => Some(place),
        Some(_) => None,
        None if pin_changed => Some(Place::Top),
        None if unnest => Some(after_block(inner, owner, &rec)),
        None => None,
    };
    if slot.is_none() && !archive_changed && !pin_changed && !unnest {
        return Ok(rec);
    }
    let rail_rank = match slot {
        Some(place) => Some(new_rank(inner, owner, id, place)?),
        None => None,
    };
    let entry = inner.threads.get_mut(&id).ok_or(StoreError::NotFound)?;
    if pin_changed {
        entry.record.pinned_at = want_pinned.then_some(now);
    }
    if archive_changed {
        entry.record.archived_at = want_archived.then_some(now);
    }
    if unnest {
        entry.record.rail_parent = None;
    }
    if let Some(rank) = rail_rank {
        entry.record.rail_rank = rank;
    }
    Ok(entry.record.clone())
}

/// Whether `rec` is already where `place` puts it, in its own section.
fn already_there(inner: &Inner, owner: &UserId, rec: &ThreadRecord, place: Place) -> bool {
    if rec.rail_parent.is_some() || rec.archived_at.is_some() {
        return false;
    }
    let rows = section(inner, owner, rec.pinned_at.is_some(), None);
    let at = |id: ThreadId| rows.iter().position(|r| r.id == id);
    let Some(mine) = at(rec.id) else {
        return false;
    };
    match place {
        Place::Top => mine == 0,
        Place::Before(a) => at(a).is_some_and(|a| mine + 1 == a),
        Place::After(a) => at(a).is_some_and(|a| mine == a + 1),
    }
}

/// Where an ejected row goes: right after the block it left, unless that block is pinned or
/// archived, which the row is not: then on top of the unpinned ones.
fn after_block(inner: &Inner, owner: &UserId, rec: &ThreadRecord) -> Place {
    rec.rail_parent
        .and_then(|p| inner.threads.get(&p))
        .filter(|e| {
            &e.record.owner == owner
                && e.record.pinned_at.is_none()
                && e.record.archived_at.is_none()
                && !is_edit(&e.record)
        })
        .map_or(Place::Top, |e| Place::After(e.record.id))
}

/// The rows of `listing`, flat: the newest first, a row at a time.
fn list_recent(inner: &Inner, owner: &UserId, listing: &ThreadListing) -> Vec<ThreadRecord> {
    let mut all: Vec<&ThreadRecord> = inner
        .threads
        .values()
        .map(|e| &e.record)
        .filter(|r| &r.owner == owner)
        .filter(|r| listing.include_edits || !is_edit(r))
        .filter(|r| match listing.archived {
            ArchivedFilter::Exclude => r.archived_at.is_none(),
            ArchivedFilter::Only => r.archived_at.is_some(),
            ArchivedFilter::Include => true,
        })
        .filter(|r| listing.before.is_none_or(|b| r.id < b))
        .collect();
    all.sort_by(|a, b| b.id.cmp(&a.id));
    all.into_iter()
        .take(listing.limit as usize)
        .cloned()
        .collect()
}

/// The owner's list in their own order: pages of top-level rows, each with its children.
fn list_rail(inner: &Inner, owner: &UserId, listing: &ThreadListing) -> Vec<ThreadRecord> {
    let section_of = |r: &ThreadRecord| match (r.archived_at, r.pinned_at) {
        (Some(_), _) => 2,
        (None, Some(_)) => 0,
        (None, None) => 1,
    };
    let order = |a: &&ThreadRecord, b: &&ThreadRecord| {
        let (sa, sb) = (section_of(a), section_of(b));
        sa.cmp(&sb).then_with(|| {
            if sa < 2 {
                by_rank(a, b)
            } else {
                b.archived_at
                    .cmp(&a.archived_at)
                    .then_with(|| b.id.cmp(&a.id))
            }
        })
    };
    let mine: Vec<&ThreadRecord> = inner
        .threads
        .values()
        .map(|e| &e.record)
        .filter(|r| &r.owner == owner)
        .filter(|r| listing.include_edits || !is_edit(r))
        .collect();
    let archived_root = |id: ThreadId| {
        inner
            .threads
            .get(&id)
            .is_some_and(|e| e.record.archived_at.is_some())
    };
    let mut units: Vec<&ThreadRecord> = mine
        .iter()
        .copied()
        .filter(|r| match listing.archived {
            ArchivedFilter::Exclude => r.rail_parent.is_none() && r.archived_at.is_none(),
            ArchivedFilter::Include => r.rail_parent.is_none(),
            ArchivedFilter::Only => {
                r.archived_at.is_some() && r.rail_parent.is_none_or(|p| !archived_root(p))
            }
        })
        .collect();
    units.sort_by(order);
    let start = match listing.before {
        None => 0,
        Some(cursor) => match units.iter().position(|r| r.id == cursor) {
            Some(at) => at + 1,
            None => return Vec::new(),
        },
    };
    let mut out = Vec::new();
    for unit in units.into_iter().skip(start).take(listing.limit as usize) {
        out.push(unit.clone());
        let mut children: Vec<&ThreadRecord> = mine
            .iter()
            .copied()
            .filter(|r| r.rail_parent == Some(unit.id))
            .filter(|r| listing.archived != ArchivedFilter::Exclude || r.archived_at.is_none())
            .collect();
        children.sort_by(|a, b| b.id.cmp(&a.id));
        out.extend(children.into_iter().cloned());
    }
    out
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

    async fn thread_by_share_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
    ) -> Result<Option<ThreadRecord>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .threads
            .values()
            .find(|e| {
                e.record
                    .share
                    .as_ref()
                    .is_some_and(|s| s.nonce.as_bytes() == nonce)
            })
            .map(|e| e.record.clone()))
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
        listing: ThreadListing,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        let inner = self.lock();
        if listing.order == ListOrder::Recent
            && let Some(cursor) = listing.before
            && inner
                .threads
                .get(&cursor)
                .is_none_or(|e| &e.record.owner != owner)
        {
            return Ok(Vec::new());
        }
        Ok(match listing.order {
            ListOrder::Recent => list_recent(&inner, owner, &listing),
            ListOrder::Rail => list_rail(&inner, owner, &listing),
        })
    }

    async fn arrange_thread(
        &self,
        owner: &UserId,
        id: ThreadId,
        change: Arrangement,
        now: Timestamp,
    ) -> Result<ThreadRecord, StoreError> {
        let mut inner = self.lock();
        arrange(&mut inner, owner, id, change, now)
    }

    async fn commit(
        &self,
        thread: ThreadId,
        expected_version: i64,
        commit: Commit,
    ) -> Result<CommitOutcome, StoreError> {
        let interloper = self.lock().interlopers.pop_front();
        if let Some((other, first)) = interloper {
            let version = self
                .lock()
                .threads
                .get(&other)
                .map(|e| e.record.version)
                .ok_or(StoreError::NotFound)?;
            self.commit_now(other, version, first)?;
        }
        self.commit_now(thread, expected_version, commit)
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
                | OutboxKind::Ask
                | OutboxKind::Title
                | OutboxKind::Description => false,
                // one lane for delegations and one for steers: a steer is for the turn the open
                // delegation is running, so it never waits behind it (ADR 0036)
                OutboxKind::Delegate | OutboxKind::Steer => inner.outbox[..i].iter().any(|older| {
                    older.thread_id == row.thread_id
                        && older.kind == row.kind
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

    async fn requeue_as_delegate(&self, lease: &Lease, now: Timestamp) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        let Some(row) = leased(&mut inner, lease) else {
            return Ok(false);
        };
        let Some(payload) = row.payload.steer_as_delegate() else {
            return Ok(false);
        };
        row.kind = OutboxKind::Delegate;
        row.payload = payload;
        row.status = OutboxStatus::Pending;
        row.next_attempt_at = now;
        row.lease_owner = None;
        row.lease_until = None;
        Ok(true)
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
