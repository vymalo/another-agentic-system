use orch_core::ErrorClass;
use orch_ports::{StoreError, WakeupError};

/// Whether a SQLSTATE says "try again later": class 08 (connection exception), `40001`
/// (serialization failure), `40P01` (deadlock detected), class 53 (insufficient resources),
/// `57P01`..`57P03` (admin shutdown, crash shutdown, cannot connect now).
///
/// Meanings *unverified* (PostgreSQL manual, Appendix A, from memory, 2026-09-29).
fn transient_sqlstate(code: &str) -> bool {
    code.starts_with("08")
        || code.starts_with("53")
        || matches!(code, "40001" | "40P01" | "57P01" | "57P02" | "57P03")
}

/// What to do about a driver error.
///
/// - the network, TLS, the pool and the connection worker, and the SQLSTATEs above:
///   [`ErrorClass::Transient`];
/// - a value the driver could not decode, or a column the query did not return: the row or
///   the schema is not what the code expects, [`ErrorClass::Corrupt`];
/// - everything else (a rejected statement, a constraint violation, `RowNotFound`, a bad
///   configuration): retrying repeats the failure, so it is a bug, [`ErrorClass::Internal`].
pub(crate) fn sql_class(err: &sqlx::Error) -> ErrorClass {
    match err {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::Protocol(_) => ErrorClass::Transient,
        sqlx::Error::Database(db) => match db.code() {
            Some(code) if transient_sqlstate(&code) => ErrorClass::Transient,
            _ => ErrorClass::Internal,
        },
        sqlx::Error::Decode(_)
        | sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::ColumnNotFound(_) => ErrorClass::Corrupt,
        _ => ErrorClass::Internal,
    }
}

/// Maps a driver error to the port's error type, keeping it as the boxed source (fixes the
/// old behaviour where a syntax error was "unavailable" and retried).
pub(crate) fn store_err(err: sqlx::Error) -> StoreError {
    match sql_class(&err) {
        ErrorClass::Transient => StoreError::unavailable(err),
        ErrorClass::Corrupt => StoreError::corrupt_with("a row does not decode", err),
        _ => StoreError::internal(err),
    }
}

/// Maps a migration failure: the history not matching what is embedded means the schema
/// cannot be trusted ([`StoreError::Corrupt`]); a failing statement is classified like any
/// driver error.
pub(crate) fn migrate_err(err: sqlx::migrate::MigrateError) -> StoreError {
    use sqlx::migrate::MigrateError as M;
    match err {
        M::Execute(e) | M::ExecuteMigration(e, _) => store_err(e),
        e @ (M::VersionMissing(_)
        | M::VersionMismatch(_)
        | M::VersionNotPresent(_)
        | M::VersionTooOld(..)
        | M::VersionTooNew(..)
        | M::Dirty(_)) => StoreError::corrupt_with("the applied migrations do not match", e),
        other => StoreError::internal(other),
    }
}

pub(crate) fn wakeup_err(err: sqlx::Error) -> WakeupError {
    WakeupError::unavailable(err)
}

/// Whether the error is a unique violation, optionally of one named constraint/index.
pub(crate) fn is_unique_violation(err: &sqlx::Error, constraint: Option<&str>) -> bool {
    err.as_database_error().is_some_and(|db| {
        db.is_unique_violation() && constraint.is_none_or(|c| db.constraint() == Some(c))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::borrow::Cow;
    use std::fmt;

    use sqlx::error::{DatabaseError, ErrorKind};

    use super::*;

    /// A server error carrying only a SQLSTATE.
    #[derive(Debug)]
    struct Sqlstate(&'static str);

    impl fmt::Display for Sqlstate {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "server error {}", self.0)
        }
    }

    impl std::error::Error for Sqlstate {}

    impl DatabaseError for Sqlstate {
        fn message(&self) -> &str {
            "server error"
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn db(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(Sqlstate(code)))
    }

    fn boxed(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
        msg.into()
    }

    #[test]
    fn driver_errors_are_classified_by_what_retrying_can_do() {
        use ErrorClass::{Corrupt, Internal, Transient};
        let table: Vec<(sqlx::Error, ErrorClass)> = vec![
            (sqlx::Error::Io(std::io::Error::other("reset")), Transient),
            (sqlx::Error::Tls(boxed("handshake")), Transient),
            (sqlx::Error::PoolTimedOut, Transient),
            (sqlx::Error::PoolClosed, Transient),
            (sqlx::Error::WorkerCrashed, Transient),
            (sqlx::Error::Protocol("bad frame".into()), Transient),
            (db("08006"), Transient),
            (db("40001"), Transient),
            (db("40P01"), Transient),
            (db("53300"), Transient),
            (db("57P01"), Transient),
            // What is wrong with the statement or the data is not cured by waiting.
            (db("42601"), Internal), // syntax_error
            (db("42P01"), Internal), // undefined_table
            (db("23505"), Internal), // unique_violation
            (db("23514"), Internal), // check_violation
            (db("22012"), Internal), // division_by_zero
            (sqlx::Error::RowNotFound, Internal),
            (sqlx::Error::Configuration(boxed("bad url")), Internal),
            (sqlx::Error::Encode(boxed("too big")), Internal),
            (sqlx::Error::Decode(boxed("not a uuid")), Corrupt),
            (sqlx::Error::ColumnNotFound("x".into()), Corrupt),
            (
                sqlx::Error::ColumnDecode {
                    index: "x".into(),
                    source: boxed("bad"),
                },
                Corrupt,
            ),
        ];
        for (err, want) in table {
            assert_eq!(sql_class(&err), want, "{err}");
        }
    }

    #[test]
    fn store_err_keeps_the_driver_error_as_its_source() {
        use orch_core::Classify as _;
        let e = store_err(sqlx::Error::PoolTimedOut);
        assert_eq!(e.class(), ErrorClass::Transient);
        assert!(e.is_retryable());
        let source = std::error::Error::source(&e).expect("a source");
        assert!(source.downcast_ref::<sqlx::Error>().is_some());
        assert!(!e.to_string().contains("pool"), "{e}");

        let e = store_err(db("42601"));
        assert_eq!(e.class(), ErrorClass::Internal);
        assert!(!e.is_retryable(), "a syntax error is never retried");
        assert!(matches!(e, StoreError::Internal { .. }));

        let e = store_err(sqlx::Error::Decode(boxed("bad")));
        assert_eq!(e.class(), ErrorClass::Corrupt);
        assert!(matches!(e, StoreError::Corrupt { .. }));
    }

    #[test]
    fn migration_history_problems_are_corrupt_and_statements_are_classified() {
        use orch_core::Classify as _;
        use sqlx::migrate::MigrateError as M;
        assert_eq!(
            migrate_err(M::VersionMismatch(3)).class(),
            ErrorClass::Corrupt
        );
        assert_eq!(migrate_err(M::Dirty(3)).class(), ErrorClass::Corrupt);
        assert_eq!(
            migrate_err(M::Execute(sqlx::Error::PoolTimedOut)).class(),
            ErrorClass::Transient
        );
        assert_eq!(
            migrate_err(M::ExecuteMigration(db("42601"), 2)).class(),
            ErrorClass::Internal
        );
    }

    #[test]
    fn a_wakeup_failure_is_transient_with_its_source() {
        use orch_core::Classify as _;
        let e = wakeup_err(sqlx::Error::PoolClosed);
        assert_eq!(e.class(), ErrorClass::Transient);
        assert!(std::error::Error::source(&e).is_some());
    }
}
