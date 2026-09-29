use jiff::{Timestamp, Unit};
use uuid::Uuid;

/// Source of "now". Injected so tests are deterministic.
pub trait Clock: Send + Sync + 'static {
    /// The current instant.
    fn now(&self) -> Timestamp;
}

/// Source of fresh identifiers. Injected so tests are deterministic.
pub trait IdGen: Send + Sync + 'static {
    /// A new unique id. Implementations used for thread ids must be time-ordered (UUIDv7).
    fn new_id(&self) -> Uuid;
}

/// The wall clock, rounded to microseconds (Postgres precision) so every store agrees.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let now = Timestamp::now();
        now.round(Unit::Microsecond).unwrap_or(now)
    }
}

/// UUIDv7 identifiers (time-ordered).
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidV7Ids;

impl IdGen for UuidV7Ids {
    fn new_id(&self) -> Uuid {
        Uuid::now_v7()
    }
}
