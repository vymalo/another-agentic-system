use std::future::Future;

use jiff::Timestamp;
use orch_core::search::{SearchRole, SearchTerms};
use orch_core::{ThreadId, ThreadRecord, UserId};

use crate::store::{ArchivedFilter, StoreError};

/// How strongly a thread matched, which orders the results before the thread's id does
/// (`docs/api/thread-search-v1.md`, "The result"): the title beats the description, which beats the
/// messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SearchRank {
    /// Only messages match.
    Messages,
    /// The description matches (and the title does not).
    Description,
    /// The title matches.
    Title,
}

impl SearchRank {
    /// The number the contract's `rank` is: 0, 1 or 2.
    pub fn number(self) -> u8 {
        match self {
            SearchRank::Messages => 0,
            SearchRank::Description => 1,
            SearchRank::Title => 2,
        }
    }

    /// The rank a number names; `None` for any other.
    pub fn from_number(n: u8) -> Option<Self> {
        match n {
            0 => Some(SearchRank::Messages),
            1 => Some(SearchRank::Description),
            2 => Some(SearchRank::Title),
            _ => None,
        }
    }
}

/// Where a page of results ended: the last result's rank and thread. The next page is the results
/// that come after it in the order (rank descending, then thread id descending).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchPosition {
    /// The rank of the last result of the page before.
    pub rank: SearchRank,
    /// Its thread.
    pub thread: ThreadId,
}

/// What [`ThreadSearch::search_threads`] is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The terms: every one a prefix of a key of one word of a document that matches. Letters and
    /// numbers only, so a store may build its query out of them and nothing else. No term: nothing
    /// matches.
    pub terms: SearchTerms,
    /// Which of the owner's threads by what is archived, as for [`ThreadListing`](crate::ThreadListing).
    pub archived: ArchivedFilter,
    /// Also search the threads made by an edit of a message (branches), as the list does with
    /// `include_edits`.
    pub include_edits: bool,
    /// Where the page before ended; `None` for the first page.
    pub after: Option<SearchPosition>,
    /// How many threads at most; the store may be asked for any number, the API asks 1 to 50.
    pub limit: u32,
}

/// A message that matched, with the words the snippet is cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedMessage {
    /// The event's `seq`.
    pub seq: i64,
    /// Who said it.
    pub role: SearchRole,
    /// When.
    pub at: Timestamp,
    /// The words.
    pub text: String,
}

/// One thread that matched.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow {
    /// The thread, as [`ThreadStore::get_thread`](crate::ThreadStore::get_thread) gives it: its title and
    /// description are the documents the title and description hits are cut from.
    pub thread: ThreadRecord,
    /// How strongly it matched.
    pub rank: SearchRank,
    /// The newest [`MAX_MESSAGE_HITS`](orch_core::search::MAX_MESSAGE_HITS) matching messages, newest
    /// first.
    pub messages: Vec<MatchedMessage>,
    /// How many messages match in all (at least `messages.len()`).
    pub message_hits: u32,
}

/// A page of results.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRows {
    /// The threads, in order: rank descending, then thread id descending.
    pub rows: Vec<SearchRow>,
    /// Whether more results follow the last row.
    pub more: bool,
}

/// What one page of [`ThreadSearch::backfill_search`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchBackfill {
    /// Threads whose log (or title) the index had not been through, and which it went through.
    pub threads: u32,
    /// Messages in the index for the threads it went through, from the point it started at: the exact
    /// number of rows it wrote that are there when it is done (a message a later one replaced is not
    /// counted).
    pub messages: u64,
    /// Threads left alone because somebody held their lock for longer than the page waited for it:
    /// run the backfill again (the commit that held the lock, or the next one, catches the thread up
    /// as well).
    pub skipped: u32,
    /// Where the next page starts (pass it as `after`); `None` when the page was the last one.
    pub next: Option<ThreadId>,
}

/// The search index of a person's threads (ADR 0068): the folded words of the titles, the
/// descriptions and the messages, **a function of the log** that the
/// [`ThreadStore`](crate::ThreadStore) writes in the transaction that appends the event and
/// erases in the transaction that erases the thread.
///
/// **Every [`ThreadStore`](crate::ThreadStore) is one** (a supertrait, as
/// [`UsageLedger`](crate::UsageLedger) is: a store that did not implement it would not compile).
/// The rules are `orch_core::search` and the contract `docs/api/thread-search-v1.md`: what is a
/// word, what is searched, when a document matches, which messages a fork shows (the events it
/// copied from a parent that still exists give none) and the order. A store is only the **index**:
/// whatever its way of finding candidates, the set it answers is the contract's.
///
/// There is no method to write the index: the store that holds the log writes it with the event.
pub trait ThreadSearch: Send + Sync + 'static {
    /// The threads of `owner` that match `request`, one page, in order (rank descending, then
    /// thread id descending), each with its newest matching messages and their number. **Only the
    /// owner's threads and the owner's words** are searched: another person's are never in the
    /// answer, by title or by content, and the answer is always a subset of
    /// [`ThreadStore::list_threads`](crate::ThreadStore::list_threads) for the same `archived` and
    /// `include_edits`. A thread deleted is not in it from the moment the delete committed. **A thread whose
    /// index is behind is found only for what is indexed**: a thread stored under another
    /// [`FOLD_VERSION`](orch_core::search::FOLD_VERSION) is not found at all until a commit or the backfill
    /// has indexed it again (never by keys made by the old rules), and one a commit has caught up only in
    /// part is found by its title, its description and the messages indexed so far.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] when the store is down or the search took too long.
    fn search_threads(
        &self,
        owner: &UserId,
        request: &SearchRequest,
    ) -> impl Future<Output = Result<SearchRows, StoreError>> + Send;

    /// Indexes the titles and messages that threads logged before the index existed, or while a
    /// build without it wrote to the log, or under an older [`FOLD_VERSION`](orch_core::search::FOLD_VERSION),
    /// or that a commit left behind (a commit catches an old log up only
    /// [`COMMIT_CATCH_UP_EVENTS`](orch_core::search::COMMIT_CATCH_UP_EVENTS) events at a time), for at
    /// most `limit` threads with an id above `after` (`None`: from the start), in id order. One thread
    /// is one transaction, under the lock of the thread, so it can run while the service does;
    /// **running it again changes nothing**.
    ///
    /// A thread whose stored fold version is not the current one is indexed **from the start,
    /// replacing its rows**. With `rebuild` every thread is, whatever it has stored.
    ///
    /// Call it with the `next` of the page before until `next` is `None`.
    fn backfill_search(
        &self,
        after: Option<ThreadId>,
        limit: u32,
        rebuild: bool,
    ) -> impl Future<Output = Result<SearchBackfill, StoreError>> + Send;

    /// How many threads the index has not been through (a log or a fold version it is behind), **counted
    /// up to `cap`**: the answer is `min(threads, cap)`. For the startup warning and for what
    /// `search backfill` says remains; it stops reading at `cap`.
    fn search_backlog(&self, cap: u32) -> impl Future<Output = Result<u32, StoreError>> + Send;
}
