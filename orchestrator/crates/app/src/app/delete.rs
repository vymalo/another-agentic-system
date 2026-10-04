//! Deleting a thread erases it (ADR 0043): what its owner does, the purge of its files, and what a
//! worker does when it meets a thread that is gone.
//!
//! [`App::delete_thread`] deletes the thread **and every thread made from it by an edit**, in one
//! store transaction ([`ThreadStore::delete_threads`]): the log, the outbox, the binding, the
//! watches and the share go with the rows, the thread's timers with them, and one purge row is
//! written per thread. Then it erases the files ([`ArtifactStore::delete_prefix`]) and finishes the
//! purge row; when that fails the row stays and [`PurgeWorker`](crate::PurgeWorker) finishes it. The
//! log goes before the files, so nothing can reference a file that is missing, and a crash
//! between the two leaves files that a purge row finishes.
//!
//! A thread that works is refused ([`deletable`], `thread_active`): the delete would cascade the
//! unsent `cancel` row away and leave the remote task working. A late input for a thread that is
//! gone ([`App::late_input_dropped`]) is dropped, with a log line and a counter, and never retried.

use std::sync::atomic::{AtomicU64, Ordering};

use orch_core::{Classify, ThreadId, ThreadRecord, UserId, deletable, report};
use orch_ports::{ArtifactError, ArtifactStore, Clock, Ports, StoreError, ThreadStore, Topic};

use super::App;
use crate::{AppError, Denied, Permission, Requester, Resource};

/// Where a late input for a deleted thread was dropped, for the counter
/// `late_input_dropped_total{source}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateSource {
    /// A result the dispatcher had for the thread: an agent's reply, a verdict, a title.
    Dispatcher,
    /// A timer or a CI report the inbox worker had for the thread.
    Inbox,
}

impl LateSource {
    /// The label of the counter.
    pub const fn as_str(self) -> &'static str {
        match self {
            LateSource::Dispatcher => "dispatcher",
            LateSource::Inbox => "inbox",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct DeleteCounters {
    deleted: AtomicU64,
    late_dispatcher: AtomicU64,
    late_inbox: AtomicU64,
}

impl DeleteCounters {
    pub(crate) fn snapshot(&self) -> DeleteStats {
        DeleteStats {
            threads_deleted: self.deleted.load(Ordering::Relaxed),
            late_input_dropped: [
                (
                    LateSource::Dispatcher,
                    self.late_dispatcher.load(Ordering::Relaxed),
                ),
                (LateSource::Inbox, self.late_inbox.load(Ordering::Relaxed)),
            ],
        }
    }
}

/// The counters of deleting in this process (ADR 0043, decision 7): how many threads were
/// deleted, and how many late inputs for a deleted thread were dropped, by where. They count what
/// happened and name no thread and no person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteStats {
    /// `threads_deleted_total`: threads erased by [`App::delete_thread`], edits included.
    pub threads_deleted: u64,
    /// `late_input_dropped_total{source}`.
    pub late_input_dropped: [(LateSource, u64); 2],
}

impl<P: Ports> App<P> {
    /// The counters of deleting in this process.
    pub fn delete_stats(&self) -> DeleteStats {
        self.delete_counters.snapshot()
    }

    /// How many purge rows there are (threads deleted whose files are not yet erased): the gauge
    /// `thread_purges_pending`, over every replica's rows, whichever process answers.
    ///
    /// # Errors
    /// [`AppError::Store`] when the store cannot be read.
    pub async fn purges_pending(&self) -> Result<u64, AppError> {
        Ok(self.ports.store().purges_pending().await?)
    }

    /// Deletes one of the person's threads, **and every thread made from it by an edit**,
    /// transitively: they are branches of the conversation that the list does not show, and what
    /// is left of an erased conversation is not erased. Forks are kept: they are conversations of
    /// their own, standing alone from now on. The threads nested under the deleted one take its
    /// place in the person's list.
    ///
    /// The log is deleted in one transaction, then the files, which are gone when this returns
    /// unless the artifact store failed: the purge row stays and the purge sweep erases them (see
    /// the module documentation). Open streams of the thread end ([`Topic::Thread`] is notified)
    /// and a link to it is a 404 from the commit on, because the nonce went with the row.
    ///
    /// Needs `thread.delete`, **not** `thread.write`: a person who may only read may still erase
    /// their own data.
    ///
    /// # Errors
    /// [`AppError::Forbidden`] when the person's roles do not hold `thread.delete`;
    /// [`AppError::NotFound`] for a thread that is not theirs, does not exist or was deleted
    /// already; [`AppError::ThreadActive`] while the thread, or an edit of it, is `queued`,
    /// `working` or `verifying` or has a running ask (stop it, wait for it to end, delete it);
    /// [`AppError::Contended`] when the thread kept changing under the delete.
    pub async fn delete_thread(&self, who: &impl Requester, id: ThreadId) -> Result<(), AppError> {
        let access = self.access(who);
        // The permission first, so that what is refused for a missing permission is refused for
        // every id alike.
        if !access.has(Permission::ThreadDelete) {
            return Err(AppError::missing_permission(Permission::ThreadDelete));
        }
        let user = who.user();
        let root = self
            .ports
            .store()
            .get_thread(None, id)
            .await?
            .ok_or(AppError::NotFound)?;
        match access.check(
            Permission::ThreadDelete,
            &Resource::Thread { owner: &root.owner },
        ) {
            Ok(()) => {}
            Err(Denied::Permission(p)) => return Err(AppError::missing_permission(p)),
            Err(Denied::OutOfScope(_)) => return Err(AppError::NotFound),
        }
        let deleted = self.erase(user, id).await?;
        // The files, now that nothing can reference them. A failure is the sweep's, never the
        // person's: the thread is gone, and the answer says so.
        for thread in &deleted {
            self.purge_files(*thread).await;
        }
        self.delete_counters
            .deleted
            .fetch_add(deleted.len() as u64, Ordering::Relaxed);
        for thread in &deleted {
            self.notify(Topic::Thread(*thread)).await;
        }
        Ok(())
    }

    /// The transaction of [`delete_thread`](Self::delete_thread), decided again on what each
    /// attempt reads: the thread and its edit descendants, each at the version it was read at.
    /// Returns the threads that were deleted.
    async fn erase(&self, user: &UserId, id: ThreadId) -> Result<Vec<ThreadId>, AppError> {
        let store = self.ports.store();
        for _ in 0..self.cfg.max_commit_attempts {
            let mut threads: Vec<ThreadRecord> = Vec::new();
            for thread in self.edit_family(user, id).await? {
                match store.get_thread(Some(user), thread).await? {
                    Some(record) => threads.push(record),
                    // a branch that went since the family was read has nothing left to delete
                    None if thread != id => {}
                    None => return Err(AppError::NotFound),
                }
            }
            for thread in &threads {
                deletable(thread.state, &thread.job).map_err(AppError::ThreadActive)?;
            }
            let versions: Vec<(ThreadId, i64)> =
                threads.iter().map(|t| (t.id, t.version)).collect();
            let now = self.ports.clock().now();
            match store.delete_threads(user, &versions, now).await {
                Ok(()) => return Ok(versions.into_iter().map(|(t, _)| t).collect()),
                Err(StoreError::NotFound) => return Err(AppError::NotFound),
                // changed since it was read, or an edit was made since the family was: read again
                Err(StoreError::VersionConflict) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::Contended)
    }

    /// `id` and every thread made from it by an edit, transitively (the family of edits it belongs
    /// to, from `id` down).
    async fn edit_family(&self, user: &UserId, id: ThreadId) -> Result<Vec<ThreadId>, AppError> {
        let family = self.ports.store().fork_family(user, id).await?;
        let mut doomed = vec![id];
        loop {
            let before = doomed.len();
            for node in &family {
                if let Some(link) = node.link
                    && doomed.contains(&link.parent)
                    && !doomed.contains(&node.id)
                {
                    doomed.push(node.id);
                }
            }
            if doomed.len() == before {
                return Ok(doomed);
            }
        }
    }

    /// Erases the files of a deleted thread and finishes its purge row. `true` when the row is
    /// finished (it is removed); `false` when the artifact store or the row could not be reached,
    /// and the row is left for the sweep. A deployment with no artifact store has nothing to erase.
    /// Idempotent, so the inline purge of a delete and the sweep may both run.
    pub(crate) async fn purge_files(&self, thread: ThreadId) -> bool {
        match self.ports.artifacts().delete_prefix(thread).await {
            Ok(files) => {
                tracing::debug!(%thread, files, "the files of a deleted thread were erased")
            }
            // no store, no file: there was never anything to erase
            Err(ArtifactError::NotConfigured) => {}
            Err(e) => {
                tracing::warn!(%thread, error = %report(&e), class = ?e.class(),
                    "the files of a deleted thread could not be erased; the purge sweep will retry");
                return false;
            }
        }
        match self.ports.store().finish_purge(thread).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(%thread, error = %report(&e),
                    "the purge of a deleted thread could not be finished; the sweep will do it");
                false
            }
        }
    }

    /// A worker met a thread that is gone (it was deleted while the worker held a result or a row
    /// for it): the input is dropped, once, with a log line and the counter
    /// `late_input_dropped_total`. Never retried, never an error to alert on.
    pub(crate) fn late_input_dropped(&self, source: LateSource, thread: ThreadId) {
        let counter = match source {
            LateSource::Dispatcher => &self.delete_counters.late_dispatcher,
            LateSource::Inbox => &self.delete_counters.late_inbox,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        tracing::info!(%thread, source = source.as_str(), "the thread was deleted; the late input was dropped");
    }
}
