//! [`LocalAgentsError`]: why setting up or running the local agents failed.

use std::error::Error as StdError;

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
///
/// No error holds a connection string (it may carry a password), nor a database's error as the
/// driver keeps it: a server's `DETAIL` may quote a row, so where the chain reached the driver it
/// ends in a [`DbFailure`] instead, the server's severity, message and SQLSTATE, or the driver's
/// words for what went wrong.
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
    /// cure it. `source` is kept as a [`DbFailure`] (its text, with no server's `DETAIL`).
    pub fn unavailable(source: impl StdError + Send + Sync + 'static) -> Self {
        LocalAgentsError::Store(StoreError::unavailable(DbFailure::of(&source)))
    }

    /// The store's error, its driver's error kept as a [`DbFailure`].
    pub(crate) fn store(err: StoreError) -> Self {
        LocalAgentsError::Store(scrub_store(err))
    }

    /// The worker's error, a store's driver error under it kept as a [`DbFailure`].
    pub(crate) fn worker(err: RuntimeError) -> Self {
        LocalAgentsError::Worker(match err {
            RuntimeError::Store(e) => RuntimeError::Store(scrub_store(e)),
            other => other,
        })
    }
}

/// A [`StoreError`] whose backend's error is a [`DbFailure`]: the same class, nothing of the
/// driver.
pub(crate) fn scrub_store(err: StoreError) -> StoreError {
    match err {
        StoreError::Backend { class, source } => StoreError::Backend {
            class,
            source: Box::new(DbFailure::of(source.as_ref())),
        },
        other => other,
    }
}

/// A database's failure as this crate keeps it: the words of each layer of an error, down to the
/// driver's, and there the server's severity, message and SQLSTATE (never its `DETAIL`, which may
/// quote a row, nor its `HINT`) or the driver's words for its kind (and the I/O error under it).
/// It has no source: the driver's error `tokio-postgres` keeps, its `Debug` included, holds the
/// `DETAIL`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{text}")]
pub(crate) struct DbFailure {
    text: String,
}

impl DbFailure {
    /// `err` and its sources, as words.
    pub(crate) fn of(err: &(dyn StdError + 'static)) -> Self {
        let mut words = Vec::new();
        let mut layer = Some(err);
        while let Some(e) = layer {
            if let Some(driver) = e.downcast_ref::<tokio_postgres::Error>() {
                words.push(driver_words(driver));
                break;
            }
            if let Some(db) = e.downcast_ref::<tokio_postgres::error::DbError>() {
                words.push(server_words(db));
                break;
            }
            words.push(e.to_string());
            layer = e.source();
        }
        DbFailure {
            text: words.join(": "),
        }
    }
}

fn server_words(db: &tokio_postgres::error::DbError) -> String {
    format!(
        "{}: {} (SQLSTATE {})",
        db.severity(),
        db.message(),
        db.code().code()
    )
}

/// `tokio-postgres` 0.7's `Display` is its kind and nothing else; the server's error and an I/O
/// error are its source.
fn driver_words(err: &tokio_postgres::Error) -> String {
    if let Some(db) = err.as_db_error() {
        return server_words(db);
    }
    match err
        .source()
        .and_then(|s| s.downcast_ref::<std::io::Error>())
    {
        Some(io) => format!("{err}: {io}"),
        None => err.to_string(),
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

    /// An error with a source of its own, as a driver's error over the server's.
    #[derive(Debug, thiserror::Error)]
    #[error("the pool cannot make a connection")]
    struct Layer(#[source] std::io::Error);

    #[test]
    fn a_stores_and_a_workers_backend_error_keep_their_class_and_their_words() {
        for (store, class) in [
            (
                StoreError::unavailable(Layer(lower())),
                ErrorClass::Transient,
            ),
            (StoreError::internal(Layer(lower())), ErrorClass::Internal),
            (
                StoreError::corrupt_source(Layer(lower())),
                ErrorClass::Corrupt,
            ),
        ] {
            let kept = LocalAgentsError::store(store);
            assert_eq!(kept.class(), class);
            assert!(
                orch_core::report(&kept)
                    .ends_with("the pool cannot make a connection: connection refused"),
                "{}",
                orch_core::report(&kept)
            );
        }
        let worker =
            LocalAgentsError::worker(RuntimeError::Store(StoreError::unavailable(Layer(lower()))));
        assert_eq!(worker.class(), ErrorClass::Transient);
        assert!(orch_core::report(&worker).ends_with("connection refused"));
        // a store's error that is not the driver's is kept as it is
        let busy = LocalAgentsError::store(StoreError::InvalidInput("a bad prefix".into()));
        assert_eq!(busy.class(), ErrorClass::Invalid);
        assert!(orch_core::report(&busy).contains("a bad prefix"));
    }
}
