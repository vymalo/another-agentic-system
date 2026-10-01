use std::time::Duration;

use futures::StreamExt;
use orch_core::{AgentId, LiveChunk, LiveEnd, LiveText, MAX_LIVE_PIECE_BYTES, ThreadId};
use uuid::Uuid;

use crate::{Topic, Wakeup};

async fn expect<S: futures::Stream<Item = Topic> + Unpin>(stream: &mut S, want: &Topic) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let got = tokio::time::timeout(remaining, stream.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {want:?}"));
        match got {
            Some(t) if &t == want => return,
            // Other hints (including Resync) are allowed; keep waiting for ours.
            Some(_) => {}
            None => panic!("subscription ended"),
        }
    }
}

/// A subscriber created before `notify(Thread(id))` receives it.
pub async fn delivers_thread_topic<W: Wakeup>(wakeup: W) {
    let id = ThreadId(Uuid::from_u128(0x7000_8000_0000_0000_0000_0000_0000_0042));
    let mut sub = wakeup.subscribe();
    // Give listener-based implementations time to attach.
    tokio::time::sleep(Duration::from_millis(300)).await;
    wakeup.notify(Topic::Thread(id)).await.unwrap();
    expect(&mut sub, &Topic::Thread(id)).await;
}

/// Every subscriber sees a topic.
pub async fn delivers_outbox_to_every_subscriber<W: Wakeup>(wakeup: W) {
    let mut a = wakeup.subscribe();
    let mut b = wakeup.subscribe();
    tokio::time::sleep(Duration::from_millis(300)).await;
    wakeup.notify(Topic::Outbox).await.unwrap();
    expect(&mut a, &Topic::Outbox).await;
    expect(&mut b, &Topic::Outbox).await;
}

/// The inbox topic is delivered like the others (it is its own channel in a listener-based
/// implementation).
pub async fn delivers_inbox_topic<W: Wakeup>(wakeup: W) {
    let mut sub = wakeup.subscribe();
    tokio::time::sleep(Duration::from_millis(300)).await;
    wakeup.notify(Topic::Inbox).await.unwrap();
    expect(&mut sub, &Topic::Inbox).await;
}

/// A piece of the test's own thread. Every case uses its own thread id, because a listener-based
/// implementation shares one channel per database: pieces of tests running beside this one are
/// allowed to show up, and are skipped (as other topics are by `expect`).
fn live(thread: u128, message_id: &str, offset: u64, text: &str, end: LiveEnd) -> LiveText {
    LiveText {
        thread: ThreadId(Uuid::from_u128(thread)),
        agent: AgentId::new("coder"),
        chunk: LiveChunk {
            message_id: message_id.to_string(),
            offset,
            text: text.to_string(),
            end,
        },
    }
}

fn thread_of(id: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(id))
}

async fn next_live<S: futures::Stream<Item = LiveText> + Unpin>(
    stream: &mut S,
    thread: u128,
) -> LiveText {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let got = tokio::time::timeout(remaining, stream.next())
            .await
            .expect("timed out waiting for live text")
            .expect("live subscription ended");
        if got.thread == thread_of(thread) {
            return got;
        }
    }
}

/// A subscriber created before `publish_live` receives the piece, unchanged.
pub async fn delivers_live_text<W: Wakeup>(wakeup: W) {
    assert!(wakeup.capabilities().live, "this suite needs live text");
    let mut sub = wakeup.subscribe_live();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let piece = live(
        0x71,
        "S-1",
        3,
        "onacci \u{e9}\u{1f980} \"q\"\n",
        LiveEnd::Open,
    );
    wakeup.publish_live(piece.clone()).await.unwrap();
    assert_eq!(next_live(&mut sub, 0x71).await, piece);
}

/// Every subscriber sees every piece, and in the order they were published.
pub async fn live_reaches_every_subscriber<W: Wakeup>(wakeup: W) {
    let mut a = wakeup.subscribe_live();
    let mut b = wakeup.subscribe_live();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let pieces = [
        live(0x72, "S-2", 0, "Fib", LiveEnd::Open),
        live(0x72, "S-2", 3, "onacci ", LiveEnd::Open),
        live(0x72, "S-3", 0, "", LiveEnd::Abandoned),
        live(0x72, "S-2", 10, "in Rust.", LiveEnd::Last),
    ];
    for piece in &pieces {
        wakeup.publish_live(piece.clone()).await.unwrap();
    }
    for piece in &pieces {
        assert_eq!(&next_live(&mut a, 0x72).await, piece);
        assert_eq!(&next_live(&mut b, 0x72).await, piece);
    }
}

/// Live text and hints are separate: a piece is not a topic and a topic is not a piece, and
/// neither holds the other up.
pub async fn live_does_not_disturb_topics<W: Wakeup>(wakeup: W) {
    let mut topics = wakeup.subscribe();
    let mut pieces = wakeup.subscribe_live();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let piece = live(0x74, "S-4", 0, "hello", LiveEnd::Open);
    wakeup.publish_live(piece.clone()).await.unwrap();
    wakeup.notify(Topic::Outbox).await.unwrap();
    expect(&mut topics, &Topic::Outbox).await;
    assert_eq!(next_live(&mut pieces, 0x74).await, piece);
    // Nothing more of ours arrives: the hint did not become a piece.
    let quiet = tokio::time::Instant::now() + Duration::from_millis(300);
    while let Ok(Some(extra)) = tokio::time::timeout_at(quiet, pieces.next()).await {
        assert_ne!(extra.thread, piece.thread, "a hint showed up as live text");
    }
}

/// A piece of the largest size the relay sends (`MAX_LIVE_PIECE_BYTES`), made of what a JSON
/// codec has to escape, is accepted. An implementation may split it to fit its transport, so what
/// is checked is what arrives in the end: the same thread, agent and stream id, pieces that follow
/// one another by byte offset, joined into the same text, and the end on the last one only.
pub async fn live_accepts_a_full_piece<W: Wakeup>(wakeup: W) {
    let mut sub = wakeup.subscribe_live();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let unit = "q\"\\\n\t\u{0}\u{e9}\u{4e2d}\u{1f980}";
    let mut text = unit.repeat(MAX_LIVE_PIECE_BYTES / unit.len());
    while text.len() < MAX_LIVE_PIECE_BYTES - 4 {
        text.push('x');
    }
    assert!(text.len() <= MAX_LIVE_PIECE_BYTES);
    let sent = live(0x75, "S-5", 100, &text, LiveEnd::Last);
    wakeup.publish_live(sent.clone()).await.unwrap();

    let mut got = String::new();
    let mut next_offset = 100u64;
    while got.len() < text.len() {
        let piece = next_live(&mut sub, 0x75).await;
        assert_eq!(piece.agent, sent.agent);
        assert_eq!(piece.chunk.message_id, "S-5");
        assert_eq!(piece.chunk.offset, next_offset, "pieces follow one another");
        next_offset += piece.chunk.text.len() as u64;
        got.push_str(&piece.chunk.text);
        if got.len() < text.len() {
            assert_eq!(piece.chunk.end, LiveEnd::Open, "only the last piece ends");
        } else {
            assert_eq!(piece.chunk.end, LiveEnd::Last);
        }
    }
    assert_eq!(got, text);
}
