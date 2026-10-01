//! Live text through the application (ADR 0027): the dispatcher relays the pieces of a reply an
//! agent is still writing on the wakeup port and never applies them, the log gets the whole text
//! once as the agent message under the stream's id, and `App::thread_feed` mixes the pieces of a
//! thread into its events for the viewers (after the replay, and never those of another thread).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_app::FeedItem;
use orch_core::{AgentTaskState, EventBody, LiveChunk, LiveEnd, LiveText, ThreadId, ThreadState};
use orch_ports::memory::{STREAM_PIECES, stream_id, stream_text};
use orch_ports::{ThreadStore, Wakeup, WakeupCapabilities, WakeupError};
use support::*;

/// Everything published until the stream is quiet for `quiet`.
async fn collect(sub: &mut BoxStream<'static, LiveText>, quiet: Duration) -> Vec<LiveText> {
    let mut out = Vec::new();
    while let Ok(Some(piece)) = tokio::time::timeout(quiet, sub.next()).await {
        out.push(piece);
    }
    out
}

/// Everything published in the next `span` (a stream that is refreshed is never quiet).
async fn collect_for(sub: &mut BoxStream<'static, LiveText>, span: Duration) -> Vec<LiveText> {
    let end = tokio::time::Instant::now() + span;
    let mut out = Vec::new();
    while let Ok(Some(piece)) = tokio::time::timeout_at(end, sub.next()).await {
        out.push(piece);
    }
    out
}

/// The text the pieces of `stream` say, joined by their byte offsets.
fn said(pieces: &[LiveText], stream: &str) -> String {
    let mut text = String::new();
    for p in pieces.iter().filter(|p| p.chunk.message_id == stream) {
        let at = p.chunk.offset as usize;
        // a refresh from the start overlaps what was said
        assert!(
            at <= text.len(),
            "a piece starts beyond what was said: {p:?}"
        );
        text.push_str(&p.chunk.text[text.len() - at..]);
    }
    text
}

#[tokio::test]
async fn a_streamed_reply_is_relayed_in_pieces_and_logged_once_whole() {
    let w = World::new();
    let app = w.app();
    let mut live = w.wakeup.subscribe_live();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream tell me").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;

    // The log has the reply once, as one final message under the stream's id, and no partial.
    let task = "task-1";
    let messages: Vec<_> = ev
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::AgentMessage(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 1, "{:?}", shape(&ev));
    assert_eq!(messages[0].message_id, stream_id(task));
    assert_eq!(messages[0].text, stream_text());
    assert!(messages[0].is_final);
    assert_contiguous(&ev);
    // Nothing of a piece is in the log: no event of its own, no artifact.
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "agent_message",
            "agent_status:completed",
            "thread_state:done"
        ]
    );

    // The pieces went out on the wakeup port, in order, ending with the last one.
    let pieces = collect(&mut live, Duration::from_millis(300)).await;
    assert!(
        pieces
            .iter()
            .all(|p| p.thread == t.id && p.agent.as_str() == "plain"),
        "{pieces:?}"
    );
    assert_eq!(said(&pieces, &stream_id(task)), stream_text());
    assert!(
        pieces.len() >= 2,
        "more than one piece before the end: {}",
        pieces.len()
    );
    let ends: Vec<LiveEnd> = pieces.iter().map(|p| p.chunk.end).collect();
    assert_eq!(ends.last(), Some(&LiveEnd::Last));
    assert!(ends[..ends.len() - 1].iter().all(|e| *e == LiveEnd::Open));
    run.shutdown().await;
}

#[tokio::test]
async fn a_piece_is_never_applied_it_leaves_no_trace_in_the_thread() {
    // Nothing of a piece reaches the log or the binding: no artifact, no error, and the task ends
    // the way the status says.
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream tell me").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    let binding = w.store.get_binding(t.id).await.unwrap().unwrap();
    assert_eq!(binding.task_state, Some(AgentTaskState::Completed));
    assert!(
        ev.iter()
            .all(|e| !matches!(&e.body, EventBody::Artifact(_) | EventBody::Error(_)))
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_reply_given_up_is_relayed_marked_and_logs_no_message() {
    let w = World::new();
    let app = w.app();
    let mut live = w.wakeup.subscribe_live();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream-abandon go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    let ev = events(&app, &alice(), t.id).await;
    assert!(
        !ev.iter()
            .any(|e| matches!(&e.body, EventBody::AgentMessage(_))),
        "{:?}",
        shape(&ev)
    );
    let pieces = collect(&mut live, Duration::from_millis(300)).await;
    let last = pieces.last().unwrap();
    assert_eq!(last.chunk.end, LiveEnd::Abandoned);
    assert_eq!(
        said(&pieces, &stream_id("task-1")),
        format!("{}{}", STREAM_PIECES[0], STREAM_PIECES[1])
    );
    run.shutdown().await;
}

#[tokio::test]
async fn an_open_reply_is_published_again_from_the_start_while_the_agent_is_quiet() {
    let w = World::new();
    let app = w.app();
    let mut live = w.wakeup.subscribe_live();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "stream-gate go").await;
    // Three pieces, then the agent stops until the gate opens.
    let first = STREAM_PIECES[..3].concat();
    let mut heard = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while said(&heard, &stream_id("task-1")) != first {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first three pieces never came: {heard:?}"
        );
        heard.extend(collect_for(&mut live, Duration::from_millis(20)).await);
    }
    // The refresh (150 ms in the test configuration) repeats the whole text so far, from 0.
    let again = collect_for(&mut live, Duration::from_millis(500)).await;
    assert!(
        !again.is_empty(),
        "a quiet agent's reply is still refreshed"
    );
    assert_eq!(again[0].chunk.offset, 0);
    assert_eq!(said(&again, &stream_id("task-1")), first);

    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    // Once the reply is in the log nothing more is relayed: what is left is what was on its way.
    let _ = collect_for(&mut live, Duration::from_millis(300)).await;
    let late = collect_for(&mut live, Duration::from_millis(500)).await;
    assert!(late.is_empty(), "{late:?}");
    run.shutdown().await;
}

/// A wakeup that cannot carry live text.
#[derive(Clone)]
struct NoLive(orch_ports::memory::MemoryWakeup);

impl Wakeup for NoLive {
    async fn notify(&self, topic: orch_ports::Topic) -> Result<(), WakeupError> {
        self.0.notify(topic).await
    }
    fn subscribe(&self) -> BoxStream<'static, orch_ports::Topic> {
        self.0.subscribe()
    }
    fn capabilities(&self) -> WakeupCapabilities {
        WakeupCapabilities {
            push: true,
            live: false,
        }
    }
    async fn publish_live(&self, _: LiveText) -> Result<(), WakeupError> {
        panic!("a wakeup without live text is never asked to publish it")
    }
    fn subscribe_live(&self) -> BoxStream<'static, LiveText> {
        futures::stream::empty().boxed()
    }
}

#[tokio::test]
async fn a_wakeup_without_live_text_changes_nothing_about_the_log() {
    use orch_app::{App, AppConfig};
    use orch_ports::{PortSet, SystemClock};
    let w = World::new();
    let app = std::sync::Arc::new(
        App::new(
            PortSet {
                store: w.store.clone(),
                wakeup: NoLive(w.wakeup.clone()),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                registry: directory().fixed_registry(),
            },
            directory(),
            AppConfig::default(),
        )
        .unwrap(),
    );
    let token = tokio_util::sync::CancellationToken::new();
    let d = orch_app::Dispatcher::new(std::sync::Arc::clone(&app), fast(), "d1");
    let handle = tokio::spawn(d.run(token.clone()));
    let t = app
        .create_thread(
            &alice(),
            orch_app::NewThread {
                title: None,
                target: target("plain"),
                text: "stream tell me".to_owned(),
            },
        )
        .await
        .unwrap();
    eventually("done", || async {
        (app.get_thread(&alice(), t.id).await.unwrap().state == ThreadState::Done).then_some(())
    })
    .await;
    let ev = app.list_events(&alice(), t.id, 0, 500).await.unwrap();
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(&e.body, EventBody::AgentMessage(_)))
            .count(),
        1
    );
    token.cancel();
    handle.await.unwrap();
}

// ---- the feed ------------------------------------------------------------------------

fn piece(thread: ThreadId, id: &str, offset: u64, text: &str) -> LiveText {
    LiveText {
        thread,
        agent: orch_core::AgentId::new("plain"),
        chunk: LiveChunk {
            message_id: id.to_owned(),
            offset,
            text: text.to_owned(),
            end: LiveEnd::Open,
        },
    }
}

async fn next(feed: &mut BoxStream<'static, FeedItem>) -> FeedItem {
    tokio::time::timeout(Duration::from_secs(5), feed.next())
        .await
        .expect("the feed went quiet")
        .expect("the feed ended")
}

#[tokio::test]
async fn the_feed_is_the_events_with_the_threads_live_pieces_between_them() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "gate hold").await;
    let other = create(&app, &alice(), "plain", "gate hold").await;
    // The log so far: the user's message. A caught-up viewer.
    let mut feed = app.thread_feed(&alice(), t.id, 0).await.unwrap();
    let FeedItem::Event(first) = next(&mut feed).await else {
        panic!("the replay starts with the log");
    };
    assert_eq!(first.seq, 1);

    // Another thread's piece is not this feed's; this thread's is, in order.
    w.wakeup
        .publish_live(piece(other.id, "S", 0, "not for you"))
        .await
        .unwrap();
    w.wakeup
        .publish_live(piece(t.id, "S", 0, "Fib"))
        .await
        .unwrap();
    w.wakeup
        .publish_live(piece(t.id, "S", 3, "onacci"))
        .await
        .unwrap();
    let mut said = Vec::new();
    for _ in 0..2 {
        let FeedItem::Live(p) = next(&mut feed).await else {
            panic!("a live piece");
        };
        assert_eq!(p.thread, t.id);
        said.push((p.chunk.offset, p.chunk.text));
    }
    assert_eq!(said, [(0, "Fib".to_owned()), (3, "onacci".to_owned())]);

    // A new event of the log arrives on the same stream.
    let run = spawn_dispatcher(&app, fast(), "d1");
    w.agent.release_gate();
    let FeedItem::Event(e) = next(&mut feed).await else {
        panic!("an event");
    };
    assert!(e.seq > 1);
    run.shutdown().await;
}

#[tokio::test]
async fn no_piece_is_yielded_before_the_log_has_been_replayed_up_to_its_head() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let head = events(&app, &alice(), t.id).await.len() as i64;

    // The feed is opened (subscribed to live text), a piece is published before anything is read
    // from it: the replay of the thread's history is still to come. The piece waits; it is not
    // shown under whatever the replay has open.
    let mut feed = app.thread_feed(&alice(), t.id, 0).await.unwrap();
    w.wakeup
        .publish_live(piece(t.id, "S", 0, "early"))
        .await
        .unwrap();
    let mut seen_events = 0;
    while seen_events < head {
        match next(&mut feed).await {
            FeedItem::Event(_) => seen_events += 1,
            FeedItem::Live(p) => panic!("a piece during the replay: {p:?}"),
        }
    }
    // Caught up: the pieces are delivered, the one that waited and the one that comes now.
    w.wakeup
        .publish_live(piece(t.id, "S", 0, "late"))
        .await
        .unwrap();
    let mut texts = Vec::new();
    while !texts.contains(&"late".to_owned()) {
        let FeedItem::Live(p) = next(&mut feed).await else {
            panic!("a live piece");
        };
        texts.push(p.chunk.text);
    }
    assert!(texts.len() <= 2, "{texts:?}");
    run.shutdown().await;
}

#[tokio::test]
async fn a_feed_that_starts_at_the_head_is_caught_up_at_once() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "gate hold").await;
    let mut feed = app.thread_feed(&alice(), t.id, 1).await.unwrap();
    w.wakeup
        .publish_live(piece(t.id, "S", 0, "now"))
        .await
        .unwrap();
    let FeedItem::Live(p) = next(&mut feed).await else {
        panic!("a live piece");
    };
    assert_eq!(p.chunk.text, "now");
}

#[tokio::test]
async fn a_stranger_gets_no_feed_and_a_shutdown_ends_it() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "gate hold").await;
    assert!(app.thread_feed(&bob(), t.id, 0).await.is_err());
    let mut feed = app.thread_feed(&alice(), t.id, 0).await.unwrap();
    let _ = next(&mut feed).await;
    app.set_shutting_down();
    // Caught up and shutting down: the feed ends (within a poll), live text or not.
    let ended = tokio::time::timeout(Duration::from_secs(3), async {
        while feed.next().await.is_some() {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "the feed did not end when the process shut down"
    );
}
