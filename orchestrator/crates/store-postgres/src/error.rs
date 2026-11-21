use deadpool_postgres::{HookError, PoolError};
use orch_core::ErrorClass;
use orch_pg_connect::ConnectError;
use orch_pg_migrate::MigrateError;
use orch_ports::{StoreError, WakeupError};

/// Why a statement of [`sql`](crate::sql) failed: the driver, the pool, the connection, or a row
/// that was not there.
///
/// It is the driver's error as this crate meets it, and is never kept as the source of a port's
/// error: [`store_err`] keeps a [`DbFailure`] instead (a server's `DETAIL`, which may quote a whole
/// row, is never in it). Its `Display` and its `Debug` are that failure's, and it has no source:
/// the server's error `tokio-postgres` keeps (its `Debug` included) holds the `DETAIL`, the `HINT`
/// and the `WHERE`.
#[derive(thiserror::Error)]
#[non_exhaustive]
pub enum SqlError {
    /// The driver's error: the server's, the network's, or a value that does not encode or decode.
    #[error("{}", DbFailure::of(self))]
    Driver(tokio_postgres::Error),
    /// The pool's: no connection in time, a closed pool, or the server's refusal of a connection.
    #[error("{}", DbFailure::of(self))]
    Pool(PoolError),
    /// A pool or a connection that could not be made (its TLS, its root certificate).
    #[error("{}", DbFailure::of(self))]
    Connect(ConnectError),
    /// No connection was made before the wait was over.
    #[error("{}", DbFailure::of(self))]
    TimedOut,
    /// A statement that must return a row returned none.
    #[error("{}", DbFailure::of(self))]
    RowNotFound,
}

impl From<tokio_postgres::Error> for SqlError {
    fn from(err: tokio_postgres::Error) -> Self {
        SqlError::Driver(err)
    }
}

impl std::fmt::Debug for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variant = match self {
            SqlError::Driver(_) => "Driver",
            SqlError::Pool(_) => "Pool",
            SqlError::Connect(_) => "Connect",
            SqlError::TimedOut => "TimedOut",
            SqlError::RowNotFound => "RowNotFound",
        };
        f.debug_tuple(variant).field(&DbFailure::of(self)).finish()
    }
}

impl SqlError {
    /// The server's error, when the server answered with one.
    pub fn as_db_error(&self) -> Option<&tokio_postgres::error::DbError> {
        match self {
            SqlError::Driver(e)
            | SqlError::Pool(PoolError::Backend(e))
            | SqlError::Connect(ConnectError::Database(e)) => e.as_db_error(),
            _ => None,
        }
    }

    /// The SQLSTATE of the server's error.
    pub fn code(&self) -> Option<&str> {
        self.as_db_error().map(|db| db.code().code())
    }
}

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

/// What `tokio-postgres` 0.7.18 says of a failure that is not the server's (its `Display`, which
/// is its kind and nothing else; the kinds themselves are private) when the database is down or
/// cannot be understood: the network, TLS, a connection that closed (its task ended), a frame that
/// does not parse or comes out of turn, a connect that failed or took too long. sqlx called these
/// `Io`, `Tls`, `Protocol` and `WorkerCrashed`. Each is pinned by a test of this module.
const DRIVER_OUTAGES: [&str; 7] = [
    "error communicating with the server",
    "error performing TLS handshake",
    "connection closed",
    "unexpected message from server",
    "error parsing response from server",
    "error connecting to server",
    "timeout waiting for server",
];

/// The kinds of `tokio-postgres` (their words, or the start of them) that can carry an I/O error
/// and are still no outage, read before the I/O rule: a message that cannot be encoded (a NUL byte
/// in a user, a database or a text the client sends at startup, refused every time; sqlx refused
/// such a value too), the client's own refusal of an authentication (a SASL mechanism or a server
/// signature it does not accept), and a parameter that cannot be encoded (`error serializing
/// parameter <n>`, whatever error its `ToSql` gave).
const DRIVER_REFUSALS: [&str; 3] = [
    "error encoding message to server",
    "authentication error",
    "error serializing parameter ",
];

fn is_refusal(kind: &str) -> bool {
    DRIVER_REFUSALS
        .iter()
        .any(|prefix| kind.starts_with(prefix))
}

/// The kinds of `tokio-postgres` that say a row is not what the code reads: a column that is not
/// there, or a value that does not decode as its Rust type (sqlx's `ColumnNotFound`,
/// `ColumnDecode`).
const DRIVER_CORRUPT: [&str; 2] = ["invalid column ", "error deserializing column "];

/// What to do about a driver error.
///
/// - the network, TLS, the pool and the connection, and the SQLSTATEs above:
///   [`ErrorClass::Transient`];
/// - a value the driver could not decode, or a column the query did not return: the row or
///   the schema is not what the code expects, [`ErrorClass::Corrupt`];
/// - everything else (a rejected statement, a constraint violation, a row that is not there, a
///   value that does not encode, a bad configuration): retrying repeats the failure, so it is a
///   bug, [`ErrorClass::Internal`].
pub(crate) fn sql_class(err: &SqlError) -> ErrorClass {
    match err {
        SqlError::Driver(e) => driver_class(e),
        SqlError::Pool(e) => pool_class(e),
        SqlError::Connect(ConnectError::Database(e)) => driver_class(e),
        // sqlx's TLS errors were transient: a root certificate that cannot be read, a TLS
        // configuration rustls refuses
        SqlError::Connect(ConnectError::RootCert | ConnectError::Tls(_)) => ErrorClass::Transient,
        SqlError::Connect(_) => ErrorClass::Internal,
        SqlError::TimedOut => ErrorClass::Transient,
        SqlError::RowNotFound => ErrorClass::Internal,
    }
}

fn pool_class(err: &PoolError) -> ErrorClass {
    match err {
        PoolError::Backend(e) | PoolError::PostCreateHook(HookError::Backend(e)) => driver_class(e),
        PoolError::Timeout(_) | PoolError::Closed => ErrorClass::Transient,
        PoolError::NoRuntimeSpecified | PoolError::PostCreateHook(HookError::Message(_)) => {
            ErrorClass::Internal
        }
    }
}

fn driver_class(err: &tokio_postgres::Error) -> ErrorClass {
    if let Some(db) = err.as_db_error() {
        return if transient_sqlstate(db.code().code()) {
            ErrorClass::Transient
        } else {
            ErrorClass::Internal
        };
    }
    driver_kind_class(
        err.to_string().as_str(),
        err.is_closed(),
        io_of(err).is_some(),
    )
}

/// The class of a driver's failure that is not the server's, from its words (`kind`), whether its
/// connection closed, and whether it carries an I/O error.
fn driver_kind_class(kind: &str, closed: bool, io: bool) -> ErrorClass {
    if DRIVER_CORRUPT.iter().any(|prefix| kind.starts_with(prefix)) {
        return ErrorClass::Corrupt;
    }
    if is_refusal(kind) {
        return ErrorClass::Internal;
    }
    if closed || io || DRIVER_OUTAGES.contains(&kind) {
        return ErrorClass::Transient;
    }
    ErrorClass::Internal
}

fn io_of(err: &tokio_postgres::Error) -> Option<&std::io::Error> {
    std::error::Error::source(err).and_then(|s| s.downcast_ref::<std::io::Error>())
}

/// Whether a connection was refused in a way that a database that is starting refuses one, which
/// sqlx's pool tried again: the TCP connection refused, too many connections (`53300`), cannot
/// connect now (`57P03`).
pub(crate) fn refused_while_starting(err: &tokio_postgres::Error) -> bool {
    if let Some(db) = err.as_db_error() {
        return retried_sqlstate(db.code().code());
    }
    io_of(err).is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// The SQLSTATEs at connection that sqlx's pool tried again.
fn retried_sqlstate(code: &str) -> bool {
    matches!(code, "53300" | "57P03")
}

/// A database's failure as this crate keeps it, the source of a port's error: its class and its
/// text, and nothing of the driver. The text is the server's severity, message and SQLSTATE (never
/// its `DETAIL`, which may quote a row, nor its `HINT`), or the driver's words for its kind (and
/// the I/O error under it, never a value that did not encode or decode).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", self.text())]
pub struct DbFailure {
    class: ErrorClass,
    severity: Option<String>,
    message: String,
    code: Option<String>,
}

impl DbFailure {
    pub(crate) fn of(err: &SqlError) -> Self {
        let class = sql_class(err);
        let driver = match err {
            SqlError::Driver(e)
            | SqlError::Pool(
                PoolError::Backend(e) | PoolError::PostCreateHook(HookError::Backend(e)),
            )
            | SqlError::Connect(ConnectError::Database(e)) => e,
            SqlError::Pool(e) => return Self::plain(class, pool_words(e)),
            SqlError::Connect(e) => return Self::plain(class, connect_words(e)),
            SqlError::TimedOut => {
                return Self::plain(
                    class,
                    "no connection to the database was made in time".into(),
                );
            }
            SqlError::RowNotFound => {
                return Self::plain(class, "a statement returned no row".into());
            }
        };
        if let Some(db) = driver.as_db_error() {
            return DbFailure {
                class,
                severity: Some(db.severity().to_owned()),
                message: db.message().to_owned(),
                code: Some(db.code().code().to_owned()),
            };
        }
        let kind = driver.to_string();
        // the I/O error says why a connection failed; a value's error could quote the value
        let message = match io_of(driver) {
            Some(io) if !is_refusal(&kind) => format!("{kind}: {io}"),
            _ => kind,
        };
        Self::plain(class, message)
    }

    /// The failure of a server's error of SQLSTATE `code`, for the tests of the mappings
    /// (`tokio-postgres` has no constructor for its error).
    #[cfg(test)]
    pub(crate) fn sqlstate(code: &str) -> Self {
        DbFailure {
            class: if transient_sqlstate(code) {
                ErrorClass::Transient
            } else {
                ErrorClass::Internal
            },
            severity: Some("ERROR".to_owned()),
            message: "refused".to_owned(),
            code: Some(code.to_owned()),
        }
    }

    fn plain(class: ErrorClass, message: String) -> Self {
        DbFailure {
            class,
            severity: None,
            message,
            code: None,
        }
    }

    /// What to do about it.
    pub fn class(&self) -> ErrorClass {
        self.class
    }

    /// The SQLSTATE, when the server answered.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    fn text(&self) -> std::borrow::Cow<'_, str> {
        match (&self.severity, &self.code) {
            (Some(severity), Some(code)) => {
                format!("{severity}: {} (SQLSTATE {code})", self.message).into()
            }
            _ => self.message.as_str().into(),
        }
    }
}

fn pool_words(err: &PoolError) -> String {
    match err {
        PoolError::Timeout(kind) => format!("no connection of the pool in time ({kind:?})"),
        PoolError::Closed => "the pool is closed".to_owned(),
        _ => "the pool cannot make a connection".to_owned(),
    }
}

fn connect_words(err: &ConnectError) -> String {
    match err {
        ConnectError::RootCert => "sslrootcert cannot be read".to_owned(),
        ConnectError::Tls(_) => "the TLS configuration is refused".to_owned(),
        ConnectError::Pool(_) => "the pool cannot be built".to_owned(),
        // `ConnectError` is non_exhaustive: its text is not kept (it might hold a driver's error)
        _ => "the connection failed in a way this build does not know".to_owned(),
    }
}

/// Maps a driver error to the port's error type, keeping the [`DbFailure`] as its source (a
/// syntax error is `Internal` and never retried, a network failure `Unavailable`).
pub(crate) fn store_err(err: impl Into<SqlError>) -> StoreError {
    let failure = DbFailure::of(&err.into());
    match failure.class() {
        ErrorClass::Transient => StoreError::unavailable(failure),
        ErrorClass::Corrupt => StoreError::corrupt_with("a row does not decode", failure),
        _ => StoreError::internal(failure),
    }
}

/// Maps a migration failure: the history not matching what is embedded means the schema
/// cannot be trusted ([`StoreError::Corrupt`]); a failing statement is classified like any
/// driver error.
pub(crate) fn migrate_err(err: MigrateError) -> StoreError {
    match err {
        MigrateError::Execute { source, .. } | MigrateError::Database(source) => store_err(source),
        // these hold a version and nothing else
        e @ (MigrateError::VersionMissing(_)
        | MigrateError::VersionMismatch(_)
        | MigrateError::Dirty(_)) => {
            StoreError::corrupt_with("the applied migrations do not match", e)
        }
        // a crate's own mistake: a name or a number, never a driver's error
        e @ (MigrateError::FileName(_)
        | MigrateError::TableName(_)
        | MigrateError::DuplicateVersion(_)) => StoreError::internal(e),
        // `MigrateError` is non_exhaustive: a variant added later is `Internal` until it is named
        // here, and is not kept as the source (it might hold a driver's error, and a DETAIL)
        _ => StoreError::internal("the migrations failed in a way this build does not know"),
    }
}

pub(crate) fn wakeup_err(err: impl Into<SqlError>) -> WakeupError {
    WakeupError::unavailable(DbFailure::of(&err.into()))
}

/// Whether the error is a unique violation, optionally of one named constraint/index.
pub(crate) fn is_unique_violation(err: &SqlError, constraint: Option<&str>) -> bool {
    err.as_db_error().is_some_and(|db| {
        db.code() == &tokio_postgres::error::SqlState::UNIQUE_VIOLATION
            && constraint.is_none_or(|c| db.constraint() == Some(c))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::time::Duration;

    use deadpool_postgres::TimeoutType;
    use orch_pg_connect::PgOptions;

    use super::*;

    #[test]
    fn sqlstates_are_classified_by_what_retrying_can_do() {
        for code in [
            "08006", "40001", "40P01", "53300", "57P01", "57P02", "57P03",
        ] {
            assert!(transient_sqlstate(code), "{code}");
        }
        // What is wrong with the statement or the data is not cured by waiting.
        for code in [
            "42601", "42P01", "23505", "23514", "22012", "55P03", "57014",
        ] {
            assert!(!transient_sqlstate(code), "{code}");
        }
    }

    #[test]
    fn what_sqlx_tried_again_at_connection_is_tried_again() {
        for code in ["53300", "57P03"] {
            assert!(retried_sqlstate(code), "{code}");
        }
        for code in ["28P01", "3D000", "08006", "57P01", "42501"] {
            assert!(!retried_sqlstate(code), "{code}");
        }
    }

    /// The driver's words for its kinds, as `tokio-postgres` 0.7.18 writes them, and the class each
    /// is: sqlx's `Io`, `Tls`, `Protocol` and `WorkerCrashed` were transient, its `ColumnDecode`
    /// and `ColumnNotFound` corrupt, everything else internal.
    #[test]
    fn driver_errors_are_classified_by_what_retrying_can_do() {
        use ErrorClass::{Corrupt, Internal, Transient};
        let table: [(&str, bool, bool, ErrorClass); 20] = [
            (
                "error communicating with the server",
                false,
                true,
                Transient,
            ),
            ("error performing TLS handshake", false, false, Transient),
            ("connection closed", true, false, Transient),
            ("unexpected message from server", false, false, Transient),
            ("error parsing response from server", false, true, Transient),
            ("error connecting to server", false, true, Transient),
            ("timeout waiting for server", false, false, Transient),
            // a closed connection is one whatever its words
            ("db error", true, false, Transient),
            ("error deserializing column 3", false, false, Corrupt),
            ("invalid column `v`", false, false, Corrupt),
            ("error serializing parameter 0", false, false, Internal),
            // whatever error the parameter's `ToSql` gave, an I/O error included
            ("error serializing parameter 3", false, true, Internal),
            (
                "query returned an unexpected number of rows",
                false,
                false,
                Internal,
            ),
            (
                "query returned an unexpected number of columns",
                false,
                false,
                Internal,
            ),
            ("expected 2 parameters but got 1", false, false, Internal),
            ("invalid connection string", false, false, Internal),
            ("invalid configuration", false, false, Internal),
            // carry an I/O error, and are refusals all the same
            ("error encoding message to server", false, true, Internal),
            ("authentication error", false, true, Internal),
            ("authentication error", false, false, Internal),
        ];
        for (kind, closed, io, want) in table {
            assert_eq!(driver_kind_class(kind, closed, io), want, "{kind}");
        }
    }

    #[test]
    fn the_pools_errors_are_classified() {
        use ErrorClass::{Internal, Transient};
        for (err, want) in [
            (
                SqlError::Pool(PoolError::Timeout(TimeoutType::Wait)),
                Transient,
            ),
            (
                SqlError::Pool(PoolError::Timeout(TimeoutType::Create)),
                Transient,
            ),
            (SqlError::Pool(PoolError::Closed), Transient),
            (SqlError::Pool(PoolError::NoRuntimeSpecified), Internal),
            (SqlError::TimedOut, Transient),
            (SqlError::RowNotFound, Internal),
            (SqlError::Connect(ConnectError::RootCert), Transient),
            (
                SqlError::Connect(ConnectError::Pool("max_size".into())),
                Internal,
            ),
        ] {
            assert_eq!(sql_class(&err), want, "{err}");
        }
    }

    #[test]
    fn store_err_keeps_the_failure_and_not_the_driver_error_as_its_source() {
        use orch_core::Classify as _;
        let e = store_err(SqlError::Pool(PoolError::Timeout(TimeoutType::Wait)));
        assert_eq!(e.class(), ErrorClass::Transient);
        assert!(e.is_retryable());
        let source = std::error::Error::source(&e).expect("a source");
        let failure = source.downcast_ref::<DbFailure>().expect("the failure");
        assert!(
            std::error::Error::source(failure).is_none(),
            "nothing under it"
        );
        assert!(!e.to_string().contains("pool"), "{e}");

        let e = store_err(SqlError::RowNotFound);
        assert_eq!(e.class(), ErrorClass::Internal);
        assert!(
            !e.is_retryable(),
            "a statement that is wrong is never retried"
        );
        assert!(matches!(e, StoreError::Internal { .. }));
    }

    #[test]
    fn migration_history_problems_are_corrupt_and_statements_are_classified() {
        use orch_core::Classify as _;
        assert_eq!(
            migrate_err(MigrateError::VersionMismatch(3)).class(),
            ErrorClass::Corrupt
        );
        assert_eq!(
            migrate_err(MigrateError::Dirty(3)).class(),
            ErrorClass::Corrupt
        );
        assert_eq!(
            migrate_err(MigrateError::VersionMissing(30)).class(),
            ErrorClass::Corrupt
        );
        assert_eq!(
            migrate_err(MigrateError::FileName("x.sql".into())).class(),
            ErrorClass::Internal
        );
    }

    /// The driver's errors of a migration are classified as any statement's: a database that
    /// cannot be reached is `Unavailable` (whether it failed a migration's SQL or the
    /// bookkeeping), a connection string the driver refuses is `Internal`.
    #[tokio::test]
    async fn a_migration_s_driver_errors_are_classified_as_a_statement_s() {
        let refused = || driver_error_of("postgres://u@127.0.0.1:1/db?sslmode=disable");
        let e = migrate_err(MigrateError::Database(refused().await));
        assert!(matches!(e, StoreError::Unavailable { .. }), "{e:?}");
        let e = migrate_err(MigrateError::Execute {
            version: 3,
            source: refused().await,
        });
        assert!(matches!(e, StoreError::Unavailable { .. }), "{e:?}");

        let unreadable = "host=h port=notaport"
            .parse::<tokio_postgres::Config>()
            .unwrap_err();
        let e = migrate_err(MigrateError::Database(unreadable));
        assert!(matches!(e, StoreError::Internal { .. }), "{e:?}");
    }

    #[tokio::test]
    async fn a_wakeup_failure_is_transient_with_its_source() {
        use orch_core::Classify as _;
        let e = wakeup_err(SqlError::Pool(PoolError::Closed));
        assert_eq!(e.class(), ErrorClass::Transient);
        assert!(std::error::Error::source(&e).is_some());
    }

    /// A server on a port of its own: it reads what the client sends first, answers `reply`, and
    /// holds the socket open (or closes it, with an empty reply and `close`), so the client reads
    /// the answer and not the end of the stream.
    async fn fake_server(reply: impl Into<Vec<u8>>, close: bool) -> u16 {
        let reply: std::sync::Arc<[u8]> = reply.into().into();
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut first = [0_u8; 1024];
                    let _ = socket.read(&mut first).await;
                    let _ = socket.write_all(&reply).await;
                    if !close {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                });
            }
        });
        port
    }

    /// A backend message: its tag, its length and its body.
    fn message(tag: u8, body: &[u8]) -> Vec<u8> {
        let len = u32::try_from(body.len() + 4).unwrap();
        let mut out = vec![tag];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    /// What the real driver makes of a connection to `url`.
    async fn driver_error_of(url: &str) -> tokio_postgres::Error {
        let options: PgOptions = url.parse().unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), options.connect())
            .await
            .unwrap();
        match outcome {
            Err(ConnectError::Database(e)) => e,
            Ok(_) => panic!("{url}: connected"),
            Err(other) => panic!("{url}: not the driver's error: {other:?}"),
        }
    }

    fn class_of(e: tokio_postgres::Error) -> ErrorClass {
        sql_class(&SqlError::Driver(e))
    }

    #[tokio::test]
    async fn a_refused_connection_is_transient_tried_again_and_shows_no_password() {
        let e = driver_error_of("postgres://u:hunter2-pw@127.0.0.1:1/db?sslmode=disable").await;
        assert_eq!(e.to_string(), "error connecting to server");
        assert!(refused_while_starting(&e));
        let error = store_err(e);
        assert!(matches!(error, StoreError::Unavailable { .. }), "{error:?}");
        assert!(
            !format!("{error} {error:?}").contains("hunter2"),
            "{error:?}"
        );
    }

    #[test]
    fn a_connection_string_the_driver_cannot_read_is_internal() {
        let e = "host=h port=notaport"
            .parse::<tokio_postgres::Config>()
            .unwrap_err();
        assert_eq!(e.to_string(), "invalid connection string");
        assert!(!refused_while_starting(&e));
        assert_eq!(class_of(e), ErrorClass::Internal);
    }

    /// `sslmode=require` against a server that answers `N` to the SSLRequest: the driver's TLS
    /// error, which carries no I/O error, so only its words make it transient.
    #[tokio::test]
    async fn a_server_that_refuses_tls_under_require_is_transient() {
        let port = fake_server(b"N".to_vec(), false).await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=require")).await;
        assert_eq!(e.to_string(), "error performing TLS handshake");
        assert!(io_of(&e).is_none());
        assert_eq!(class_of(e), ErrorClass::Transient);
    }

    /// A ReadyForQuery (`Z`, length 5, idle) before any authentication: the driver's unexpected
    /// message, with no I/O error either.
    #[tokio::test]
    async fn a_server_that_answers_out_of_turn_is_transient() {
        let port = fake_server(message(b'Z', b"I"), false).await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=disable")).await;
        assert_eq!(e.to_string(), "unexpected message from server");
        assert!(io_of(&e).is_none());
        assert_eq!(class_of(e), ErrorClass::Transient);
    }

    /// A server that hangs up before it answers.
    #[tokio::test]
    async fn a_server_that_hangs_up_is_transient() {
        let port = fake_server(Vec::new(), true).await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=disable")).await;
        let words = e.to_string();
        assert!(
            DRIVER_OUTAGES.contains(&words.as_str()),
            "{words}: not a word of the outages"
        );
        assert_eq!(class_of(e), ErrorClass::Transient);
    }

    /// A message of a type the protocol does not have (`!`, length 4): the codec fails, which the
    /// driver words as the I/O it was reading.
    #[tokio::test]
    async fn a_frame_of_no_type_is_transient() {
        let port = fake_server(message(b'!', b""), false).await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=disable")).await;
        assert_eq!(e.to_string(), "error communicating with the server");
        assert!(io_of(&e).is_some());
        assert_eq!(class_of(e), ErrorClass::Transient);
    }

    /// A list of SASL mechanisms whose last one is not ended: the driver cannot parse the body.
    #[tokio::test]
    async fn a_body_that_does_not_parse_is_transient() {
        let port = fake_server(message(b'R', b"\0\0\0\x0aNOPE"), false).await;
        let e = driver_error_of(&format!(
            "postgres://u:pw@127.0.0.1:{port}/db?sslmode=disable"
        ))
        .await;
        assert_eq!(e.to_string(), "error parsing response from server");
        assert_eq!(class_of(e), ErrorClass::Transient);
    }

    /// A NUL byte in the user: the startup message cannot be encoded, every time. The driver's
    /// error carries an I/O error, and is still no outage (sqlx refused such a URL).
    #[tokio::test]
    async fn a_user_that_cannot_be_encoded_is_internal() {
        let port = fake_server(Vec::new(), false).await;
        let e = driver_error_of(&format!(
            "postgres://a%00b@127.0.0.1:{port}/db?sslmode=disable"
        ))
        .await;
        assert_eq!(e.to_string(), "error encoding message to server");
        assert!(io_of(&e).is_some());
        assert!(!refused_while_starting(&e), "not tried again");
        assert_eq!(class_of(e), ErrorClass::Internal);
    }

    /// A server that asks for SASL with a mechanism the client does not offer: the client refuses
    /// the authentication itself, and refuses it every time.
    #[tokio::test]
    async fn an_authentication_the_client_refuses_is_internal() {
        // AuthenticationSASL (10) with one mechanism, `NOPE`
        let port = fake_server(message(b'R', b"\0\0\0\x0aNOPE\0\0"), false).await;
        let e = driver_error_of(&format!(
            "postgres://u:pw@127.0.0.1:{port}/db?sslmode=disable"
        ))
        .await;
        assert_eq!(e.to_string(), "authentication error");
        assert!(!refused_while_starting(&e));
        assert_eq!(class_of(e), ErrorClass::Internal);
    }

    /// The server's `DETAIL` (here a made-up row) is never in the failure that is kept, and the
    /// failure has nothing under it.
    #[tokio::test]
    async fn a_servers_detail_is_not_kept() {
        // ErrorResponse: severity, SQLSTATE 28000, message, DETAIL, HINT, WHERE, then the end
        let reply = message(
            b'E',
            b"SFATAL\0C28000\0Mno such role\0Dthe row (alice, hunter2)\0Hhunter3\0Whunter4\0\0",
        );
        let port = fake_server(reply, false).await;
        let e = driver_error_of(&format!("postgres://u@127.0.0.1:{port}/db?sslmode=disable")).await;
        assert_eq!(
            e.as_db_error().and_then(|db| db.detail()),
            Some("the row (alice, hunter2)")
        );
        assert!(
            format!("{e:?}").contains("hunter3"),
            "the driver's Debug has it"
        );
        let sql = SqlError::Driver(e);
        let shown = format!("{sql} {sql:?}");
        assert!(!shown.contains("hunter"), "{shown}");
        assert!(
            shown.starts_with("FATAL: no such role (SQLSTATE 28000) Driver("),
            "{shown}"
        );
        assert!(
            std::error::Error::source(&sql).is_none(),
            "nothing under it"
        );
        let error = store_err(sql);
        let source = std::error::Error::source(&error).map(ToString::to_string);
        let text = format!("{error} {error:?} {source:?}");
        assert!(!text.contains("hunter"), "{text}");
        assert!(text.contains("SQLSTATE 28000"), "{text}");
        assert!(text.contains("no such role"), "{text}");
    }

    /// A connection of the test to the database itself (`ORCH_TEST_DIRECT_DATABASE_URL`, else
    /// `ORCH_TEST_DATABASE_URL`: its temporary tables need one session), or nothing when unset.
    async fn database() -> Option<tokio_postgres::Client> {
        let url = std::env::var("ORCH_TEST_DIRECT_DATABASE_URL")
            .or_else(|_| std::env::var("ORCH_TEST_DATABASE_URL"));
        let Ok(url) = url else {
            eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
            return None;
        };
        let options: PgOptions = url.parse().unwrap();
        Some(options.connect().await.unwrap())
    }

    /// A constraint's `DETAIL` quotes the failing row (a check) or the key (a unique index): it is
    /// in no `Display` or `Debug` of the error, nor of the port's error made of it.
    #[tokio::test]
    async fn a_violation_s_detail_is_in_no_display_or_debug() {
        use crate::sql::query;
        let Some(conn) = database().await else {
            return;
        };
        crate::sql::raw_sql(
            "CREATE TEMP TABLE checked (v text CHECK (v <> 'x'), s text); \
             CREATE TEMP TABLE keyed (k text PRIMARY KEY)",
            &conn,
        )
        .await
        .unwrap();
        let check = query("INSERT INTO checked VALUES ('x', 'hunter5-row')")
            .execute(&conn)
            .await
            .unwrap_err();
        query("INSERT INTO keyed VALUES ('hunter6-key')")
            .execute(&conn)
            .await
            .unwrap();
        let unique = query("INSERT INTO keyed VALUES ('hunter6-key')")
            .execute(&conn)
            .await
            .unwrap_err();
        for (err, code) in [(check, "23514"), (unique, "23505")] {
            let detail = err.as_db_error().and_then(|db| db.detail()).unwrap();
            assert!(
                detail.contains("hunter"),
                "{code}: the server's DETAIL quotes it"
            );
            let shown = format!("{err} {err:?}");
            assert!(!shown.contains("hunter"), "{shown}");
            assert!(shown.contains(code), "{shown}");
            let port = store_err(err);
            let shown = format!("{port} {port:?}");
            assert!(!shown.contains("hunter"), "{shown}");
            assert!(matches!(port, StoreError::Internal { .. }), "{shown}");
        }
    }

    /// A column that does not decode as the Rust type read: the row is not what the code
    /// expects, `Corrupt` (sqlx's `ColumnDecode`).
    #[tokio::test]
    async fn a_value_that_does_not_decode_is_corrupt() {
        use crate::sql::query_scalar;
        let Some(conn) = database().await else {
            return;
        };
        let err = query_scalar::<i64>("SELECT 'x'::text")
            .fetch_one(&conn)
            .await
            .unwrap_err();
        let error = store_err(err);
        assert!(matches!(error, StoreError::Corrupt { .. }), "{error:?}");
    }
}
