//! How many `wait_for_job` calls may be open at once. A wait is a request that stays open, on a
//! machine route that has no request timeout, so it is bounded here: per process, and per user
//! so that one token cannot use up the process's share.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use orch_core::UserId;

/// Which limit a refused wait ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// The process already holds as many waits as it allows.
    Server {
        /// The limit.
        limit: usize,
    },
    /// The user already holds as many waits as one user may.
    User {
        /// The limit.
        limit: usize,
    },
}

impl Busy {
    /// What the caller is told.
    pub fn message(self) -> String {
        match self {
            Busy::Server { limit } => format!(
                "too many waits: this server already holds {limit} wait_for_job calls; retry later"
            ),
            Busy::User { limit } => format!(
                "too many waits: you already have {limit} wait_for_job calls in progress; \
                 wait for one to return, or retry later"
            ),
        }
    }
}

#[derive(Debug, Default)]
struct Counts {
    total: usize,
    per_user: HashMap<UserId, usize>,
}

/// The open waits of one process.
#[derive(Debug)]
pub struct WaitSlots {
    max_total: usize,
    max_per_user: usize,
    counts: Mutex<Counts>,
}

/// One open wait: the slot is given back when it is dropped, however the call ends.
#[derive(Debug)]
#[must_use = "the slot is held until the permit is dropped"]
pub struct WaitPermit {
    slots: Arc<WaitSlots>,
    user: UserId,
}

impl WaitSlots {
    /// Limits of `max_total` waits for the process and `max_per_user` for each user (each at
    /// least 1).
    pub fn new(max_total: usize, max_per_user: usize) -> Arc<Self> {
        Arc::new(WaitSlots {
            max_total: max_total.max(1),
            max_per_user: max_per_user.max(1),
            counts: Mutex::new(Counts::default()),
        })
    }

    /// Takes a slot for `user`, or says which limit is reached.
    pub fn acquire(self: &Arc<Self>, user: &UserId) -> Result<WaitPermit, Busy> {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if counts.total >= self.max_total {
            return Err(Busy::Server {
                limit: self.max_total,
            });
        }
        let mine = counts.per_user.get(user).copied().unwrap_or(0);
        if mine >= self.max_per_user {
            return Err(Busy::User {
                limit: self.max_per_user,
            });
        }
        counts.total += 1;
        counts.per_user.insert(user.clone(), mine + 1);
        Ok(WaitPermit {
            slots: Arc::clone(self),
            user: user.clone(),
        })
    }

    /// The waits open now, for tests and logs.
    pub fn open(&self) -> usize {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .total
    }
}

impl Drop for WaitPermit {
    fn drop(&mut self) {
        let mut counts = self
            .slots
            .counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        counts.total = counts.total.saturating_sub(1);
        if let Some(n) = counts.per_user.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.per_user.remove(&self.user);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_user_and_the_server_are_each_limited_and_a_dropped_permit_frees_its_slot() {
        let slots = WaitSlots::new(3, 2);
        let (alice, bob) = (UserId::new("alice@x.io"), UserId::new("bob@x.io"));
        let a1 = slots.acquire(&alice).unwrap();
        let _a2 = slots.acquire(&alice).unwrap();
        assert_eq!(slots.acquire(&alice).unwrap_err(), Busy::User { limit: 2 });
        let _b1 = slots.acquire(&bob).unwrap();
        // Three are open: the server is full for everyone, and says so before the user limit.
        assert_eq!(slots.acquire(&bob).unwrap_err(), Busy::Server { limit: 3 });
        assert_eq!(slots.open(), 3);
        drop(a1);
        assert_eq!(slots.open(), 2);
        let _b2 = slots.acquire(&bob).unwrap();
        assert_eq!(slots.acquire(&bob).unwrap_err(), Busy::Server { limit: 3 });
    }

    #[test]
    fn the_message_names_the_limit_and_starts_with_the_words_callers_look_for() {
        assert!(
            Busy::User { limit: 16 }
                .message()
                .starts_with("too many waits")
        );
        assert!(Busy::Server { limit: 256 }.message().contains("256"));
    }
}
