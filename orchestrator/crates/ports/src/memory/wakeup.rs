use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::LiveText;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{Topic, Wakeup, WakeupCapabilities, WakeupError};

/// In-process wakeup over tokio broadcast channels: one for the hints and one for live text (so
/// a flood of one cannot push the other out). Clones share the channels, so two app instances
/// built from clones wake each other like two replicas on one database.
#[derive(Debug, Clone)]
pub struct MemoryWakeup {
    tx: broadcast::Sender<Topic>,
    live: broadcast::Sender<LiveText>,
}

impl Default for MemoryWakeup {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryWakeup {
    /// New channels (capacity 1024 each; a lagging subscriber gets [`Topic::Resync`], and loses
    /// live pieces without notice).
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        let (live, _) = broadcast::channel(1024);
        MemoryWakeup { tx, live }
    }
}

impl Wakeup for MemoryWakeup {
    async fn notify(&self, topic: Topic) -> Result<(), WakeupError> {
        // No subscriber is not an error: notifications are hints.
        let _ = self.tx.send(topic);
        Ok(())
    }

    fn subscribe(&self) -> BoxStream<'static, Topic> {
        BroadcastStream::new(self.tx.subscribe())
            .map(|item| match item {
                Ok(topic) => topic,
                Err(BroadcastStreamRecvError::Lagged(_)) => Topic::Resync,
            })
            .boxed()
    }

    fn capabilities(&self) -> WakeupCapabilities {
        WakeupCapabilities {
            push: true,
            live: true,
        }
    }

    async fn publish_live(&self, live: LiveText) -> Result<(), WakeupError> {
        // Best effort, like every hint: nobody listening is not an error.
        let _ = self.live.send(live);
        Ok(())
    }

    fn subscribe_live(&self) -> BoxStream<'static, LiveText> {
        // A lagging subscriber loses pieces silently (the sender repeats the text so far).
        BroadcastStream::new(self.live.subscribe())
            .filter_map(|item| std::future::ready(item.ok()))
            .boxed()
    }
}
