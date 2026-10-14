//! An in-memory store of the definitions of people's MCP servers: the reference implementation of
//! the conformance suite (`user_tool_server_store_conformance!`), and what a test of the
//! application puts in the bundle when it needs servers to be kept.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use orch_core::UserId;

use crate::{
    MAX_USER_TOOL_SERVERS, UserToolServer, UserToolServerId, UserToolServerState,
    UserToolServerStore, UserToolServerStoreError,
};

#[derive(Debug, Default)]
struct State {
    /// Ordered by `(owner, id)`, so a listing is ordered by id.
    servers: BTreeMap<(UserId, UserToolServerId), UserToolServer>,
    down: bool,
}

/// The in-memory [`UserToolServerStore`]. Cheap to clone: clones share the definitions, so a test
/// keeps one to look into while the application holds another.
#[derive(Debug, Clone, Default)]
pub struct MemoryUserToolServers {
    state: Arc<Mutex<State>>,
}

impl MemoryUserToolServers {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A test that panicked while holding the lock left a map that is still whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Like `lock`, for a call: refused while the store is [down](Self::set_down).
    fn up(&self) -> Result<MutexGuard<'_, State>, UserToolServerStoreError> {
        let state = self.lock();
        if state.down {
            return Err(UserToolServerStoreError::unavailable(
                "the in-memory store was taken down",
            ));
        }
        Ok(state)
    }

    /// Makes every call fail as a store that cannot be reached would
    /// ([`UserToolServerStoreError::Unavailable`]) until it is set up again; nothing is lost.
    pub fn set_down(&self, down: bool) {
        self.lock().down = down;
    }

    /// How many definitions are kept, of every owner.
    pub fn len(&self) -> usize {
        self.lock().servers.len()
    }

    /// Whether no definition is kept.
    pub fn is_empty(&self) -> bool {
        self.lock().servers.is_empty()
    }
}

impl UserToolServerStore for MemoryUserToolServers {
    async fn put(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<(), UserToolServerStoreError> {
        let mut state = self.up()?;
        let key = (owner.clone(), server.id().clone());
        if !state.servers.contains_key(&key) {
            let held = state.servers.keys().filter(|(o, _)| o == owner).count();
            if held >= MAX_USER_TOOL_SERVERS {
                return Err(UserToolServerStoreError::TooMany {
                    limit: MAX_USER_TOOL_SERVERS,
                });
            }
        }
        state.servers.insert(key, server.clone());
        Ok(())
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<UserToolServer>, UserToolServerStoreError> {
        Ok(self
            .up()?
            .servers
            .get(&(owner.clone(), id.clone()))
            .cloned())
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<UserToolServer>, UserToolServerStoreError> {
        Ok(self
            .up()?
            .servers
            .iter()
            .filter(|((o, _), _)| o == owner)
            .map(|(_, server)| server.clone())
            .collect())
    }

    async fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
    ) -> Result<(), UserToolServerStoreError> {
        let mut kept = self.up()?;
        let server = kept
            .servers
            .get_mut(&(owner.clone(), id.clone()))
            .ok_or(UserToolServerStoreError::NotFound)?;
        *server = server.clone().with_state(state);
        Ok(())
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        self.up()?.servers.remove(&(owner.clone(), id.clone()));
        Ok(())
    }
}
