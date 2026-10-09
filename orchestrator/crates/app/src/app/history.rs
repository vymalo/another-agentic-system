//! The finite reads of a thread's log that the history routes fold (ADR 0059,
//! `docs/api/history.md`): the events from the first to the thread's last, in order, through the
//! store's paged read, and the counters of what the folds cost.
//!
//! The fold itself is `orch_agui_projection::History`, in the AG-UI surface; this is the part that
//! knows who may read the thread and where the log is.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::{Event, ThreadId, ThreadRecord};
use orch_ports::{Ports, ThreadStore};

use super::{App, SharedRead};
use crate::reader::reader_event;
use crate::{AppError, Requester};

/// Events read from the store per page when a history read folds the log.
const HISTORY_PAGE: u32 = 500;

/// The bounds of a page of history (`server.history`, ADR 0059).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistorySettings {
    /// The most turns a page holds.
    pub max_turns: usize,
    /// The most bytes of frames a page holds, as JSON; the newest chain is returned whole.
    pub max_page_bytes: usize,
}

impl Default for HistorySettings {
    fn default() -> Self {
        HistorySettings {
            max_turns: 100,
            max_page_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct HistoryCounters {
    pages: AtomicU64,
    events: AtomicU64,
    micros: AtomicU64,
}

/// What the history reads of this process cost: `history_pages_total`,
/// `history_events_folded_total` and `history_fold_seconds_total`. They name no thread and no
/// person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStats {
    /// Pages answered.
    pub pages: u64,
    /// Events the folds took in, over every page.
    pub events_folded: u64,
    /// Microseconds spent reading and folding, over every page.
    pub fold_micros: u64,
}

impl<P: Ports> App<P> {
    /// The bounds of a page of history.
    pub fn history_settings(&self) -> HistorySettings {
        self.cfg.history
    }

    /// What the history reads of this process cost so far.
    pub fn history_stats(&self) -> HistoryStats {
        let c = &self.history_counters;
        HistoryStats {
            pages: c.pages.load(Ordering::Relaxed),
            events_folded: c.events.load(Ordering::Relaxed),
            fold_micros: c.micros.load(Ordering::Relaxed),
        }
    }

    /// A page was answered: it took `events` events in and `elapsed` to read and fold them.
    pub fn history_answered(&self, events: u64, elapsed: Duration) {
        let c = &self.history_counters;
        c.pages.fetch_add(1, Ordering::Relaxed);
        c.events.fetch_add(events, Ordering::Relaxed);
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        c.micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// The thread `id` of one of the user's, and its log from the first event to the last it had
    /// when this looked: the owner's history read. Someone else's thread, or a malformed one, is
    /// [`AppError::NotFound`] like every read.
    ///
    /// # Errors
    ///
    /// [`AppError::NotFound`], [`AppError::Forbidden`] for roles without `thread.read`, and
    /// [`AppError::Store`] when the store fails; the stream yields the last two too.
    pub async fn history_log(
        self: &std::sync::Arc<Self>,
        who: &impl Requester,
        id: ThreadId,
    ) -> Result<(ThreadRecord, BoxStream<'static, Result<Event, AppError>>), AppError> {
        let thread = self.get_thread(who, id).await?;
        let log = self.log_until(thread.id, thread.last_seq, None);
        Ok((thread, log))
    }

    /// The log of a thread opened through its link, as the reader may see it (`reader_event`),
    /// from the first event to the last the thread had when it was opened. The link was checked
    /// when `read` was made; a history read is one short request, so it is not watched.
    pub fn shared_history_log(
        self: &std::sync::Arc<Self>,
        read: &SharedRead,
    ) -> BoxStream<'static, Result<Event, AppError>> {
        let thread = read.thread();
        self.log_until(thread.id, thread.last_seq, Some(*read.rules()))
    }

    /// The events of `id` up to `head`, oldest first, in pages; `rules` make each one what a
    /// reader may see. A log that ends before `head` (the thread was deleted meanwhile) is
    /// [`AppError::NotFound`].
    fn log_until(
        self: &std::sync::Arc<Self>,
        id: ThreadId,
        head: i64,
        rules: Option<crate::reader::ReaderRules>,
    ) -> BoxStream<'static, Result<Event, AppError>> {
        struct St<P: Ports> {
            app: std::sync::Arc<App<P>>,
            id: ThreadId,
            head: i64,
            cursor: i64,
            buf: VecDeque<Event>,
            rules: Option<crate::reader::ReaderRules>,
            over: bool,
        }
        let st = St {
            app: std::sync::Arc::clone(self),
            id,
            head,
            cursor: 0,
            buf: VecDeque::new(),
            rules,
            over: false,
        };
        futures::stream::unfold(st, |mut st| async move {
            loop {
                if let Some(event) = st.buf.pop_front() {
                    let event = match &st.rules {
                        Some(rules) => reader_event(&event, rules),
                        None => event,
                    };
                    return Some((Ok(event), st));
                }
                if st.over || st.cursor >= st.head {
                    return None;
                }
                match st
                    .app
                    .ports
                    .store()
                    .list_events(st.id, st.cursor, HISTORY_PAGE)
                    .await
                {
                    Ok(events) if events.is_empty() => {
                        st.over = true;
                        return Some((Err(AppError::NotFound), st));
                    }
                    Ok(events) => {
                        if let Some(last) = events.last() {
                            st.cursor = last.seq;
                        }
                        let head = st.head;
                        st.buf.extend(events.into_iter().filter(|e| e.seq <= head));
                    }
                    Err(e) => {
                        st.over = true;
                        return Some((Err(e.into()), st));
                    }
                }
            }
        })
        .boxed()
    }
}
