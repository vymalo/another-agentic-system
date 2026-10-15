//! An in-memory store of pending authorizations: the reference implementation of the conformance
//! suite (`pending_authorizations_conformance!`), and what a test of the application or the API puts
//! in the bundle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jiff::Timestamp;
use orch_core::UserId;

use crate::{
    NoPendingAuthorizations, OAuthState, PendingAuthorization, PendingAuthorizationError,
    PendingAuthorizations, StateDigest, UserToolServerId,
};

#[derive(Debug)]
struct Entry {
    owner: UserId,
    server: UserToolServerId,
    expires_at: Timestamp,
}

#[derive(Debug, Default)]
struct State {
    /// Keyed by the digest of the state: the text of a state is never kept.
    by_digest: HashMap<StateDigest, Entry>,
    /// The state a server has pending.
    by_server: HashMap<(UserId, UserToolServerId), StateDigest>,
    down: bool,
}

impl State {
    fn remove(&mut self, digest: &StateDigest) -> Option<Entry> {
        let entry = self.by_digest.remove(digest)?;
        let key = (entry.owner.clone(), entry.server.clone());
        if self.by_server.get(&key) == Some(digest) {
            self.by_server.remove(&key);
        }
        Some(entry)
    }
}

/// The in-memory [`PendingAuthorizations`]. Cheap to clone: clones share the states, so a test keeps
/// one to look into while the application holds another.
#[derive(Debug, Clone, Default)]
pub struct MemoryPendingAuthorizations {
    state: Arc<Mutex<State>>,
}

impl MemoryPendingAuthorizations {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A test that panicked while holding the lock left maps that are still whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn up(&self) -> Result<MutexGuard<'_, State>, PendingAuthorizationError> {
        let state = self.lock();
        if state.down {
            return Err(PendingAuthorizationError::unavailable(
                "the in-memory store was taken down",
            ));
        }
        Ok(state)
    }

    /// Makes every call fail as a store that cannot be reached would until it is set up again;
    /// nothing is lost.
    pub fn set_down(&self, down: bool) {
        self.lock().down = down;
    }

    /// How many states are kept, of every owner, ended ones not yet forgotten included.
    pub fn len(&self) -> usize {
        self.lock().by_digest.len()
    }

    /// Whether no state is kept.
    pub fn is_empty(&self) -> bool {
        self.lock().by_digest.is_empty()
    }
}

/// What a bundle of the in-memory kind puts where the deployment has no pending authorizations
/// ([`UserTools::off`](crate::UserTools::off)): a store that is down, so it refuses as
/// [`NoPendingAuthorizations`] does.
impl From<NoPendingAuthorizations> for MemoryPendingAuthorizations {
    fn from(_: NoPendingAuthorizations) -> Self {
        let store = MemoryPendingAuthorizations::new();
        store.set_down(true);
        store
    }
}

impl PendingAuthorizations for MemoryPendingAuthorizations {
    async fn put(
        &self,
        state: &OAuthState,
        owner: &UserId,
        server: &UserToolServerId,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> Result<(), PendingAuthorizationError> {
        let mut kept = self.up()?;
        let ended: Vec<StateDigest> = kept
            .by_digest
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(digest, _)| *digest)
            .collect();
        for digest in ended {
            kept.remove(&digest);
        }
        let digest = StateDigest::of(state);
        // an earlier put of the same state, and the state this server had pending
        kept.remove(&digest);
        let key = (owner.clone(), server.clone());
        if let Some(earlier) = kept.by_server.get(&key).copied() {
            kept.remove(&earlier);
        }
        kept.by_server.insert(key, digest);
        kept.by_digest.insert(
            digest,
            Entry {
                owner: owner.clone(),
                server: server.clone(),
                expires_at,
            },
        );
        Ok(())
    }

    async fn take(
        &self,
        state: &OAuthState,
        now: Timestamp,
    ) -> Result<Option<PendingAuthorization>, PendingAuthorizationError> {
        let mut kept = self.up()?;
        Ok(kept
            .remove(&StateDigest::of(state))
            .filter(|entry| now < entry.expires_at)
            .map(|entry| PendingAuthorization::new(entry.owner, entry.server)))
    }
}
