//! The connect stream as a pure fold: which frames a client that attaches to a thread gets, and
//! when its stream ends (`docs/api/agui.md`, "Connect binding").
//!
//! [`Connect`] sits on top of [`Projector`] and knows about the client's *cursor* (the last log
//! sequence number it holds, 0 for none) and about how long the stream should last. The surface
//! reads the log from the start, hands every event to [`Connect::feed`], and writes what comes
//! back. Events up to the cursor are folded and not written, so the projector's state at the
//! cursor is a function of the log alone and every replica agrees; then the preamble re-opens
//! the run that is open there, and every later event is written. Nothing here reads a clock or a
//! store, and nothing is remembered between connections: a reconnect builds a new [`Connect`]
//! from the log and the cursor.
//!
//! ```text
//! frames(cursor c) = preamble(state after events <= c) ++ frames(events > c)
//! ```
//!
//! The preamble is empty when no run is open at `c`, and the frames of the events after `c` are
//! exactly those an uninterrupted stream would have written after its frame with `id: c`.

use orch_core::Event;

use crate::frame::{Audience, Frame};
use crate::projector::{Projector, ThreadMeta};

/// How long a connect stream lasts (`?mode=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Follow {
    /// Until the client closes: replay, then every later run. Between runs the stream is idle.
    #[default]
    Forever,
    /// `?mode=run`: the replay, and then, if a run is open at the end of it, that run to its
    /// terminal event; the stream ends at the first moment nothing is open, once the thread's log
    /// as it stood when the client connected has been replayed.
    ThroughRun,
}

/// The state of one connect stream.
#[derive(Debug, Clone)]
pub struct Connect {
    projector: Projector,
    cursor: i64,
    head: i64,
    follow: Follow,
    /// Frames are written from here on: the cursor has been reached.
    started: bool,
    /// The last sequence number folded.
    folded: i64,
}

impl Connect {
    /// A stream for the thread `meta` describes, for a client that holds the log up to `cursor`.
    /// `head` is the thread's last sequence number when the client connected; a cursor beyond it
    /// (stale or forged) counts as `head`, so the client waits for new events.
    pub fn new(meta: ThreadMeta, cursor: i64, head: i64, follow: Follow) -> Self {
        let head = head.max(0);
        let cursor = cursor.clamp(0, head);
        Connect {
            projector: Projector::new(meta),
            cursor,
            head,
            follow,
            // With nothing held there is nothing to skip and nothing to re-open.
            started: cursor == 0,
            folded: 0,
        }
    }

    /// The cursor after clamping: events up to and including it are not written.
    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    /// Folds the next log event (they arrive in `seq` order, from the first) and returns the
    /// frames to write, if any. The first call that reaches the cursor also returns the
    /// preamble, ahead of what the event itself says when it comes after the cursor.
    pub fn feed(&mut self, event: &Event) -> Vec<Frame> {
        let mut out = Vec::new();
        if !self.started && event.seq > self.cursor {
            // The log has no event at the cursor itself; start from what was folded.
            self.started = true;
            out.extend(self.projector.resume_preamble());
        }
        let frames = self.projector.apply(event, Audience::Viewer);
        if self.started {
            out.extend(frames);
        }
        self.folded = self.folded.max(event.seq);
        if !self.started && event.seq >= self.cursor {
            self.started = true;
            out.extend(self.projector.resume_preamble());
        }
        out
    }

    /// Whether the stream has said all it will say: [`Follow::ThroughRun`], the log as it stood
    /// at connect time replayed, and no run open.
    pub fn finished(&self) -> bool {
        self.follow == Follow::ThroughRun
            && self.started
            && self.folded >= self.head
            && !self.projector.run_open()
    }

    /// The projection so far, for the surface's diagnostics.
    pub fn projector(&self) -> &Projector {
        &self.projector
    }
}
