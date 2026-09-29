use std::time::Duration;

use futures::StreamExt;
use orch_core::ThreadId;
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
