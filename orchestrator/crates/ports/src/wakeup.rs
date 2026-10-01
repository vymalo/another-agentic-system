use std::future::Future;

use futures::stream::BoxStream;
use orch_core::{BoxError, Classify, ErrorClass, LiveText, ThreadId};

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
    /// Live text ([`Wakeup::publish_live`], [`Wakeup::subscribe_live`]) is carried. An
    /// implementation without it accepts and drops what is published, and its subscription never
    /// yields: nothing is shown live, the log is unchanged (ADR 0027).
    pub live: bool,
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
    /// A message cannot be sent because it does not fit the channel. Retrying the same message
    /// cannot help; the caller drops it (live text is best effort).
    #[error("wakeup message too large")]
    PayloadTooLarge {
        /// The size the message came to, in bytes.
        len: usize,
        /// The most the channel carries, in bytes.
        limit: usize,
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
            WakeupError::PayloadTooLarge { .. } => ErrorClass::Invalid,
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

    /// Publishes a piece of live text to every subscriber of [`subscribe_live`](Self::subscribe_live),
    /// in every process (ADR 0027).
    ///
    /// **Best effort, and not a topic.** Live text is a view of a reply that is still being
    /// written; the log's final message replaces it. So it is never stored, never retried by the
    /// port, and a failure only means the viewers see the words later (at the next refresh, or in
    /// the final message). A piece of at most [`MAX_LIVE_PIECE_BYTES`](orch_core::MAX_LIVE_PIECE_BYTES)
    /// of text must be accepted; an implementation whose transport is smaller splits it, in order,
    /// and answers [`WakeupError::PayloadTooLarge`] only for a message it cannot send at all.
    fn publish_live(&self, live: LiveText) -> impl Future<Output = Result<(), WakeupError>> + Send;

    /// A stream of live text from now on. Best effort: a lagging subscriber silently loses pieces
    /// (there is no [`Topic::Resync`] for it: the sender repeats the text so far from time to time,
    /// and the final message is in the log). It never ends while the implementation lives.
    fn subscribe_live(&self) -> BoxStream<'static, LiveText>;
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
            WakeupError::PayloadTooLarge { .. } => ErrorClass::Invalid,
        };
        assert_eq!(e.class(), expected);
        assert!(e.is_retryable());
        assert!(!e.to_string().contains("listener closed"));

        let big = WakeupError::PayloadTooLarge {
            len: 9000,
            limit: 7900,
        };
        assert_eq!(big.class(), ErrorClass::Invalid);
        assert!(!big.is_retryable());
    }
}
