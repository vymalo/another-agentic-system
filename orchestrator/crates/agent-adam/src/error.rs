//! [`LocalAgentsError`]: why setting up or running the local agents failed.

use adam_core::StoreError;
use adam_notify_postgres::NotifyError;
use adam_runtime::{Classify as _, RuntimeError};
use orch_core::{Classify, ErrorClass};

/// Why setting up or running [`LocalAgents`](crate::LocalAgents) failed.
///
/// Each layer describes itself; the lower error is the [`source`](std::error::Error::source),
/// and [`orch_core::report`] prints the chain once. The class is the lower error's, so the
/// binary maps a transient outage of the database to the same exit code as the orchestrator's
/// own store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LocalAgentsError {
    /// The journal store could not be set up (invalid prefix, migration, connection).
    #[error("the local agents' store failed")]
    Store(#[source] StoreError),
    /// The notifier could not be set up or stopped by itself.
    #[error("the local agents' notifier failed")]
    Notify(#[source] NotifyError),
    /// The worker stopped by itself.
    #[error("the local agents' worker failed")]
    Worker(#[source] RuntimeError),
}

impl LocalAgentsError {
    /// The journal's database cannot be reached (or went away): transient, so a restart may
    /// cure it. `source` is the driver's error.
    pub fn unavailable(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        LocalAgentsError::Store(StoreError::unavailable(source))
    }
}

/// `adam-rs` and the orchestrator each define the same classification model (the core does not
/// depend on `adam-rs`: ADR 0007). This is the only translation between them; the wildcard is
/// for classes `adam-rs` may add, which are alerts until someone decides.
fn class_of(class: adam_runtime::ErrorClass) -> ErrorClass {
    use adam_runtime::ErrorClass as A;
    match class {
        A::Transient => ErrorClass::Transient,
        A::RateLimited => ErrorClass::RateLimited,
        A::Conflict => ErrorClass::Conflict,
        A::Invalid => ErrorClass::Invalid,
        A::NotFound => ErrorClass::NotFound,
        A::Rejected => ErrorClass::Rejected,
        A::Unauthenticated => ErrorClass::Unauthenticated,
        A::Unsupported => ErrorClass::Unsupported,
        A::Corrupt => ErrorClass::Corrupt,
        A::Internal => ErrorClass::Internal,
        _ => ErrorClass::Internal,
    }
}

impl Classify for LocalAgentsError {
    fn class(&self) -> ErrorClass {
        match self {
            LocalAgentsError::Store(e) => class_of(e.class()),
            LocalAgentsError::Notify(e) => class_of(e.class()),
            LocalAgentsError::Worker(e) => class_of(e.class()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn lower() -> std::io::Error {
        std::io::Error::other("connection refused")
    }

    #[test]
    fn the_class_is_the_lower_errors() {
        let down = LocalAgentsError::Store(StoreError::unavailable(lower()));
        assert_eq!(down.class(), ErrorClass::Transient);
        assert!(down.is_retryable());

        let bug = LocalAgentsError::Store(StoreError::internal(lower()));
        assert_eq!(bug.class(), ErrorClass::Internal);

        let prefix = LocalAgentsError::Notify(NotifyError::InvalidPrefix("X".into()));
        assert_eq!(prefix.class(), ErrorClass::Invalid);
        assert!(!prefix.is_retryable());

        let closed = LocalAgentsError::Notify(NotifyError::PoolClosed);
        assert_eq!(closed.class(), ErrorClass::Rejected);
    }

    #[test]
    fn the_source_is_kept_and_printed_once() {
        let e = LocalAgentsError::Store(StoreError::unavailable(lower()));
        assert!(std::error::Error::source(&e).is_some());
        assert!(!e.to_string().contains("connection refused"));
        assert!(orch_core::report(&e).ends_with("connection refused"));
    }
}
