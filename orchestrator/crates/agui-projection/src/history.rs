//! The history read as a pure fold: a finite page of the connect stream's frames, from the newest
//! settled chain back, or the settled chains after a point (`docs/api/history.md`, ADR 0059).
//!
//! [`History`] sits on top of [`Projector`] the way [`Connect`](crate::Connect) does. The surface
//! reads the log from the first event, hands every event to [`History::feed`] until it says it is
//! done, and takes the [`Page`] from [`History::finish`]. Nothing here reads a clock or a store.
//!
//! # Chains
//!
//! A **chain** is the events from one that opens a run while none is open to the first event after
//! which no run is open. A steering message closes the open run and opens its own in the same event
//! (`RunClose::Superseded`), so the chain goes on: no settled point lies inside it, and a page never
//! cuts there. Events that have no frame (`ui_catalog`, `thread_shared`) never open a run and belong
//! to the chain before them; the first chain starts at event 1. Chains partition the log.
//!
//! A **turn** is a chain whose first event is a person's message (the first chain counts as one
//! whatever starts it). A chain between two turns (a title, a tool attached, a CI report) rides along
//! with the turn before it and costs nothing of a page's `turns`.
//!
//! # The cut
//!
//! The fold keeps a ring of chain buffers, so memory is bounded by the turns asked for and not by
//! the thread. A page is a whole number of **settled** chains: it never includes a chain that is
//! still open (the stream says that, in full, from the page's `end`). The byte cap drops the oldest
//! chains that do not fit and never the newest: a single chain larger than the cap is returned
//! whole.
//!
//! ```text
//! frames(page) = frames(connect from event 1, no live text) restricted to [start, end]
//! ```
//!
//! The projector is folded from the first event whatever the page, so its state at a chain start is
//! a function of the log alone and every replica agrees; the cost of a page is a connect's, less the
//! events after the page for a read that goes back from a point.

use std::collections::VecDeque;
use std::io::Write;

use orch_agui_proto as agui;
use orch_core::{Event, EventBody};

use crate::frame::{Audience, Frame};
use crate::projector::{Projector, ThreadMeta};

/// The version of the projection: it rises whenever the frames written for an event already in a
/// log could change (a new activity, another id rule). `tests/projection-digests.txt` pins the
/// frames of every golden under it, and `tools/projection-digests-check.sh` fails a change to a
/// pinned line that does not raise this number. A client that stored frames of another version
/// discards them.
pub const PROJECTION_VERSION: u32 = 1;

/// The most chains the fold keeps for a thread whose turns do not bound it (a thread with no
/// person's message, a webhook or CI driven one): older chains are dropped.
const MAX_KEPT_CHAINS: usize = 512;

/// What a page may hold, whatever the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLimits {
    /// The most turns of a page (`server.history.maxTurns`).
    pub max_turns: usize,
    /// The most bytes of frames of a page, as serialised JSON (`server.history.maxPageBytes`); the
    /// newest chain is returned whole whatever its size.
    pub max_page_bytes: usize,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        HistoryLimits {
            max_turns: 100,
            max_page_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Which chains a page holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The newest `turns` turns, or, with `before`, the `turns` turns that precede the chain that
    /// holds event `before`.
    Turns {
        /// An event of the chain the caller holds the page of, normally the `start` of its oldest
        /// page. A value beyond the log counts as absent.
        before: Option<i64>,
        /// How many turns, at most [`HistoryLimits::max_turns`].
        turns: usize,
    },
    /// Back at least to the chain that holds event `since`, never beyond the limits; with `before`,
    /// the chains that precede the chain that holds it.
    Since {
        /// As in [`Window::Turns`].
        before: Option<i64>,
        /// The event that must be in the page.
        since: i64,
    },
    /// A catch-up: the settled chains after the chain that holds event `after`, up to the newest
    /// settled point (at most `max_turns` turns).
    After {
        /// The last event the caller holds. A value beyond the log counts as its end.
        after: i64,
    },
}

/// What the log says about the last run that had ended by the event a catch-up names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The event that ended it.
    pub seq: i64,
    /// Its run id.
    pub run_id: String,
}

/// A page of history.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The first event the page accounts for: a chain's first event (`end + 1` for an empty page).
    pub start: i64,
    /// The last event it accounts for, and a settled point: no run is open after it.
    pub end: i64,
    /// The thread's last event when it was read.
    pub head: i64,
    /// Whether events exist before `start`.
    pub earlier: bool,
    /// The frames of events `start..=end`, in order, with the resume ids of a connect stream.
    pub frames: Vec<Frame>,
    /// A catch-up only: the last run that had ended by the event named, if any.
    pub anchor: Option<Anchor>,
}

/// Whether the fold needs more events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Hand over the next event.
    More,
    /// The page is decided: nothing after this changes it.
    Done,
}

struct Chain {
    start: i64,
    end: i64,
    turn: bool,
    frames: Vec<Frame>,
}

/// The fold of one history read.
pub struct History {
    projector: Projector,
    window: Window,
    limits: HistoryLimits,
    head: i64,
    chains: VecDeque<Chain>,
    /// Turns among `chains`.
    turns: usize,
    /// Turns the ring keeps: the page's, and one more for a chain that is still open.
    keep_turns: usize,
    /// Chains that have started, dropped ones included.
    started: usize,
    /// A run has opened in this log: until then the first chain takes every event.
    ran: bool,
    /// The id of the run that is open.
    run: Option<String>,
    /// The last run that had ended by `after`.
    anchor: Option<Anchor>,
    /// A catch-up stopped at its turn cap, with the chain it did not take still to come.
    capped: bool,
    last_seq: i64,
    done: bool,
}

impl History {
    /// A read of the thread `meta` describes, whose log ended at `head` when the caller looked.
    pub fn new(meta: ThreadMeta, window: Window, limits: HistoryLimits, head: i64) -> Self {
        let head = head.max(0);
        let max_turns = limits.max_turns.max(1);
        let window = match window {
            Window::Turns { before, turns } => Window::Turns {
                before: before.filter(|b| (1..=head).contains(b)),
                turns: turns.clamp(1, max_turns),
            },
            Window::Since { before, since } => Window::Since {
                before: before.filter(|b| (1..=head).contains(b)),
                since,
            },
            Window::After { after } => Window::After {
                after: after.clamp(0, head),
            },
        };
        let keep = match window {
            Window::Turns { turns, .. } => turns,
            Window::Since { .. } | Window::After { .. } => max_turns,
        };
        History {
            projector: Projector::new(meta),
            window,
            limits,
            head,
            chains: VecDeque::new(),
            turns: 0,
            keep_turns: keep + 1,
            started: 0,
            ran: false,
            run: None,
            anchor: None,
            capped: false,
            last_seq: 0,
            done: false,
        }
    }

    /// The event after which the fold has nothing more to learn, if the window names one.
    fn stop_after(&self) -> Option<i64> {
        match self.window {
            Window::Turns { before, .. } | Window::Since { before, .. } => before,
            Window::After { .. } => None,
        }
    }

    /// Folds the next event (they arrive in `seq` order from the first) and says whether more are
    /// needed. Events beyond the head the caller named are ignored.
    pub fn feed(&mut self, event: &Event) -> Flow {
        if self.done || event.seq > self.head {
            self.done = true;
            return Flow::Done;
        }
        let was_open = self.projector.run_open();
        let frames = self.projector.apply(event, Audience::Viewer);
        self.last_seq = event.seq;
        let opens = !was_open
            && frames
                .iter()
                .any(|f| matches!(f.event, agui::Event::RunStarted(_)));
        // The first chain starts at event 1, whatever it is; the others at the event that opens a
        // run while none is open (the first chain holds the events up to its first run).
        let starts = self.started == 0 || (opens && self.ran);
        self.ran |= opens;
        self.track(event.seq, &frames);
        if starts && !self.start_chain(event) {
            self.done = true;
            self.capped = true;
            return Flow::Done;
        }
        if let Some(chain) = self.chains.back_mut() {
            chain.end = event.seq;
            chain.frames.extend(frames);
        }
        if self.stop_after().is_some_and(|before| event.seq >= before) {
            self.done = true;
            return Flow::Done;
        }
        Flow::More
    }

    /// Opens the chain that `event` starts. False when a catch-up holds as many turns as a page
    /// may and `event` would be one more.
    fn start_chain(&mut self, event: &Event) -> bool {
        let turn = self.started == 0 || matches!(event.body, EventBody::UserMessage(_));
        self.started += 1;
        if let Window::After { after } = self.window {
            // The chain that holds `after` and the ones before it are not kept: a catch-up holds
            // the chains after it, and nothing of the thread's past.
            if event.seq <= after {
                return true;
            }
            if turn && self.turns >= self.limits.max_turns.max(1) {
                return false;
            }
        }
        self.turns += usize::from(turn);
        self.chains.push_back(Chain {
            start: event.seq,
            end: event.seq,
            turn,
            frames: Vec::new(),
        });
        if !matches!(self.window, Window::After { .. }) {
            while self.turns > self.keep_turns {
                self.drop_oldest_turn();
            }
            while self.chains.len() > MAX_KEPT_CHAINS {
                self.pop_front();
            }
        }
        true
    }

    fn pop_front(&mut self) -> Option<Chain> {
        let chain = self.chains.pop_front()?;
        self.turns -= usize::from(chain.turn);
        Some(chain)
    }

    /// Drops the oldest chain and the ones that ride along with it, up to the next turn.
    fn drop_oldest_turn(&mut self) {
        self.pop_front();
        while self.chains.front().is_some_and(|c| !c.turn) {
            self.pop_front();
        }
    }

    /// Follows the run that is open, and, for a catch-up, the last one that had ended by `after`.
    fn track(&mut self, seq: i64, frames: &[Frame]) {
        for frame in frames {
            match &frame.event {
                agui::Event::RunStarted(e) => self.run = Some(e.run_id.to_string()),
                agui::Event::RunFinished(_) | agui::Event::RunError(_) => {
                    let ended = self.run.take();
                    if let (Some(run_id), Window::After { after }) = (ended, self.window)
                        && seq <= after
                    {
                        self.anchor = Some(Anchor { seq, run_id });
                    }
                }
                _ => {}
            }
        }
    }

    /// The page. Call it once the fold said [`Flow::Done`] or the log ran out.
    pub fn finish(mut self) -> Page {
        let head = self.head;
        let open = self.projector.run_open() && !self.capped;
        let mut anchor = None;
        let (start, end) = match self.window {
            Window::After { after } => {
                anchor = self.anchor.take();
                if open {
                    // the chain still open is the stream's
                    self.chains.pop_back();
                }
                self.fit_newest();
                match (self.chains.front(), self.chains.back()) {
                    (Some(first), Some(last)) => (first.start, last.end),
                    _ => (after + 1, after),
                }
            }
            Window::Turns { before, turns } => {
                let end = self.close_back(before.is_some() || open);
                while self.turns > turns {
                    self.drop_oldest_turn();
                }
                self.fit_oldest();
                (self.first_start(end), end)
            }
            Window::Since { before, since } => {
                let end = self.close_back(before.is_some() || open);
                while self.chains.front().is_some_and(|c| c.end < since) {
                    self.pop_front();
                }
                while self.turns > self.limits.max_turns.max(1) {
                    self.drop_oldest_turn();
                }
                self.fit_oldest();
                (self.first_start(end), end)
            }
        };
        let frames = self.chains.into_iter().flat_map(|c| c.frames).collect();
        Page {
            start,
            end,
            head,
            earlier: start > 1,
            frames,
            anchor,
        }
    }

    /// Takes the newest chain out of the page when `exclude` (it is open, or it is the one the
    /// caller holds) and returns the page's last event.
    fn close_back(&mut self, exclude: bool) -> i64 {
        if exclude && let Some(last) = self.chains.pop_back() {
            self.turns -= usize::from(last.turn);
            return last.start - 1;
        }
        self.chains.back().map_or(self.last_seq, |c| c.end)
    }

    fn first_start(&self, end: i64) -> i64 {
        self.chains.front().map_or(end + 1, |c| c.start)
    }

    /// The byte cap, going back from the newest: drops the oldest chains that do not fit, never
    /// the newest.
    fn fit_oldest(&mut self) {
        let mut total: usize = self.chains.iter().map(chain_bytes).sum();
        while self.chains.len() > 1 && total > self.limits.max_page_bytes {
            if let Some(oldest) = self.pop_front() {
                total -= chain_bytes(&oldest);
            }
        }
    }

    /// The byte cap for a catch-up, which must start where it was asked to: drops the newest
    /// chains that do not fit, never the first.
    fn fit_newest(&mut self) {
        let mut total: usize = self.chains.iter().map(chain_bytes).sum();
        while self.chains.len() > 1 && total > self.limits.max_page_bytes {
            if let Some(newest) = self.chains.pop_back() {
                total -= chain_bytes(&newest);
            }
        }
    }
}

/// What a chain's frames weigh serialised, with the `{"id":…,"event":…},` around each.
fn chain_bytes(chain: &Chain) -> usize {
    chain.frames.iter().map(frame_bytes).sum()
}

fn frame_bytes(frame: &Frame) -> usize {
    /// Counts what is written to it.
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // An AG-UI event is plain data and always serialises.
    let _ = serde_json::to_writer(&mut count, &frame.event);
    count.0 + 24
}
