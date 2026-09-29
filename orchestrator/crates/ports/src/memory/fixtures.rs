use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use jiff::{SignedDuration, Timestamp};
use orch_core::Releases;
use uuid::Uuid;

use crate::{Clock, IdGen};

/// A settable clock. Clones share the same time.
#[derive(Debug, Clone)]
pub struct FixedClock(Arc<Mutex<Timestamp>>);

impl FixedClock {
    /// A clock frozen at `at`.
    pub fn new(at: Timestamp) -> Self {
        FixedClock(Arc::new(Mutex::new(at)))
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: std::time::Duration) {
        let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ok(next) = now.checked_add(SignedDuration::try_from(by).unwrap_or_default()) {
            *now = next;
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Deterministic, increasing ids (`00000000-0000-7000-8000-<counter>`). Clones share the counter.
#[derive(Debug, Clone, Default)]
pub struct SeqIds(Arc<AtomicU64>);

impl IdGen for SeqIds {
    fn new_id(&self) -> Uuid {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0000 | u128::from(n))
    }
}

/// Releases as advertised by a release-channels agent card: channels `stable` and `staging`.
pub fn sample_releases() -> Releases {
    Releases {
        default_channel: "stable".to_owned(),
        channels: BTreeMap::from([
            ("stable".to_owned(), "rev-1".to_owned()),
            ("staging".to_owned(), "rev-2".to_owned()),
        ]),
        revisions: Some(vec!["rev-1".to_owned(), "rev-2".to_owned()]),
    }
}
