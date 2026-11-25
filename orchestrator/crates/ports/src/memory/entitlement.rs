use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use jiff::Timestamp;
use orch_core::UserId;

use crate::{Entitlement, EntitlementError, EntitlementSource};

#[derive(Debug, Default)]
struct State {
    people: HashMap<UserId, Entitlement>,
    unavailable: bool,
    asked: Vec<UserId>,
}

/// An in-memory [`EntitlementSource`] a test writes as it runs: it says who is active
/// ([`set_active`](Self::set_active)), who lapsed and when ([`set_lapsed`](Self::set_lapsed)), forgets
/// a person ([`forget`](Self::forget)) and can be taken down
/// ([`set_unavailable`](Self::set_unavailable)).
///
/// A person it was never told about is [`Entitlement::Unknown`], as a source with no record of them
/// answers. [`asked`](Self::asked) lists who was read and in which order, a read of a source that was
/// down included, so a test can assert the orchestrator asked about the right person and nobody else.
/// Clones share the same state, the way two replicas read one source.
#[derive(Debug, Clone, Default)]
pub struct MemoryEntitlement {
    state: Arc<Mutex<State>>,
}

impl MemoryEntitlement {
    /// An available source that knows nobody.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves a table that is still a table.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `person`'s plan covers them from now on.
    pub fn set_active(&self, person: &UserId) {
        self.state()
            .people
            .insert(person.clone(), Entitlement::Active);
    }

    /// `person`'s plan ended at `since`.
    pub fn set_lapsed(&self, person: &UserId, since: Timestamp) {
        self.state()
            .people
            .insert(person.clone(), Entitlement::Lapsed { since });
    }

    /// The source forgets `person`: the next read answers [`Entitlement::Unknown`].
    pub fn forget(&self, person: &UserId) {
        self.state().people.remove(person);
    }

    /// Takes the source down (`true`): until it is put back up every read is
    /// [`EntitlementError::Unavailable`]. `false` puts it back.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.state().unavailable = unavailable;
    }

    /// Who was asked, in the order asked, every read since the last
    /// [`clear_asked`](Self::clear_asked).
    pub fn asked(&self) -> Vec<UserId> {
        self.state().asked.clone()
    }

    /// Forgets who was asked.
    pub fn clear_asked(&self) {
        self.state().asked.clear();
    }

    /// How many people it holds a record of.
    pub fn len(&self) -> usize {
        self.state().people.len()
    }

    /// Whether it holds no record of anybody.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl EntitlementSource for MemoryEntitlement {
    async fn entitled(&self, person: &UserId) -> Result<Entitlement, EntitlementError> {
        let mut state = self.state();
        state.asked.push(person.clone());
        if state.unavailable {
            return Err(EntitlementError::unavailable(std::io::Error::other(
                "the in-memory entitlement source is taken down",
            )));
        }
        Ok(state
            .people
            .get(person)
            .copied()
            .unwrap_or(Entitlement::Unknown))
    }
}
