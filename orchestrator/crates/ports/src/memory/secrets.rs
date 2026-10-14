//! An in-memory secret store: the reference implementation of the conformance suite
//! (`secret_store_conformance!`), and what a test of the broker or the application puts in the
//! bundle when it needs credentials to be kept. It **seals nothing**: it holds the material in
//! memory as it is, for a test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use orch_core::UserId;

use crate::{SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion};

#[derive(Debug, Default)]
struct State {
    secrets: HashMap<(UserId, SecretServer), (SecretVersion, SecretMaterial)>,
    /// The last version handed out, for every key: one counter for the whole store, so that a
    /// version is never reused, not even by a key that was deleted and made again.
    last: u64,
    down: bool,
}

impl State {
    fn next_version(&mut self) -> SecretVersion {
        self.last += 1;
        SecretVersion::new(self.last)
    }
}

/// The in-memory [`SecretStore`]. Cheap to clone: clones share the secrets, so a test keeps one to
/// look into while the application holds another.
#[derive(Debug, Clone, Default)]
pub struct MemorySecretStore {
    state: Arc<Mutex<State>>,
}

impl MemorySecretStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A test that panicked while holding the lock left a map that is still whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Like `lock`, for a call: refused while the store is [down](Self::set_down).
    fn up(&self) -> Result<MutexGuard<'_, State>, SecretError> {
        let state = self.lock();
        if state.down {
            return Err(SecretError::unavailable(
                "the in-memory store was taken down",
            ));
        }
        Ok(state)
    }

    /// Makes every call fail as a store that cannot be reached would ([`SecretError::Unavailable`])
    /// until it is set up again; nothing is lost.
    pub fn set_down(&self, down: bool) {
        self.lock().down = down;
    }

    /// How many secrets are kept, of every owner.
    pub fn len(&self) -> usize {
        self.lock().secrets.len()
    }

    /// Whether no secret is kept.
    pub fn is_empty(&self) -> bool {
        self.lock().secrets.is_empty()
    }
}

impl SecretStore for MemorySecretStore {
    async fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        let mut state = self.up()?;
        let version = state.next_version();
        state
            .secrets
            .insert((owner.clone(), server.clone()), (version, material.clone()));
        Ok(version)
    }

    async fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        Ok(self
            .up()?
            .secrets
            .get(&(owner.clone(), server.clone()))
            .cloned())
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        self.up()?.secrets.remove(&(owner.clone(), server.clone()));
        Ok(())
    }

    async fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        let mut state = self.up()?;
        let key = (owner.clone(), server.clone());
        match state.secrets.get(&key) {
            Some((version, _)) if *version == expected => {}
            _ => return Err(SecretError::Conflict),
        }
        let version = state.next_version();
        state.secrets.insert(key, (version, material.clone()));
        Ok(version)
    }
}
