use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{Topic, Wakeup, WakeupCapabilities, WakeupError};

/// In-process wakeup over a tokio broadcast channel. Clones share the channel, so two app
/// instances built from clones wake each other like two replicas on one database.
#[derive(Debug, Clone)]
pub struct MemoryWakeup {
    tx: broadcast::Sender<Topic>,
}

impl Default for MemoryWakeup {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryWakeup {
    /// A new channel (capacity 1024; a lagging subscriber gets [`Topic::Resync`]).
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        MemoryWakeup { tx }
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
        WakeupCapabilities { push: true }
    }
}
