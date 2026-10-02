//! The response stream of a run: the requester's projection of the log, from where the run
//! starts to its terminal event.
//!
//! [`Feed`] is everything the stream needs, already resolved by the handler: a projector that
//! has folded the log up to some point, events not yet folded (a backlog read earlier, then the
//! live stream), and where the run starts. The stream folds every event, in order, and writes the
//! frames from the start on; the projector state is a function of the events alone, so frames
//! before the start are simply not written.

use std::collections::{BTreeSet, VecDeque};
use std::convert::Infallible;

use axum::response::sse::Event as SseEvent;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use orch_agui_projection::{Audience, Frame, LiveOverlay, Projector};
use orch_agui_proto as agui;
use orch_app::FeedItem;
use orch_core::Event;

/// Where the stream of a run begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Start {
    /// At this log event: the first one the request's input caused. If a run is open by then (an
    /// event opened one between the read and the write), the run's opening frames come first, so
    /// the stream still starts with `RUN_STARTED`.
    Seq(i64),
    /// At the `RUN_STARTED` of the run with this id, which the log already holds: an attach.
    Run(String),
}

/// What [`frames`] folds and writes.
pub(crate) struct Feed {
    /// The projection so far.
    pub(crate) projector: Projector,
    /// Events to fold before the live ones, oldest first (an attach folds the whole log).
    pub(crate) backlog: VecDeque<Event>,
    /// Events after the backlog: the rest of the log, then new ones, with the live text of the
    /// thread's replies mixed in (ADR 0027).
    pub(crate) live: BoxStream<'static, FeedItem>,
    /// Where to begin writing.
    pub(crate) start: Start,
    /// The message ids of the request: the requester holds them already.
    pub(crate) held: BTreeSet<String>,
}

struct St {
    feed: Feed,
    /// The words of a reply still being written, beside the projection (never a resume point).
    overlay: LiveOverlay,
    writing: bool,
    done: bool,
    pending: VecDeque<Frame>,
}

impl St {
    /// Folds one event, and queues what is to be written.
    fn absorb(&mut self, event: &Event) {
        let audience = Audience::Requester {
            held_message_ids: &self.feed.held,
        };
        let mut frames = Vec::new();
        match (&self.feed.start, self.writing) {
            (Start::Seq(from), false) if event.seq >= *from => {
                self.writing = true;
                frames.extend(self.feed.projector.resume_preamble());
                frames.extend(self.feed.projector.apply(event, audience));
            }
            (Start::Run(run_id), false) => {
                let all = self.feed.projector.apply(event, audience);
                let opens = all.iter().position(|f| {
                    matches!(&f.event, agui::Event::RunStarted(r) if r.run_id.as_str() == run_id)
                });
                if let Some(at) = opens {
                    self.writing = true;
                    frames.extend(all.into_iter().skip(at));
                }
            }
            (Start::Seq(_), false) => {
                // Before the run: only the state moves.
                let _ = self.feed.projector.apply(event, audience);
            }
            (_, true) => frames.extend(self.feed.projector.apply(event, audience)),
        }
        // What the response says is the log's frames with the live text merged in; before the
        // run starts nothing is said, so the overlay has nothing to keep.
        let frames = if self.writing {
            self.overlay.logged(&self.feed.projector, frames)
        } else {
            frames
        };
        for frame in frames {
            let last = is_terminal(&frame.event);
            self.pending.push_back(frame);
            if last {
                // The run is over: nothing after its terminal event belongs to this response.
                self.done = true;
                break;
            }
        }
    }
}

/// A run ends with `RUN_FINISHED` or `RUN_ERROR`.
fn is_terminal(event: &agui::Event) -> bool {
    matches!(
        event,
        agui::Event::RunFinished(_) | agui::Event::RunError(_)
    )
}

/// One frame as an SSE message: `data:` is the event as JSON, and `id:` (the log's `seq`) is
/// there when the frame is a resume point. There is no `event:` name; consumers ignore it.
pub(crate) fn sse(frame: &Frame) -> SseEvent {
    let event = match SseEvent::default().json_data(&frame.event) {
        Ok(event) => event,
        Err(e) => {
            // An AG-UI event is plain data and always serialises; if it ever did not, the
            // stream must go on rather than end without a terminal event.
            tracing::error!(error = %e, "an AG-UI event could not be encoded");
            return SseEvent::default().comment("frame lost");
        }
    };
    match frame.resume_id {
        Some(seq) => event.id(seq.to_string()),
        None => event,
    }
}

/// The SSE messages of the run: every frame from the start to the terminal event, then the end.
///
/// The stream also ends when `feed.live` does (the process is shutting down): the client then
/// holds a truncated run, which the protocol says is not a cancelled one, and re-POSTs the same
/// `runId` to attach.
pub(crate) fn frames(feed: Feed) -> impl Stream<Item = Result<SseEvent, Infallible>> + Send {
    let st = St {
        feed,
        overlay: LiveOverlay::new(),
        writing: false,
        done: false,
        pending: VecDeque::new(),
    };
    futures::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(frame) = st.pending.pop_front() {
                return Some((Ok(sse(&frame)), st));
            }
            if st.done {
                return None;
            }
            if let Some(event) = st.feed.backlog.pop_front() {
                st.absorb(&event);
                continue;
            }
            match st.feed.live.next().await? {
                FeedItem::Event(event) => st.absorb(&event),
                // Only once the run is being written, and the backlog is folded (it is, here).
                FeedItem::Live(piece) if st.writing => {
                    let said = st.overlay.live(&st.feed.projector, &piece);
                    st.pending.extend(said);
                }
                FeedItem::Live(_) => {}
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use axum::response::IntoResponse;
    use orch_agui_projection::ThreadMeta;
    use orch_core::{
        Actor, AgentId, AgentStatus, AgentStatusData, AgentTarget, EventBody, Origin, ThreadId,
        ThreadState, ThreadStateData, Timestamp, UserId, UserMessageData,
    };

    use super::*;

    const THREAD: &str = "00000000-0000-7000-8000-000000000001";

    fn meta() -> ThreadMeta {
        ThreadMeta {
            thread_id: THREAD.parse::<ThreadId>().unwrap(),
            title: "t".to_owned(),
            description: None,
            target: AgentTarget {
                agent_id: AgentId::new("plain"),
                release: None,
            },
            gate: orch_core::GatePolicy::default(),
        }
    }

    fn event(seq: i64, actor: Actor, body: EventBody) -> Event {
        Event {
            seq,
            thread_id: THREAD.parse().unwrap(),
            at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
            actor,
            body,
        }
    }

    fn user(seq: i64, text: &str, run: &str) -> Event {
        event(
            seq,
            Actor::user(&UserId::new("alice@example.com")),
            EventBody::UserMessage(UserMessageData {
                text: text.to_owned(),
                message_id: Some(format!("m-{seq}")),
                run_id: Some(run.to_owned()),
                origin: Origin::default(),
                delivery: None,
            }),
        )
    }

    fn status(seq: i64, s: AgentStatus) -> Event {
        event(
            seq,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentStatus(AgentStatusData {
                status: s,
                detail: None,
            }),
        )
    }

    fn state(seq: i64, s: ThreadState) -> Event {
        event(
            seq,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData { state: s }),
        )
    }

    /// The types of the events a feed writes, by running its stream to the end.
    async fn written(feed: Feed) -> Vec<(String, Option<String>)> {
        // The SSE events have no public accessors; render them through a response body.
        let sse = axum::response::sse::Sse::new(frames(feed));
        let body = sse.into_response().into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .split("\n\n")
            .filter(|b| !b.is_empty())
            .map(|block| {
                let mut id = None;
                let mut kind = String::new();
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("id: ") {
                        id = Some(v.to_owned());
                    } else if let Some(v) = line.strip_prefix("data: ") {
                        let json: serde_json::Value = serde_json::from_str(v).unwrap();
                        kind = json["type"].as_str().unwrap().to_owned();
                    }
                }
                (kind, id)
            })
            .collect()
    }

    fn feed(folded: &[Event], rest: Vec<Event>, start: Start) -> Feed {
        let mut projector = Projector::new(meta());
        for e in folded {
            let _ = projector.apply(e, Audience::Viewer);
        }
        Feed {
            projector,
            backlog: rest.into(),
            live: futures::stream::empty().boxed(),
            start,
            held: BTreeSet::new(),
        }
    }

    #[tokio::test]
    async fn a_run_that_opened_between_the_read_and_the_write_is_opened_again_for_the_reader() {
        // Seq 1 opened a run (a producer-initiated status); the request's message, seq 2, lands
        // inside that run. The stream must still begin with RUN_STARTED.
        let folded = [status(1, AgentStatus::Working)];
        let rest = vec![
            user(2, "hurry", "r-mine"),
            status(3, AgentStatus::Completed),
            state(4, ThreadState::Done),
        ];
        let got = written(feed(&folded, rest, Start::Seq(2))).await;
        let kinds: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds[0], "RUN_STARTED", "{kinds:?}");
        assert_eq!(kinds[1], "SUBAGENT_STARTED");
        assert_eq!(kinds[2], "STATE_SNAPSHOT");
        assert_eq!(*kinds.last().unwrap(), "RUN_FINISHED");
        assert!(
            got.first().unwrap().1.is_none(),
            "the preamble is no resume point"
        );
    }

    #[tokio::test]
    async fn a_message_that_ended_the_open_run_starts_the_response_at_its_own_run() {
        // ADR 0036: the message of this request landed inside a run that was open (seq 1 and 2);
        // that run is finished by it, and the response is the run the message opened, from its
        // `RUN_STARTED`: nothing of the run it ended, which would end the stream at once.
        let folded = [user(1, "go", "r-first"), status(2, AgentStatus::Working)];
        let rest = vec![
            user(3, "hurry", "r-mine"),
            status(4, AgentStatus::Completed),
            state(5, ThreadState::Done),
        ];
        let got = written(feed(&folded, rest, Start::Run("r-mine".into()))).await;
        let kinds: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds[0], "RUN_STARTED", "{kinds:?}");
        assert_eq!(
            kinds.iter().filter(|k| **k == "RUN_STARTED").count(),
            1,
            "{kinds:?}"
        );
        assert_eq!(kinds.last(), Some(&"RUN_FINISHED"));
        assert_eq!(got.last().unwrap().1.as_deref(), Some("5"));
        // the same read as a position in the log would have ended with the run it finished
        let by_seq = written(feed(
            &folded,
            vec![user(3, "hurry", "r-mine")],
            Start::Seq(3),
        ))
        .await;
        assert_eq!(by_seq.last().unwrap().0, "RUN_FINISHED");
    }

    #[tokio::test]
    async fn frames_before_the_start_are_folded_but_not_written() {
        let rest = vec![
            user(1, "echo", "r1"),
            status(2, AgentStatus::Completed),
            state(3, ThreadState::Done),
        ];
        // Starting at seq 2 skips the first event's frames (the run opened at 1 is open, so
        // the preamble stands in for them).
        let got = written(feed(&[], rest, Start::Seq(2))).await;
        assert_eq!(got.first().unwrap().0, "RUN_STARTED");
        assert!(!got.iter().any(|(k, _)| k == "TEXT_MESSAGE_START"));
        assert_eq!(
            got.last().unwrap(),
            &("RUN_FINISHED".to_owned(), Some("3".to_owned()))
        );
    }

    #[tokio::test]
    async fn an_attach_writes_the_named_run_and_stops_at_its_end() {
        let rest = vec![
            user(1, "ask", "r1"),
            status(2, AgentStatus::InputRequired),
            state(3, ThreadState::Blocked),
            user(4, "main", "r2"),
            status(5, AgentStatus::Completed),
            state(6, ThreadState::Done),
        ];
        let first = written(feed(&[], rest.clone(), Start::Run("r1".into()))).await;
        assert_eq!(first.first().unwrap().0, "RUN_STARTED");
        assert_eq!(first.last().unwrap().0, "RUN_FINISHED");
        assert_eq!(first.last().unwrap().1.as_deref(), Some("3"));
        let second = written(feed(&[], rest, Start::Run("r2".into()))).await;
        assert_eq!(second.first().unwrap().0, "RUN_STARTED");
        assert_eq!(second.last().unwrap().1.as_deref(), Some("6"));
        // A run the log does not hold is never written; the stream just ends.
        let none = written(feed(
            &[],
            vec![user(1, "echo", "r1")],
            Start::Run("nope".into()),
        ))
        .await;
        assert!(none.is_empty());
    }
}
