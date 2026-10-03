//! The rate limit of the public routes (ADR 0040, section 10).
//!
//! The public routes are open to the world and return a person's data, so they are limited before
//! `sharing.mode: public` is used. Per process (an edge-side limit is better when the client's
//! address can be trusted, which behind the production ingress is *unverified*; this one does not
//! depend on it, because it is per link, not per address):
//!
//! * a **token bucket for all links together** and one **per link**: every request takes one token
//!   from each, and a request is refused with `429` and `Retry-After` when either is empty;
//! * **a failure counts against the shared bucket**: an answer of 404 costs [`FAILURE_EXTRA`] more
//!   tokens from the bucket of all links, so guessing tokens is throttled however many different
//!   ones are tried (a guess has no link of its own to be charged to);
//! * a **ceiling on open streams**, per link and in all, held by a [`StreamPermit`] that the stream
//!   carries and gives back when it is dropped.
//!
//! The numbers ([`PublicLimits`]) are a starting point, *unverified* under load. Buckets count in
//! thousandths of a token with whole milliseconds, never in floating point. The per-link table is
//! bounded: a link that has nothing to remember (its bucket is full and it has no stream) is
//! forgotten, and when the table is still full a new link is charged to the shared bucket alone, so
//! a flood of invented tokens cannot grow it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};
use tokio::time::Instant;

/// The extra tokens a request that was answered 404 costs the shared bucket.
pub const FAILURE_EXTRA: u32 = 4;

/// Most links the limiter remembers at once.
const MAX_LINKS: usize = 4096;

/// A token is 1000 of these.
const MILLI: i64 = 1000;

/// The limits of the public routes: the ADR's starting numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicLimits {
    /// Requests a second for one link.
    pub per_link_per_second: u32,
    /// Requests a second for all links together.
    pub total_per_second: u32,
    /// Open streams for one link.
    pub streams_per_link: u32,
    /// Open streams for all links together.
    pub streams_total: u32,
}

impl Default for PublicLimits {
    fn default() -> Self {
        PublicLimits {
            per_link_per_second: 10,
            total_per_second: 100,
            streams_per_link: 5,
            streams_total: 50,
        }
    }
}

/// What identifies a link to the limiter: 16 bytes of the SHA-256 of the token as it came in. The
/// token itself is never kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkKey([u8; 16]);

impl LinkKey {
    /// The key of the token `token` (whatever it is: a guess has a key too).
    pub fn of(token: &str) -> LinkKey {
        let digest = Sha256::digest(token.as_bytes());
        let mut key = [0_u8; 16];
        key.copy_from_slice(&digest[..16]);
        LinkKey(key)
    }
}

/// Why a request was not let through: how long to wait, in whole seconds, at least one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limited {
    /// For `Retry-After`.
    pub retry_after_secs: u64,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    /// Thousandths of a token; below zero is a debt.
    milli: i64,
    capacity: i64,
    /// Thousandths of a token added per millisecond, which is the rate in tokens a second.
    per_ms: i64,
    last: Instant,
}

impl Bucket {
    fn full(rate_per_second: u32, now: Instant) -> Bucket {
        let capacity = i64::from(rate_per_second) * MILLI;
        Bucket {
            milli: capacity,
            capacity,
            per_ms: i64::from(rate_per_second),
            last: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last);
        let ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
        self.milli = self
            .milli
            .saturating_add(ms.saturating_mul(self.per_ms))
            .min(self.capacity);
        self.last = now;
    }

    fn is_full(&self) -> bool {
        self.milli >= self.capacity
    }

    /// Whole seconds until one token is there, at least one.
    fn wait(&self) -> u64 {
        let missing = (MILLI - self.milli).max(0);
        let ms = (missing + self.per_ms - 1) / self.per_ms.max(1);
        u64::try_from(ms).unwrap_or(1).div_ceil(1000).max(1)
    }
}

#[derive(Debug)]
struct Link {
    bucket: Bucket,
    streams: u32,
}

#[derive(Debug)]
struct State {
    total: Bucket,
    links: HashMap<LinkKey, Link>,
    streams: u32,
}

/// The limiter of the public routes.
#[derive(Debug)]
pub struct PublicLimiter {
    limits: PublicLimits,
    state: Mutex<State>,
}

impl PublicLimiter {
    /// A limiter with these limits.
    pub fn new(limits: PublicLimits) -> Arc<PublicLimiter> {
        Arc::new(PublicLimiter {
            limits,
            state: Mutex::new(State {
                total: Bucket::full(limits.total_per_second, Instant::now()),
                links: HashMap::new(),
                streams: 0,
            }),
        })
    }

    /// The limits.
    pub fn limits(&self) -> PublicLimits {
        self.limits
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the room of one request for `link`.
    ///
    /// # Errors
    /// [`Limited`] when the bucket of all links, or the link's own, has no token.
    pub fn admit(&self, link: LinkKey) -> Result<(), Limited> {
        let now = Instant::now();
        let mut state = self.lock();
        state.total.refill(now);
        if state.total.milli < MILLI {
            return Err(Limited {
                retry_after_secs: state.total.wait(),
            });
        }
        // The link's own bucket, when the table has room for it; when it has not, the request is
        // charged to the shared bucket alone.
        if let Some(entry) = self.entry(&mut state, link, now) {
            entry.bucket.refill(now);
            if entry.bucket.milli < MILLI {
                return Err(Limited {
                    retry_after_secs: entry.bucket.wait(),
                });
            }
            entry.bucket.milli -= MILLI;
        }
        state.total.milli -= MILLI;
        Ok(())
    }

    /// Charges a request that was answered 404 to the shared bucket: [`FAILURE_EXTRA`] more
    /// tokens, as a debt if the bucket has not that many. The debt is bounded by the bucket's
    /// capacity, so a scanner stopped for a few seconds is let back in after them.
    pub fn failed(&self) {
        let now = Instant::now();
        let mut state = self.lock();
        state.total.refill(now);
        let floor = -state.total.capacity;
        state.total.milli = state
            .total
            .milli
            .saturating_sub(i64::from(FAILURE_EXTRA) * MILLI)
            .max(floor);
    }

    /// Takes the room of one open stream of `link`; the permit gives it back when it is dropped.
    ///
    /// # Errors
    /// [`Limited`] when the link has its streams, or all links together have theirs, or the table
    /// has no room to remember the link by.
    pub fn open_stream(self: &Arc<Self>, link: LinkKey) -> Result<StreamPermit, Limited> {
        let now = Instant::now();
        let mut state = self.lock();
        if state.streams >= self.limits.streams_total {
            return Err(Limited {
                retry_after_secs: 1,
            });
        }
        let per_link = self.limits.streams_per_link;
        let Some(entry) = self.entry(&mut state, link, now) else {
            return Err(Limited {
                retry_after_secs: 1,
            });
        };
        if entry.streams >= per_link {
            return Err(Limited {
                retry_after_secs: 1,
            });
        }
        entry.streams += 1;
        state.streams += 1;
        Ok(StreamPermit {
            limiter: Arc::clone(self),
            link,
        })
    }

    /// Streams open now, in all.
    pub fn open_streams(&self) -> u32 {
        self.lock().streams
    }

    /// The link's entry, made if the table has room (after forgetting the links that have nothing
    /// to remember).
    fn entry<'a>(&self, state: &'a mut State, link: LinkKey, now: Instant) -> Option<&'a mut Link> {
        if !state.links.contains_key(&link) {
            if state.links.len() >= MAX_LINKS {
                state.links.retain(|_, l| {
                    // an idle link is a full bucket; refill to find out
                    let mut bucket = l.bucket;
                    bucket.refill(now);
                    l.streams > 0 || !bucket.is_full()
                });
            }
            if state.links.len() >= MAX_LINKS {
                return None;
            }
            state.links.insert(
                link,
                Link {
                    bucket: Bucket::full(self.limits.per_link_per_second, now),
                    streams: 0,
                },
            );
        }
        state.links.get_mut(&link)
    }

    fn close_stream(&self, link: LinkKey) {
        let mut state = self.lock();
        state.streams = state.streams.saturating_sub(1);
        if let Some(entry) = state.links.get_mut(&link) {
            entry.streams = entry.streams.saturating_sub(1);
        }
    }
}

/// The room of one open stream, given back on drop. A response body that carries it is the stream.
#[derive(Debug)]
pub struct StreamPermit {
    limiter: Arc<PublicLimiter>,
    link: LinkKey,
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.limiter.close_stream(self.link);
    }
}

/// What the limiter's middleware puts in the extensions of a request to the public routes: the
/// limiter and the link the request is for, so a streaming handler can take its permit.
#[derive(Debug, Clone)]
pub struct PublicAccess {
    limiter: Arc<PublicLimiter>,
    link: LinkKey,
}

impl PublicAccess {
    pub(crate) fn new(limiter: Arc<PublicLimiter>, link: LinkKey) -> Self {
        PublicAccess { limiter, link }
    }

    /// Takes the room of one open stream of this request's link.
    ///
    /// # Errors
    /// [`Limited`], to be answered 429 with `Retry-After`.
    pub fn stream_permit(&self) -> Result<StreamPermit, Limited> {
        self.limiter.open_stream(self.link)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn limits(
        per_link: u32,
        total: u32,
        streams_per_link: u32,
        streams_total: u32,
    ) -> PublicLimits {
        PublicLimits {
            per_link_per_second: per_link,
            total_per_second: total,
            streams_per_link,
            streams_total,
        }
    }

    fn link(n: u32) -> LinkKey {
        LinkKey::of(&format!("token-{n}"))
    }

    #[tokio::test(start_paused = true)]
    async fn one_link_is_limited_to_its_rate_and_the_others_are_not() {
        let l = PublicLimiter::new(limits(3, 100, 5, 50));
        for _ in 0..3 {
            l.admit(link(1)).unwrap();
        }
        let limited = l.admit(link(1)).unwrap_err();
        assert_eq!(limited.retry_after_secs, 1);
        // another link has its own bucket
        l.admit(link(2)).unwrap();
        // a second later the first is let back in: a rate, not a ban
        tokio::time::advance(secs(1)).await;
        l.admit(link(1)).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn all_links_together_are_limited_to_the_shared_rate() {
        let l = PublicLimiter::new(limits(100, 5, 5, 50));
        for n in 0..5 {
            l.admit(link(n)).unwrap();
        }
        assert!(l.admit(link(99)).is_err());
        tokio::time::advance(Duration::from_millis(400)).await;
        // two tokens a second per ... 5 a second: 400 ms is two
        l.admit(link(100)).unwrap();
        l.admit(link(101)).unwrap();
        assert!(l.admit(link(102)).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_costs_the_shared_bucket_so_guessing_is_throttled() {
        let l = PublicLimiter::new(limits(10, 20, 5, 50));
        // a scanner tries different tokens: each is admitted, answered 404 and charged
        let mut tried = 0;
        while l.admit(link(tried)).is_ok() {
            l.failed();
            tried += 1;
            assert!(tried < 100, "never throttled");
        }
        // 20 tokens, 5 for each guess: about 4 guesses before the bucket is empty
        assert!((3..=5).contains(&tried), "{tried} guesses");
        // a reader of a link that works is held up too, until the bucket recovers
        assert!(l.admit(link(1000)).is_err());
        tokio::time::advance(secs(3)).await;
        l.admit(link(1000)).unwrap();
        // the debt is bounded: it is never more than the bucket holds
        for _ in 0..1000 {
            l.failed();
        }
        tokio::time::advance(secs(2)).await;
        l.admit(link(1001)).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn streams_are_held_per_link_and_in_all_and_given_back() {
        let l = PublicLimiter::new(limits(100, 100, 2, 3));
        let a1 = l.open_stream(link(1)).unwrap();
        let a2 = l.open_stream(link(1)).unwrap();
        assert!(l.open_stream(link(1)).is_err(), "two for one link");
        let b1 = l.open_stream(link(2)).unwrap();
        assert!(l.open_stream(link(3)).is_err(), "three in all");
        assert_eq!(l.open_streams(), 3);
        drop(a1);
        assert_eq!(l.open_streams(), 2);
        let _a3 = l.open_stream(link(1)).unwrap();
        drop((a2, b1));
        assert_eq!(l.open_streams(), 1);
        assert_eq!(l.limits().streams_total, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn the_table_of_links_cannot_be_grown_by_invented_tokens() {
        let l = PublicLimiter::new(limits(10, 1_000_000, 5, 1_000_000));
        // far more invented tokens than the table holds, each used once and left
        for n in 0..(2 * u32::try_from(MAX_LINKS).unwrap()) {
            l.admit(link(n)).unwrap();
            tokio::time::advance(Duration::from_millis(2)).await;
        }
        assert!(l.lock().links.len() <= MAX_LINKS);
        // streams hold a link in the table: it is not forgotten while one is open
        let permit = l.open_stream(link(0)).unwrap();
        tokio::time::advance(secs(10)).await;
        for n in 10_000..(10_000 + 2 * u32::try_from(MAX_LINKS).unwrap()) {
            let _ = l.admit(link(n));
        }
        assert!(l.lock().links.contains_key(&link(0)));
        drop(permit);
    }

    #[test]
    fn a_link_key_is_the_token_hashed_and_not_the_token() {
        assert_eq!(LinkKey::of("a"), LinkKey::of("a"));
        assert_ne!(LinkKey::of("a"), LinkKey::of("b"));
        assert!(!format!("{:?}", LinkKey::of("secret-token")).contains("secret-token"));
    }
}
