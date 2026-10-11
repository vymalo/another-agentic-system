//! What becomes of an access tier echo that is not logged (ADR 0061, `access-tier/v1`): the
//! counter `access_tier_echoes_dropped_total`, for this process.
//!
//! The agent says which tier it works under in its task's metadata. The orchestrator reads it as
//! one of four words; anything else is refused by the A2A adapter (`AgentUpdate::TierRejected`) and
//! never reaches the core. It never fails a task. The counter names no thread and no agent; a debug
//! line says which thread.

use std::sync::atomic::{AtomicU64, Ordering};

use orch_core::ThreadId;
use orch_ports::Ports;

use super::App;

#[derive(Debug, Default)]
pub(crate) struct EchoCounters {
    dropped: AtomicU64,
}

/// The counter of access tier echoes this process did not log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierStats {
    /// `access_tier_echoes_dropped_total`: echoes that were not one of the four tiers.
    pub echoes_dropped: u64,
}

impl<P: Ports> App<P> {
    /// The counter of access tier echoes this process did not log.
    pub fn tier_stats(&self) -> TierStats {
        TierStats {
            echoes_dropped: self.echo_counters.dropped.load(Ordering::Relaxed),
        }
    }

    /// An echo of `thread`'s agent was not one of the four tiers, and was dropped.
    pub(crate) fn tier_echo_dropped(&self, thread: ThreadId) {
        self.echo_counters.dropped.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(%thread, "an access tier echo that is not one of the four tiers was dropped");
    }
}
