use std::future::Future;

use futures::stream::BoxStream;
use orch_core::ThreadId;

/// What changed. Notifications are hints: consumers always re-read the store and also poll.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Topic {
    /// The thread's event log or state changed.
    Thread(ThreadId),
    /// There may be new outbox rows to claim.
    Outbox,
    /// The subscriber missed notifications: re-read everything.
    Resync,
}

/// What a [`Wakeup`] implementation can do (ADR 0009 rule 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeupCapabilities {
    /// Notifications are pushed to subscribers (as opposed to poll-only).
    pub push: bool,
}

/// Wakeup failure.
#[derive(Debug, thiserror::Error)]
pub enum WakeupError {
    /// The channel could not be reached; callers fall back to polling.
    #[error("wakeup unavailable: {0}")]
    Unavailable(String),
}

/// Notify/listen hints between processes (Postgres `LISTEN/NOTIFY`, in-memory broadcast, …).
pub trait Wakeup: Send + Sync + 'static {
    /// Publishes a hint to every subscriber, in every process.
    fn notify(&self, topic: Topic) -> impl Future<Output = Result<(), WakeupError>> + Send;
    /// A stream of hints from now on. A lagging subscriber receives [`Topic::Resync`].
    fn subscribe(&self) -> BoxStream<'static, Topic>;
    /// What this implementation supports.
    fn capabilities(&self) -> WakeupCapabilities;
}
