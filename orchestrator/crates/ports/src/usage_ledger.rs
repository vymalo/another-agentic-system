use std::future::Future;

use orch_core::{ThreadId, UsageMonth, UsageMonthTotals, UsageRow, UserId};

use crate::store::StoreError;

/// What one page of [`UsageLedger::backfill`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageBackfill {
    /// Threads whose log held usage events the ledger had not folded in, and which it folded in.
    pub threads: u32,
    /// Usage events folded in.
    pub events: u64,
    /// Threads left alone because somebody held their lock for longer than the page waited for it.
    /// They are not gone through: run the backfill again (the commit that held the lock, or the next
    /// one, catches the thread up as well).
    pub skipped: u32,
    /// Rows of the ledger made to name no thread because the thread they named no longer exists: an
    /// older build erased it without clearing the rows. Only the first page (`after` of `None`)
    /// looks for them.
    pub detached: u64,
    /// Where the next page starts (pass it as `after`); `None` when the page was the last one.
    pub next: Option<ThreadId>,
}

/// The usage ledger of the people (ADR 0065): what each person used, counted per task, kept per UTC
/// month, and outliving the threads it came from.
///
/// **Every [`ThreadStore`](crate::ThreadStore) is one** (a supertrait: a store that did not
/// implement it would not compile), so the application reaches the ledger through the store of its
/// ports.
///
/// The ledger is **written by the [`ThreadStore`](crate::ThreadStore) that holds the log, in the
/// transaction that appends the event**: [`usage_change`](orch_core::usage_change) says what an
/// event changes and [`UsageRow::fold`] what the row becomes, and the store applies both before it
/// commits, so a commit that is refused (a version conflict, a lost claim, a duplicate key) or that
/// fails writes neither, and one that succeeds writes both. There is no method to write a row
/// here: an implementation of this trait is the same store the log is in, read.
///
/// Erasing a thread ([`ThreadStore::delete_threads`](crate::ThreadStore::delete_threads)) keeps its
/// rows and forgets which thread they came from, in the same transaction. A fork does not count the
/// events it copied from its parent: their usage was counted in the parent.
///
/// Not the job's `orch_core::UsageLedger`, the count of call reports a job logged.
pub trait UsageLedger: Send + Sync + 'static {
    /// What `owner` used in `month`: the sums of their rows of that month, per provider and model
    /// and per sandbox shape ([`UsageMonthTotals`]). A person who used nothing gets
    /// [`UsageMonthTotals::empty`]. Another person's rows are never in it.
    fn month(
        &self,
        owner: &UserId,
        month: UsageMonth,
    ) -> impl Future<Output = Result<UsageMonthTotals, StoreError>> + Send;

    /// The rows of one thread of `owner`, ordered by agent and task: the person's breakdown of one
    /// conversation. Empty for a thread that is not the owner's, that does not exist, or that was
    /// erased (its rows no longer name it).
    fn thread_rows(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> impl Future<Output = Result<Vec<UsageRow>, StoreError>> + Send;

    /// Folds into the ledger the usage events that threads logged before it existed or while a
    /// build without it wrote to the log, for at most `limit` threads with an id above `after`
    /// (`None`: from the start), in id order. One thread is one transaction, under the lock of the
    /// thread, so it can run while the service does; the events it folds are the ones the ledger has
    /// not folded, so **running it again changes nothing** and a thread the service has written to
    /// since is caught up as the next commit would catch it up. Events a fork copied from its parent
    /// are not folded.
    ///
    /// The first page (`after` of `None`) also detaches the rows of threads that no longer exist, which
    /// an older build left naming them ([`UsageBackfill::detached`]). A store that can wait for a
    /// thread's lock does not wait for long: a thread it cannot lock is skipped and counted
    /// ([`UsageBackfill::skipped`]), and a later call folds it in.
    ///
    /// Call it with the `next` of the page before until `next` is `None`.
    fn backfill(
        &self,
        after: Option<ThreadId>,
        limit: u32,
    ) -> impl Future<Output = Result<UsageBackfill, StoreError>> + Send;

    /// The retention sweep (ADR 0065, decision 8): removes the tasks of **every person** whose month is
    /// strictly **before** `before`, with their lines and sandbox entries, whether their thread still
    /// exists or was erased. At most `limit` tasks (a `limit` of 0 is 1), the oldest months first, so a
    /// worker can loop in batches without holding a long lock. Returns how many tasks it removed: fewer
    /// than `limit` means nothing old is left, and calling it again **changes nothing**.
    ///
    /// `before` is the first month to keep. It does not touch the log or any thread's watermark
    /// (`usage_through`), so usage that was swept is not folded in again; a task that continues after its
    /// row was swept starts a row of its own from the events that come after.
    fn sweep(
        &self,
        before: UsageMonth,
        limit: u32,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
}
