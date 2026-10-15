//! The rate limit of `POST /api/user-tool-servers/{id}/authorize` (ADR 0046, slice 4c), per person.
//!
//! An authorization makes the broker **fetch from the network, URLs that a stranger's metadata
//! names** (the protected resource metadata of the person's server, then its authorization server's
//! metadata), and register a client at that server: a person who calls it in a loop turns the
//! orchestrator into a request generator aimed at hosts of their choosing (ADR 0046, slice 2's open
//! point 2: "rate-limits `begin_oauth`"). So each person has a **token bucket**: a burst of
//! [`AuthorizeLimits::burst`] calls, refilled one call every [`AuthorizeLimits::refill`], which with
//! the defaults is ten a minute.
//!
//! - **Per person, in this process.** The bucket is in memory, so with several replicas a person may
//!   authorize that many times as many per minute. The limit is a brake on abuse and not a quota; a
//!   shared one would need a store of its own, and the pending authorizations the callback needs are
//!   already bounded at one per server of each person (the table does not grow with calls).
//! - **Every call counts**, one that is refused for another reason after the limiter included: the
//!   limiter sits in front of the application, and a call that is refused for a missing permission or
//!   an unknown server costs the same token as one that reaches the broker.
//! - **The table of persons is bounded** ([`MAX_PEOPLE`]): a person with nothing to remember (a full
//!   bucket) is forgotten, and when the table is still full a person it has not met is refused until
//!   one is. Only a signed-in person with `tools.user.add` gets a bucket made, but this is not left
//!   to depend on it.
//!
//! Time is `tokio::time::Instant`, so a test with paused time moves it. The counts are in thousandths
//! of a call and whole milliseconds, never floating point.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use orch_core::UserId;
use tokio::time::Instant;

use crate::limiter::Limited;

/// The most persons the limiter remembers at once.
pub const MAX_PEOPLE: usize = 4096;

/// A call is 1000 of these.
const MILLI: i64 = 1000;

/// The limit of `authorize`: a burst, and how often a call is given back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizeLimits {
    /// Calls a person may make at once (the bucket's size), at least 1.
    pub burst: u32,
    /// How long it takes to give one call back, at least a millisecond.
    pub refill: Duration,
}

impl Default for AuthorizeLimits {
    /// Ten calls, and one more every six seconds: ten a minute.
    fn default() -> Self {
        AuthorizeLimits {
            burst: 10,
            refill: Duration::from_secs(6),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    /// Thousandths of a call; never below zero.
    milli: i64,
    last: Instant,
}

/// The limiter of `authorize`. See the module's docs.
#[derive(Debug)]
pub struct AuthorizeLimiter {
    capacity: i64,
    /// Milliseconds to give one call back.
    refill_ms: i64,
    people: Mutex<HashMap<UserId, Bucket>>,
}

impl AuthorizeLimiter {
    /// A limiter with these limits (a burst of 0 is 1, a refill under a millisecond is a
    /// millisecond).
    pub fn new(limits: AuthorizeLimits) -> Self {
        AuthorizeLimiter {
            capacity: i64::from(limits.burst.max(1)) * MILLI,
            refill_ms: i64::try_from(limits.refill.as_millis())
                .unwrap_or(i64::MAX)
                .max(1),
            people: Mutex::new(HashMap::new()),
        }
    }

    /// Gives back what time has earned.
    fn refilled(&self, mut bucket: Bucket, now: Instant) -> Bucket {
        let elapsed = now.saturating_duration_since(bucket.last);
        let ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
        // `MILLI` thousandths for `refill_ms` milliseconds
        let earned = ms.saturating_mul(MILLI) / self.refill_ms;
        bucket.milli = bucket.milli.saturating_add(earned).min(self.capacity);
        // keep the remainder of a millisecond: move `last` only by the time that was turned into calls
        let spent_ms = earned.saturating_mul(self.refill_ms) / MILLI;
        bucket.last = bucket
            .last
            .checked_add(Duration::from_millis(u64::try_from(spent_ms).unwrap_or(0)))
            .unwrap_or(now);
        bucket
    }

    /// Takes one call of `who`.
    ///
    /// # Errors
    /// [`Limited`] with the whole seconds to wait for the next call (at least one) when the person's
    /// bucket is empty, or when the table of persons is full and `who` has no bucket in it.
    pub fn admit(&self, who: &UserId) -> Result<(), Limited> {
        let now = Instant::now();
        let mut people = self.people.lock().unwrap_or_else(PoisonError::into_inner);
        if !people.contains_key(who) && people.len() >= MAX_PEOPLE {
            // forget the persons who have nothing to remember: their bucket is full again
            people.retain(|_, bucket| self.refilled(*bucket, now).milli < self.capacity);
            if people.len() >= MAX_PEOPLE {
                return Err(Limited {
                    retry_after_secs: self.wait(0),
                });
            }
        }
        let bucket = people.entry(who.clone()).or_insert(Bucket {
            milli: self.capacity,
            last: now,
        });
        let mut current = self.refilled(*bucket, now);
        if current.milli < MILLI {
            let wait = self.wait(current.milli);
            *bucket = current;
            return Err(Limited {
                retry_after_secs: wait,
            });
        }
        current.milli -= MILLI;
        *bucket = current;
        Ok(())
    }

    /// Whole seconds until one call is there, at least one.
    fn wait(&self, milli: i64) -> u64 {
        // thousandths of a call missing, at `refill_ms` milliseconds a call, up to a whole second
        let missing = u64::try_from((MILLI - milli).max(0)).unwrap_or(0);
        let refill_ms = u64::try_from(self.refill_ms).unwrap_or(u64::MAX);
        let ms = missing.saturating_mul(refill_ms).div_ceil(1000);
        ms.div_ceil(1000).max(1)
    }

    /// How many persons have a bucket now.
    pub fn people(&self) -> usize {
        self.people
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn person(n: usize) -> UserId {
        UserId::new(&format!("p{n}@example.test"))
    }

    fn limits(burst: u32, refill_secs: u64) -> AuthorizeLimits {
        AuthorizeLimits {
            burst,
            refill: Duration::from_secs(refill_secs),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ten_a_minute_by_default_a_burst_and_then_one_every_six_seconds() {
        let limiter = AuthorizeLimiter::new(AuthorizeLimits::default());
        let alice = person(1);
        for n in 0..10 {
            limiter.admit(&alice).unwrap_or_else(|_| panic!("call {n}"));
        }
        let refused = limiter.admit(&alice).unwrap_err();
        assert_eq!(
            refused.retry_after_secs, 6,
            "one call is given back in six seconds"
        );
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(limiter.admit(&alice).is_err(), "not yet");
        tokio::time::advance(Duration::from_secs(1)).await;
        limiter.admit(&alice).unwrap();
        assert!(limiter.admit(&alice).is_err(), "and one only");
        // a minute is ten calls
        tokio::time::advance(Duration::from_secs(60)).await;
        for _ in 0..10 {
            limiter.admit(&alice).unwrap();
        }
        assert!(limiter.admit(&alice).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_person_is_limited_and_another_is_not() {
        let limiter = AuthorizeLimiter::new(limits(2, 60));
        let (alice, bob) = (person(1), person(2));
        limiter.admit(&alice).unwrap();
        limiter.admit(&alice).unwrap();
        assert!(limiter.admit(&alice).is_err());
        limiter.admit(&bob).unwrap();
        limiter.admit(&bob).unwrap();
        assert!(limiter.admit(&bob).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn the_wait_shortens_as_time_passes_and_is_at_least_a_second() {
        let limiter = AuthorizeLimiter::new(limits(1, 10));
        let alice = person(1);
        limiter.admit(&alice).unwrap();
        assert_eq!(limiter.admit(&alice).unwrap_err().retry_after_secs, 10);
        tokio::time::advance(Duration::from_millis(9_500)).await;
        assert_eq!(limiter.admit(&alice).unwrap_err().retry_after_secs, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_hour_of_silence_is_a_burst_and_no_more() {
        let limiter = AuthorizeLimiter::new(limits(3, 6));
        let alice = person(1);
        tokio::time::advance(Duration::from_secs(3_600)).await;
        for _ in 0..3 {
            limiter.admit(&alice).unwrap();
        }
        assert!(limiter.admit(&alice).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn the_table_cannot_be_grown_by_more_persons_than_it_holds() {
        let limiter = AuthorizeLimiter::new(limits(2, 60));
        // every person uses one call, so none is idle: the table fills
        for n in 0..MAX_PEOPLE {
            limiter.admit(&person(n)).unwrap();
        }
        assert_eq!(limiter.people(), MAX_PEOPLE);
        assert!(
            limiter.admit(&person(MAX_PEOPLE)).is_err(),
            "a person the limiter has not met is refused while it is full"
        );
        assert_eq!(limiter.people(), MAX_PEOPLE);
        // a person it knows is still limited as before
        limiter.admit(&person(0)).unwrap();
        assert!(limiter.admit(&person(0)).is_err());
        // when the buckets are full again, the idle are forgotten and the newcomer is let in
        tokio::time::advance(Duration::from_secs(300)).await;
        limiter.admit(&person(MAX_PEOPLE)).unwrap();
        assert!(limiter.people() <= MAX_PEOPLE);
    }

    #[tokio::test(start_paused = true)]
    async fn the_extremes_of_the_limits_are_made_sane() {
        let limiter = AuthorizeLimiter::new(AuthorizeLimits {
            burst: 0,
            refill: Duration::ZERO,
        });
        let alice = person(1);
        limiter.admit(&alice).unwrap();
        assert!(limiter.admit(&alice).is_err(), "a burst of 0 is 1");
        tokio::time::advance(Duration::from_millis(2)).await;
        limiter.admit(&alice).unwrap();
    }
}
