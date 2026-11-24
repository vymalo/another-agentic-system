//! What this process is carrying right now: the dispatcher's slots in use and the open
//! server-sent-event subscribers (`dispatcher_slots_in_use`, `dispatcher_slots_configured`,
//! `sse_subscribers_open` of `GET /metrics`). They are per process (aggregate with `sum` over the
//! replicas) and name no person, thread, token or secret.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use orch_ports::Ports;

use super::App;

#[derive(Debug, Default)]
pub(crate) struct LoadGauges {
    slots_in_use: AtomicU64,
    slots_configured: AtomicU64,
    subscribers: AtomicU64,
}

/// The gauges of this process, as read at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadStats {
    /// Dispatcher workers busy with an outbox row (0 where no dispatcher runs).
    pub slots_in_use: u64,
    /// The dispatcher's configured concurrency (0 where no dispatcher runs).
    pub slots_configured: u64,
    /// Server-sent-event streams open now.
    pub subscribers: u64,
}

/// One open event stream, counted until this is dropped (the stream ends, or the client goes).
#[derive(Debug)]
pub struct SubscriberGuard(Arc<LoadGauges>);

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.0.subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<P: Ports> App<P> {
    /// The slots and the subscribers of this process now.
    pub fn load_stats(&self) -> LoadStats {
        let g = &self.load_gauges;
        LoadStats {
            slots_in_use: g.slots_in_use.load(Ordering::Relaxed),
            slots_configured: g.slots_configured.load(Ordering::Relaxed),
            subscribers: g.subscribers.load(Ordering::Relaxed),
        }
    }

    /// The dispatcher says how many of its `configured` slots are busy.
    pub fn set_dispatcher_slots(&self, in_use: usize, configured: usize) {
        let g = &self.load_gauges;
        g.slots_in_use
            .store(u64::try_from(in_use).unwrap_or(u64::MAX), Ordering::Relaxed);
        g.slots_configured.store(
            u64::try_from(configured).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    /// Counts one open event stream until the guard is dropped: hold it in the response body.
    #[must_use]
    pub fn open_subscriber(&self) -> SubscriberGuard {
        self.load_gauges.subscribers.fetch_add(1, Ordering::Relaxed);
        SubscriberGuard(Arc::clone(&self.load_gauges))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subscriber_gauge_follows_guards_opening_and_dropping() {
        let gauges = Arc::new(LoadGauges::default());
        let open = |g: &Arc<LoadGauges>| {
            g.subscribers.fetch_add(1, Ordering::Relaxed);
            SubscriberGuard(Arc::clone(g))
        };
        let mut guards: Vec<_> = (0..3).map(|_| open(&gauges)).collect();
        assert_eq!(gauges.subscribers.load(Ordering::Relaxed), 3);
        drop(guards.pop());
        assert_eq!(gauges.subscribers.load(Ordering::Relaxed), 2);
        drop(guards);
        assert_eq!(gauges.subscribers.load(Ordering::Relaxed), 0);
    }
}
