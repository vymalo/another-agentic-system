use std::future::Future;

use futures::stream::BoxStream;
use orch_core::{BoxError, Classify, ErrorClass, ThreadId};

/// What changed. Notifications are hints: consumers always re-read the store and also poll.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Topic {
    /// The thread's event log or state changed.
    Thread(ThreadId),
    /// There may be new outbox rows to claim.
    Outbox,
    /// There may be new inbox rows to claim (a report was received, or a parked one re-armed).
    /// A timer that becomes due is not announced: the inbox worker polls for those.
    Inbox,
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
#[non_exhaustive]
pub enum WakeupError {
    /// The channel could not be reached; callers fall back to polling.
    #[error("wakeup unavailable")]
    Unavailable {
        /// The driver's error.
        #[source]
        source: BoxError,
    },
}

impl WakeupError {
    /// The channel is down.
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        WakeupError::Unavailable {
            source: source.into(),
        }
    }
}

impl Classify for WakeupError {
    fn class(&self) -> ErrorClass {
        match self {
            WakeupError::Unavailable { .. } => ErrorClass::Transient,
        }
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn class_table() {
        let e = WakeupError::unavailable(std::io::Error::other("listener closed"));
        let expected = match &e {
            WakeupError::Unavailable { .. } => ErrorClass::Transient,
        };
        assert_eq!(e.class(), expected);
        assert!(e.is_retryable());
        assert!(!e.to_string().contains("listener closed"));
    }
}
