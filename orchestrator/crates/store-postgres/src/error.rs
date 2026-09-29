use orch_ports::{StoreError, WakeupError};

/// Maps a driver error to the port's error type. Everything that is not provably bad data
/// is `Unavailable` (retryable): connection loss, pool timeouts, deadlocks, serialization
/// failures.
pub(crate) fn store_err(err: sqlx::Error) -> StoreError {
    match err {
        sqlx::Error::Decode(_)
        | sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::ColumnIndexOutOfBounds { .. }
        | sqlx::Error::TypeNotFound { .. }
        | sqlx::Error::Encode(_) => StoreError::Corrupt(err.to_string()),
        other => StoreError::Unavailable(other.to_string()),
    }
}

pub(crate) fn wakeup_err(err: sqlx::Error) -> WakeupError {
    WakeupError::Unavailable(err.to_string())
}

/// Whether the error is a unique violation, optionally of one named constraint/index.
pub(crate) fn is_unique_violation(err: &sqlx::Error, constraint: Option<&str>) -> bool {
    err.as_database_error().is_some_and(|db| {
        db.is_unique_violation() && constraint.is_none_or(|c| db.constraint() == Some(c))
    })
}
