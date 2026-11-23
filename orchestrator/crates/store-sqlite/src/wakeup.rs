//! [`Wakeup`] for a store with one process: hints and live text over in-process channels.
//!
//! One file belongs to one process (ADR 0076 decision 6), so there is no other process to tell and
//! no `LISTEN`/`NOTIFY` to use. A hint is a latency optimisation: consumers re-read the store and
//! poll, so a subscriber that lags is told [`Topic::Resync`] and loses nothing that matters. Live
//! text is a view of a reply still being written and is lost silently by a lagging subscriber.
//!
//! The channels are not in the file: a hint does not outlive the process, and none is needed to.

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::LiveText;
use orch_ports::{Topic, Wakeup, WakeupCapabilities, WakeupError};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// What each channel holds before a slow subscriber starts to lag.
const CAPACITY: usize = 1024;

/// In-process [`Wakeup`]: one broadcast channel for the hints and one for live text, so a flood of
/// one cannot push the other out. Clones share the channels.
#[derive(Debug, Clone)]
pub struct SqliteWakeup {
    hints: broadcast::Sender<Topic>,
    live: broadcast::Sender<LiveText>,
}

impl Default for SqliteWakeup {
    fn default() -> Self {
        Self::new()
    }
}

impl SqliteWakeup {
    /// New channels.
    #[must_use]
    pub fn new() -> Self {
        let (hints, _) = broadcast::channel(CAPACITY);
        let (live, _) = broadcast::channel(CAPACITY);
        Self { hints, live }
    }
}

impl Wakeup for SqliteWakeup {
    async fn notify(&self, topic: Topic) -> Result<(), WakeupError> {
        // Nobody listening is not an error: a hint is never the truth.
        let _ = self.hints.send(topic);
        Ok(())
    }

    fn subscribe(&self) -> BoxStream<'static, Topic> {
        BroadcastStream::new(self.hints.subscribe())
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
        let _ = self.live.send(live);
        Ok(())
    }

    fn subscribe_live(&self) -> BoxStream<'static, LiveText> {
        BroadcastStream::new(self.live.subscribe())
            .filter_map(|item| std::future::ready(item.ok()))
            .boxed()
    }
}
