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

    /// 16 unpredictable bytes: the nonce a share link is built on (ADR 0040). A capability, so
    /// from the operating system's random source in every implementation that is not a test's.
    fn new_token_bytes(&self) -> [u8; 16];
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

    fn new_token_bytes(&self) -> [u8; 16] {
        let mut bytes = [0_u8; 16];
        // The operating system's source failing is not something to go on from: a nonce that is
        // not random would be a link anybody could guess.
        #[allow(clippy::expect_used)]
        getrandom::fill(&mut bytes).expect("the operating system's random source");
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bytes_are_not_repeated_and_not_zero() {
        let ids = UuidV7Ids;
        let draws: Vec<[u8; 16]> = (0..64).map(|_| ids.new_token_bytes()).collect();
        for (i, a) in draws.iter().enumerate() {
            assert_ne!(*a, [0; 16]);
            assert!(draws[i + 1..].iter().all(|b| b != a), "a nonce came twice");
        }
    }
}
