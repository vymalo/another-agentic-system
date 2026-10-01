//! The relay of live text (ADR 0027): what the dispatcher does with the pieces of a reply an agent
//! is still writing (`text-stream/v1`) while it consumes the agent's stream.
//!
//! A piece is **never applied**: nothing of it is written to the log (the whole text is, once, as
//! an ordinary agent message, when the agent states it). The relay publishes the pieces on the
//! wakeup port, which carries them to every process that serves a viewer, and forgets them. It is
//! best effort in every direction: a failed publish is logged at `debug` and never fails a
//! delegation.
//!
//! One [`LiveRelay`] serves one consumed stream. Per reply (stream id) it keeps
//!
//! - the **text known from offset 0**, while the first piece it saw started at 0 and nothing was
//!   lost since, up to `live_max_bytes` (a viewer that connects mid-stream, or lost a piece, is
//!   sent the text so far by the refresh);
//! - a **pending piece**, what arrived since the last publish, merged: at most one publish per
//!   `live_flush` for a reply, at once on the last piece;
//!
//! and every `live_refresh` it republishes the whole known text from offset 0, in pieces of at most
//! [`MAX_LIVE_PIECE_BYTES`], for each reply that is still open and small enough. A reply is
//! forgotten when its last piece is published or when its whole text reaches the log
//! ([`LiveRelay::persisted`]). At most [`MAX_OPEN_STREAMS`] replies are followed at once.

use std::time::Duration;

use orch_core::{AgentId, LiveChunk, LiveEnd, LiveText, MAX_LIVE_PIECE_BYTES, ThreadId};
use orch_ports::{Wakeup, WakeupCapabilities};
use tokio::time::Instant;

/// Most replies one delegation relays at once; a reply beyond that is relayed without a refresh.
const MAX_OPEN_STREAMS: usize = 8;

/// What the relay is told to do, from the dispatcher's configuration.
#[derive(Debug, Clone, Copy)]
pub(super) struct LiveTiming {
    /// The least time between two publishes of one reply.
    pub flush: Duration,
    /// How often the text so far is published again from offset 0.
    pub refresh: Duration,
    /// The most text the relay holds per reply for the refresh, in bytes.
    pub max_bytes: usize,
}

/// One reply being relayed.
struct Stream {
    id: String,
    /// The text from offset 0, while it is known; `None` once the reply was joined mid-way, a piece
    /// was lost, or it outgrew the bound.
    known: Option<String>,
    /// Where the next piece is expected to start (bytes).
    end: u64,
    /// What arrived and was not published yet, and where it starts.
    pending: String,
    pending_at: u64,
    /// When the reply was last published, and last republished from the start.
    flushed: Instant,
    refreshed: Instant,
}

/// The relay of one consumed stream. See the module documentation.
pub(super) struct LiveRelay<'a, W: Wakeup> {
    wakeup: &'a W,
    thread: ThreadId,
    agent: AgentId,
    timing: LiveTiming,
    on: bool,
    streams: Vec<Stream>,
}

impl<'a, W: Wakeup> LiveRelay<'a, W> {
    pub(super) fn new(
        wakeup: &'a W,
        capabilities: WakeupCapabilities,
        thread: ThreadId,
        agent: AgentId,
        timing: LiveTiming,
    ) -> Self {
        LiveRelay {
            wakeup,
            thread,
            agent,
            timing,
            // Without the capability nothing is published, so nothing is kept either.
            on: capabilities.live,
            streams: Vec::new(),
        }
    }

    /// When something is due: a pending piece to publish, or a refresh. `None` when nothing is.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.streams
            .iter()
            .flat_map(|s| {
                let flush = (!s.pending.is_empty()).then_some(s.flushed + self.timing.flush);
                let refresh = s
                    .known
                    .as_ref()
                    .filter(|k| !k.is_empty())
                    .map(|_| s.refreshed + self.timing.refresh);
                [flush, refresh]
            })
            .flatten()
            .min()
    }

    /// One piece of a reply, from the agent.
    pub(super) async fn chunk(&mut self, chunk: LiveChunk) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        let at = self.streams.iter().position(|s| s.id == chunk.message_id);
        let at = match at {
            Some(at) => at,
            None => {
                if self.streams.len() == MAX_OPEN_STREAMS {
                    self.streams.remove(0);
                }
                self.streams.push(Stream {
                    id: chunk.message_id.clone(),
                    known: (chunk.offset == 0).then(String::new),
                    end: chunk.offset,
                    pending: String::new(),
                    pending_at: chunk.offset,
                    flushed: now.checked_sub(self.timing.flush).unwrap_or(now),
                    refreshed: now,
                });
                self.streams.len() - 1
            }
        };
        let max_bytes = self.timing.max_bytes;
        let s = &mut self.streams[at];

        // Where the piece lands: an overlap is trimmed to what is new, a gap is published as it is
        // (the viewers place it or ignore it) and means the text is no longer known whole.
        let (offset, text) = if chunk.offset < s.end {
            let skip = usize::try_from(s.end - chunk.offset).unwrap_or(usize::MAX);
            match chunk.text.get(skip..) {
                Some(rest) => (s.end, rest),
                // The agent repeated words and the cut is not on a character: nothing is new.
                None if skip >= chunk.text.len() => (s.end, ""),
                None => return,
            }
        } else {
            (chunk.offset, chunk.text.as_str())
        };
        if offset > s.end {
            s.known = None;
        }
        // What is pending must be contiguous with what is added: publish it first when it is not.
        let mut now_out: Vec<(u64, String, LiveEnd)> = Vec::new();
        if !s.pending.is_empty() && offset != s.pending_at + s.pending.len() as u64 {
            now_out.push((s.pending_at, std::mem::take(&mut s.pending), LiveEnd::Open));
        }
        if s.pending.is_empty() {
            s.pending_at = offset;
        }
        s.pending.push_str(text);
        s.end = offset + text.len() as u64;
        if let Some(known) = &mut s.known {
            if known.len() + text.len() <= max_bytes {
                known.push_str(text);
            } else {
                s.known = None;
            }
        }

        let due = now >= s.flushed + self.timing.flush;
        if chunk.end != LiveEnd::Open {
            // The reply is over: what is pending goes out now, marked, and it is forgotten.
            let last = (s.pending_at, std::mem::take(&mut s.pending), chunk.end);
            let id = chunk.message_id.clone();
            self.streams.remove(at);
            for (offset, text, end) in now_out.into_iter().chain([last]) {
                self.publish(&id, offset, &text, end).await;
            }
            return;
        }
        if due && !s.pending.is_empty() {
            now_out.push((s.pending_at, std::mem::take(&mut s.pending), LiveEnd::Open));
            s.flushed = now;
        }
        let id = chunk.message_id;
        for (offset, text, end) in now_out {
            self.publish(&id, offset, &text, end).await;
        }
    }

    /// The whole text of a reply is in the log: no more of it is relayed.
    pub(super) fn persisted(&mut self, id: &str) {
        self.streams.retain(|s| s.id != id);
    }

    /// Publishes what is due: pending pieces whose time has come, and the refresh of the replies
    /// whose turn it is.
    pub(super) async fn tick(&mut self) {
        let now = Instant::now();
        let mut out: Vec<(String, u64, String)> = Vec::new();
        for s in &mut self.streams {
            if !s.pending.is_empty() && now >= s.flushed + self.timing.flush {
                out.push((s.id.clone(), s.pending_at, std::mem::take(&mut s.pending)));
                s.flushed = now;
            }
            if let Some(known) = s.known.as_ref().filter(|k| !k.is_empty())
                && now >= s.refreshed + self.timing.refresh
            {
                // The whole text so far, which includes whatever was pending a moment ago.
                out.push((s.id.clone(), 0, known.clone()));
                s.refreshed = now;
            }
        }
        for (id, offset, text) in out {
            self.publish(&id, offset, &text, LiveEnd::Open).await;
        }
    }

    /// Publishes `text` (which starts at `offset` bytes) as pieces of at most
    /// [`MAX_LIVE_PIECE_BYTES`], cut at characters, the end on the last. Failing is not an error.
    async fn publish(&self, id: &str, offset: u64, text: &str, end: LiveEnd) {
        let mut pieces = Vec::new();
        let mut from = 0usize;
        while from < text.len() {
            let mut to = (from + MAX_LIVE_PIECE_BYTES).min(text.len());
            while !text.is_char_boundary(to) {
                to -= 1;
            }
            pieces.push(&text[from..to]);
            from = to;
        }
        // An ending piece with no words still says that the reply is over.
        if pieces.is_empty() && end != LiveEnd::Open {
            pieces.push("");
        }
        let last = pieces.len().saturating_sub(1);
        let mut at = offset;
        for (n, piece) in pieces.into_iter().enumerate() {
            let live = LiveText {
                thread: self.thread,
                agent: self.agent.clone(),
                chunk: LiveChunk {
                    message_id: id.to_owned(),
                    offset: at,
                    text: piece.to_owned(),
                    end: if n == last { end } else { LiveEnd::Open },
                },
            };
            at += piece.len() as u64;
            if let Err(e) = self.wakeup.publish_live(live).await {
                tracing::debug!(error = %orch_core::report(&e), "live text was not relayed");
                return;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use futures::stream::BoxStream;
    use futures::{FutureExt, StreamExt};
    use orch_core::{LiveChunk, LiveEnd, LiveText};
    use orch_ports::memory::MemoryWakeup;
    use orch_ports::{Topic, WakeupError};

    use super::*;

    const FLUSH: Duration = Duration::from_millis(100);
    const REFRESH: Duration = Duration::from_secs(1);

    fn timing() -> LiveTiming {
        LiveTiming {
            flush: FLUSH,
            refresh: REFRESH,
            max_bytes: 64 * 1024,
        }
    }

    fn thread() -> ThreadId {
        ThreadId(uuid::Uuid::from_u128(
            0x7000_8000_0000_0000_0000_0000_0000_0007,
        ))
    }

    fn relay(wakeup: &MemoryWakeup) -> LiveRelay<'_, MemoryWakeup> {
        LiveRelay::new(
            wakeup,
            wakeup.capabilities(),
            thread(),
            AgentId::new("coder"),
            timing(),
        )
    }

    fn chunk(id: &str, offset: u64, text: &str, end: LiveEnd) -> LiveChunk {
        LiveChunk {
            message_id: id.to_owned(),
            offset,
            text: text.to_owned(),
            end,
        }
    }

    /// What has been published so far: (stream, offset, text, end).
    fn heard(sub: &mut BoxStream<'static, LiveText>) -> Vec<(String, u64, String, LiveEnd)> {
        let mut out = Vec::new();
        while let Some(Some(piece)) = sub.next().now_or_never() {
            assert_eq!(piece.thread, thread());
            assert_eq!(piece.agent, AgentId::new("coder"));
            let c = piece.chunk;
            out.push((c.message_id, c.offset, c.text, c.end));
        }
        out
    }

    fn one(id: &str, offset: u64, text: &str, end: LiveEnd) -> (String, u64, String, LiveEnd) {
        (id.to_owned(), offset, text.to_owned(), end)
    }

    /// The pieces of one stream joined by offset: the text they say.
    fn said(pieces: &[(String, u64, String, LiveEnd)], id: &str) -> String {
        let mut text = String::new();
        for (stream, offset, piece, _) in pieces {
            if stream == id {
                assert_eq!(*offset as usize, text.len(), "pieces follow one another");
                text.push_str(piece);
            }
        }
        text
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_piece_goes_out_at_once_and_the_next_ones_are_merged_per_flush_interval() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        assert_eq!(heard(&mut sub), [one("S", 0, "Fib", LiveEnd::Open)]);

        // Within the interval nothing goes out, and what arrives is merged.
        relay.chunk(chunk("S", 3, "onacci ", LiveEnd::Open)).await;
        relay.chunk(chunk("S", 10, "in ", LiveEnd::Open)).await;
        assert!(heard(&mut sub).is_empty());
        assert_eq!(relay.deadline(), Some(Instant::now() + FLUSH));

        tokio::time::advance(FLUSH).await;
        relay.tick().await;
        assert_eq!(heard(&mut sub), [one("S", 3, "onacci in ", LiveEnd::Open)]);
        // Nothing pending: the next deadline is the refresh, not a flush.
        assert!(relay.deadline().is_some_and(|d| d > Instant::now() + FLUSH));
    }

    #[tokio::test(start_paused = true)]
    async fn the_last_piece_goes_out_at_once_with_what_is_pending_and_ends_the_stream() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        relay.chunk(chunk("S", 3, "onacci", LiveEnd::Open)).await;
        relay.chunk(chunk("S", 9, " in Rust.", LiveEnd::Last)).await;
        assert_eq!(
            heard(&mut sub),
            [
                one("S", 0, "Fib", LiveEnd::Open),
                one("S", 3, "onacci in Rust.", LiveEnd::Last)
            ]
        );
        // The stream is forgotten: nothing more is due, however long we wait.
        assert_eq!(relay.deadline(), None);
        tokio::time::advance(Duration::from_secs(10)).await;
        relay.tick().await;
        assert!(heard(&mut sub).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn giving_up_goes_out_at_once_marked() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        relay.chunk(chunk("S", 3, "", LiveEnd::Abandoned)).await;
        let got = heard(&mut sub);
        assert_eq!(got.last().unwrap(), &one("S", 3, "", LiveEnd::Abandoned));
        assert_eq!(relay.deadline(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_whole_text_so_far_is_published_again_every_refresh_interval() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        tokio::time::advance(FLUSH).await;
        relay.chunk(chunk("S", 3, "onacci", LiveEnd::Open)).await;
        heard(&mut sub);

        tokio::time::advance(REFRESH).await;
        relay.tick().await;
        let got = heard(&mut sub);
        assert_eq!(said(&got, "S"), "Fibonacci");
        assert_eq!(got[0].1, 0, "from the start");
        // And again a second later, with what came in between.
        relay.chunk(chunk("S", 9, " in Rust.", LiveEnd::Open)).await;
        heard(&mut sub);
        tokio::time::advance(REFRESH).await;
        relay.tick().await;
        assert_eq!(said(&heard(&mut sub), "S"), "Fibonacci in Rust.");
    }

    #[tokio::test(start_paused = true)]
    async fn a_reply_that_reached_the_log_is_not_refreshed_any_more() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        heard(&mut sub);
        relay.persisted("S");
        assert_eq!(relay.deadline(), None);
        tokio::time::advance(Duration::from_secs(5)).await;
        relay.tick().await;
        assert!(heard(&mut sub).is_empty());
        // Another reply is not touched by it.
        relay.chunk(chunk("T", 0, "Hi", LiveEnd::Open)).await;
        relay.persisted("S");
        assert_eq!(heard(&mut sub), [one("T", 0, "Hi", LiveEnd::Open)]);
        assert!(relay.deadline().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_piece_bigger_than_the_bound_is_cut_at_characters_and_ends_on_the_last_part() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        // 4-byte characters never fall on the 6 KiB cut.
        let text = "\u{1f980}".repeat(MAX_LIVE_PIECE_BYTES * 2 / 4 + 7);
        relay.chunk(chunk("S", 0, &text, LiveEnd::Last)).await;
        let got = heard(&mut sub);
        assert!(got.len() >= 3, "{}", got.len());
        for (_, _, piece, _) in &got {
            assert!(piece.len() <= MAX_LIVE_PIECE_BYTES);
        }
        assert_eq!(said(&got, "S"), text);
        let (last, rest) = got.split_last().unwrap();
        assert_eq!(last.3, LiveEnd::Last);
        assert!(rest.iter().all(|p| p.3 == LiveEnd::Open));
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_reply_is_refreshed_in_pieces_up_to_the_bound_and_not_beyond() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = LiveRelay::new(
            &wakeup,
            wakeup.capabilities(),
            thread(),
            AgentId::new("coder"),
            LiveTiming {
                max_bytes: 10 * 1024,
                ..timing()
            },
        );
        let mut offset = 0u64;
        for _ in 0..2 {
            let piece = "x".repeat(4 * 1024);
            relay.chunk(chunk("S", offset, &piece, LiveEnd::Open)).await;
            offset += piece.len() as u64;
            tokio::time::advance(FLUSH).await;
        }
        heard(&mut sub);
        tokio::time::advance(REFRESH).await;
        relay.tick().await;
        let got = heard(&mut sub);
        assert_eq!(said(&got, "S").len(), 8 * 1024);
        assert!(got.iter().all(|p| p.2.len() <= MAX_LIVE_PIECE_BYTES));
        assert!(got.len() >= 2, "an 8 KiB text is more than one piece");

        // Past the bound the text is no longer held: pieces still go out, refreshes do not.
        let piece = "y".repeat(4 * 1024);
        relay.chunk(chunk("S", offset, &piece, LiveEnd::Open)).await;
        let live = heard(&mut sub);
        assert_eq!(live.len(), 1);
        assert_eq!((live[0].1, live[0].2.len()), (offset, 4 * 1024));
        tokio::time::advance(REFRESH * 3).await;
        relay.tick().await;
        assert!(heard(&mut sub).is_empty(), "no refresh past the bound");
    }

    #[tokio::test(start_paused = true)]
    async fn a_reply_joined_in_the_middle_is_relayed_and_never_refreshed() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 40, "tail", LiveEnd::Open)).await;
        assert_eq!(heard(&mut sub), [one("S", 40, "tail", LiveEnd::Open)]);
        tokio::time::advance(REFRESH * 3).await;
        relay.tick().await;
        assert!(heard(&mut sub).is_empty());
        assert_eq!(relay.deadline(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_gap_is_relayed_as_it_is_and_ends_the_refresh() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        tokio::time::advance(FLUSH).await;
        // 3..10 were lost on the way from the agent.
        relay.chunk(chunk("S", 10, "in Rust.", LiveEnd::Open)).await;
        assert_eq!(
            heard(&mut sub),
            [
                one("S", 0, "Fib", LiveEnd::Open),
                one("S", 10, "in Rust.", LiveEnd::Open)
            ]
        );
        tokio::time::advance(REFRESH * 2).await;
        relay.tick().await;
        assert!(heard(&mut sub).is_empty(), "the text is not known whole");
    }

    #[tokio::test(start_paused = true)]
    async fn an_overlap_publishes_only_what_is_new() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("S", 0, "Fibonacci", LiveEnd::Open)).await;
        tokio::time::advance(FLUSH).await;
        heard(&mut sub);
        // The agent says the last words again, and then some.
        relay.chunk(chunk("S", 6, "cci in", LiveEnd::Open)).await;
        assert_eq!(heard(&mut sub), [one("S", 9, " in", LiveEnd::Open)]);
        // A repeat says nothing.
        relay
            .chunk(chunk("S", 0, "Fibonacci in", LiveEnd::Open))
            .await;
        tokio::time::advance(FLUSH).await;
        relay.tick().await;
        assert!(heard(&mut sub).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn several_replies_are_followed_at_once() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        relay.chunk(chunk("A", 0, "words ", LiveEnd::Open)).await;
        relay.chunk(chunk("A", 6, "before", LiveEnd::Last)).await;
        relay.chunk(chunk("B", 0, "answer", LiveEnd::Open)).await;
        let got = heard(&mut sub);
        assert_eq!(said(&got, "A"), "words before");
        assert_eq!(said(&got, "B"), "answer");
        assert_eq!(got.last().unwrap(), &one("B", 0, "answer", LiveEnd::Open));
    }

    #[tokio::test(start_paused = true)]
    async fn only_so_many_replies_are_followed_and_the_oldest_is_dropped() {
        let wakeup = MemoryWakeup::new();
        let mut sub = wakeup.subscribe_live();
        let mut relay = relay(&wakeup);
        for n in 0..=MAX_OPEN_STREAMS {
            relay
                .chunk(chunk(&format!("S{n}"), 0, "x", LiveEnd::Open))
                .await;
        }
        assert_eq!(
            heard(&mut sub).len(),
            MAX_OPEN_STREAMS + 1,
            "all were relayed"
        );
        tokio::time::advance(REFRESH).await;
        relay.tick().await;
        let refreshed: std::collections::BTreeSet<String> =
            heard(&mut sub).into_iter().map(|p| p.0).collect();
        assert_eq!(refreshed.len(), MAX_OPEN_STREAMS);
        assert!(!refreshed.contains("S0"), "the oldest lost its refresh");
    }

    /// A wakeup that cannot carry live text, or that fails to.
    #[derive(Clone)]
    struct Down {
        live: bool,
        published: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Wakeup for Down {
        async fn notify(&self, _: Topic) -> Result<(), WakeupError> {
            Ok(())
        }
        fn subscribe(&self) -> BoxStream<'static, Topic> {
            futures::stream::empty().boxed()
        }
        fn capabilities(&self) -> WakeupCapabilities {
            WakeupCapabilities {
                push: true,
                live: self.live,
            }
        }
        async fn publish_live(&self, _: LiveText) -> Result<(), WakeupError> {
            self.published
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(WakeupError::unavailable(std::io::Error::other("down")))
        }
        fn subscribe_live(&self) -> BoxStream<'static, LiveText> {
            futures::stream::empty().boxed()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_publish_that_fails_is_not_an_error_and_the_relay_goes_on() {
        let down = Down {
            live: true,
            published: std::sync::Arc::default(),
        };
        let mut relay = LiveRelay::new(
            &down,
            down.capabilities(),
            thread(),
            AgentId::new("coder"),
            timing(),
        );
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Open)).await;
        tokio::time::advance(FLUSH).await;
        relay.chunk(chunk("S", 3, "onacci", LiveEnd::Last)).await;
        assert_eq!(
            down.published.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "each was tried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn without_the_capability_nothing_is_published_or_kept() {
        let down = Down {
            live: false,
            published: std::sync::Arc::default(),
        };
        let mut relay = LiveRelay::new(
            &down,
            down.capabilities(),
            thread(),
            AgentId::new("coder"),
            timing(),
        );
        relay.chunk(chunk("S", 0, "Fib", LiveEnd::Last)).await;
        assert_eq!(down.published.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(relay.deadline(), None);
    }
}
