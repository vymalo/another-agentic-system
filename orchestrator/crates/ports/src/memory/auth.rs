//! An authenticator held in memory: bearer tokens a test hands out, and an issuer it can take down.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{AuthError, Authenticator, Credentials, Principal};

#[derive(Debug, Default)]
struct State {
    tokens: HashMap<String, Principal>,
    down: bool,
}

/// An [`Authenticator`] over a table of bearer tokens: a token in the table is its principal, any
/// other token is refused, and no bearer is `Missing` (the identity header is not read).
/// [`set_down`](MemoryAuth::set_down) makes it `Unavailable`, as an issuer whose keys cannot be
/// read is. Clones share the table.
///
/// It is the reference implementation of the `Authenticator` testkit.
#[derive(Debug, Clone, Default)]
pub struct MemoryAuth {
    state: Arc<Mutex<State>>,
}

impl MemoryAuth {
    /// A table with no token, up.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Accepts `token` as `principal`.
    pub fn allow(&self, token: &str, principal: Principal) {
        self.state().tokens.insert(token.to_owned(), principal);
    }

    /// Takes the issuer down (`true`) or brings it back (`false`).
    pub fn set_down(&self, down: bool) {
        self.state().down = down;
    }
}

impl Authenticator for MemoryAuth {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        let Some(token) = credentials.bearer else {
            return Err(AuthError::Missing);
        };
        let state = self.state();
        if state.down {
            return Err(AuthError::unavailable("the token issuer cannot be read"));
        }
        state
            .tokens
            .get(token)
            .cloned()
            .ok_or_else(|| AuthError::invalid_bearer("the token is not known"))
    }

    async fn ready(&self) -> Result<(), AuthError> {
        if self.state().down {
            Err(AuthError::unavailable("the token issuer cannot be read"))
        } else {
            Ok(())
        }
    }

    fn accepts_bearer(&self) -> bool {
        true
    }
}
