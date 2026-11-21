//! The statements of this crate on `tokio-postgres`, written as sqlx wrote them (ADR 0076, sys#382
//! E1): `query(sql).bind(x).fetch_optional(&mut *tx)`.
//!
//! What sqlx did and this module keeps:
//!
//! - **Every parameter is sent with its type**, the one sqlx sent for the Rust type ([`Encode`]):
//!   the statement is prepared with them (`prepare_typed_cached`), so the server reads `$1` as sqlx
//!   told it to and never infers a type from the SQL around it.
//! - **Every statement is prepared once per connection and cached**, as sqlx's statement cache did
//!   (deadpool's cache, on the connection; a transaction's statements share it).
//! - **A pool's connection is checked before it is handed out**, the pool waits as long as sqlx's
//!   `acquire_timeout`, and a database that refuses connections because it is starting is tried
//!   again during that wait ([`PgPool`]).
//! - **A transaction that is dropped is rolled back** before its connection serves anybody else
//!   ([`Tx`]).
//! - **Each statement is logged at `debug`** (target `orch_store_postgres::sql`, the SQL and never a
//!   parameter), and one that takes a second or more at `warn`, as sqlx logged them under the target
//!   `sqlx::query`.
//!
//! It is public for the tests of this workspace, which run their own statements on a store's pool;
//! it is not a general database API.

use std::borrow::Cow;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deadpool_postgres::{Object, Pool, PoolError};
/// The options a [`PgPool`] connects with (`orch-pg-connect`'s), re-exported for callers of
/// [`PgPool::new`].
pub use orch_pg_connect::PgOptions;
use orch_pg_connect::{ConnectionTasks, LINGER};
use tokio::sync::watch;
use tokio_postgres::types::{FromSql, ToSql, Type};
/// A connection of no pool, and a row, as `tokio-postgres` has them.
pub use tokio_postgres::{Client, Row};

pub use crate::error::SqlError;
use crate::error::refused_while_starting;

/// How long a pool waits for a connection: sqlx's default `acquire_timeout`, which every pool of
/// this crate had.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(30);

/// A statement that takes longer is logged at `warn`, as sqlx's default `log_slow_statements`.
const SLOW_STATEMENT: Duration = Duration::from_secs(1);

/// The target of the statements' log lines.
const LOG_TARGET: &str = "orch_store_postgres::sql";

/// A pool of connections to one database, and the options it connects with (the listener of a
/// [`PgWakeup`](crate::PgWakeup) makes its own connection with them).
///
/// Cheap to clone: clones share the pool. [`close`](Self::close) closes it for every clone.
#[derive(Clone)]
pub struct PgPool {
    pool: Pool,
    options: Arc<PgOptions>,
    closed: Arc<watch::Sender<bool>>,
    tasks: ConnectionTasks,
}

impl std::fmt::Debug for PgPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgPool")
            .field("status", &self.pool.status())
            .finish_non_exhaustive()
    }
}

impl PgPool {
    /// A pool of at most `max_connections` (at least one) to the database of `options`, made on
    /// demand, each waited for at most [`DEFAULT_WAIT`]. It connects to nothing yet.
    ///
    /// # Errors
    /// [`SqlError::Connect`] when the TLS configuration of `options` cannot be built (its
    /// `sslrootcert` cannot be read).
    pub fn new(options: PgOptions, max_connections: u32) -> Result<Self, SqlError> {
        Self::with_wait(options, max_connections, DEFAULT_WAIT)
    }

    /// [`new`](Self::new) with another wait.
    ///
    /// # Errors
    /// As [`new`](Self::new).
    pub fn with_wait(
        options: PgOptions,
        max_connections: u32,
        wait: Duration,
    ) -> Result<Self, SqlError> {
        let max = usize::try_from(max_connections.max(1)).unwrap_or(usize::MAX);
        let tasks = ConnectionTasks::default();
        let pool = options
            .pool_tracked(max, wait, &tasks)
            .map_err(SqlError::Connect)?;
        let (closed, _) = watch::channel(false);
        Ok(Self {
            pool,
            options: Arc::new(options),
            closed: Arc::new(closed),
            tasks,
        })
    }

    /// The options the pool connects with.
    pub fn options(&self) -> &PgOptions {
        &self.options
    }

    /// The `deadpool-postgres` pool itself.
    pub fn deadpool(&self) -> &Pool {
        &self.pool
    }

    /// How many connections the pool holds now, idle or in use (sqlx's `size`).
    pub fn size(&self) -> u32 {
        u32::try_from(self.pool.status().size).unwrap_or(u32::MAX)
    }

    /// How long a call waits for a connection.
    pub fn wait(&self) -> Duration {
        self.pool.timeouts().wait.unwrap_or(DEFAULT_WAIT)
    }

    /// A connection of the pool, checked before it is handed out. A database that refuses
    /// connections because it is starting (the connection refused, SQLSTATE `53300` or `57P03`)
    /// is tried again until the wait is over, as sqlx's pool did.
    ///
    /// # Errors
    /// [`SqlError::Pool`] (no connection in time, the pool closed, the database's refusal) or
    /// [`SqlError::TimedOut`].
    pub async fn acquire(&self) -> Result<Object, SqlError> {
        retrying(
            self.wait(),
            || self.pool.get(),
            |e| matches!(e, PoolError::Backend(e) if refused_while_starting(e)),
        )
        .await
        .map_err(|e| e.into_sql_error(SqlError::Pool))
    }

    /// `BEGIN` on a connection of the pool.
    ///
    /// # Errors
    /// As [`acquire`](Self::acquire), and the driver's error for `BEGIN`.
    pub async fn begin(&self) -> Result<Tx, SqlError> {
        self.begin_with("BEGIN").await
    }

    /// `statement` (a `BEGIN ...`) on a connection of the pool.
    ///
    /// # Errors
    /// As [`begin`](Self::begin).
    pub async fn begin_with(&self, statement: &str) -> Result<Tx, SqlError> {
        Tx::begin(self.acquire().await?, statement).await
    }

    /// Closes the pool: every later call fails as a closed pool ([`SqlError::Pool`]), idle
    /// connections are dropped, a connection in use is closed when it is given back, and a
    /// [`PgWakeup`](crate::PgWakeup) listening for the pool stops. Then it waits, at most
    /// [`LINGER`] (one second), for those connections (and that listener's) to end as a client
    /// ends one, with `Terminate` and, under TLS, `close_notify`, so that a process that exits
    /// next does not cut them (the server would log `could not receive data from client` for
    /// each). sqlx's `close` waited, without a bound, for the connections in use.
    pub async fn close(&self) {
        self.pool.close();
        self.closed.send_replace(true);
        if !self.tasks.ended_within(LINGER).await {
            tracing::debug!(
                running = self.tasks.running(),
                "postgres connections still open when the pool closed"
            );
        }
    }

    /// The tasks that drive the pool's connections, and the listener's of a
    /// [`PgWakeup::start`](crate::PgWakeup::start) on it.
    pub(crate) fn tasks(&self) -> &ConnectionTasks {
        &self.tasks
    }

    /// Whether [`close`](Self::close) was called.
    pub fn is_closed(&self) -> bool {
        self.pool.is_closed()
    }

    /// Resolves when the pool is closed.
    pub(crate) fn closed(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut closed = self.closed.subscribe();
        async move {
            let _ = closed.wait_for(|c| *c).await;
        }
    }
}

/// How [`retrying`] ended without a value.
pub(crate) enum Retried<E> {
    /// The attempt failed in a way that is not tried again, or the time was up after it.
    Failed(E),
    /// An attempt was still under way when the time was up.
    TimedOut,
}

impl<E> Retried<E> {
    pub(crate) fn into_sql_error(self, err: impl FnOnce(E) -> SqlError) -> SqlError {
        match self {
            Retried::Failed(e) => err(e),
            Retried::TimedOut => SqlError::TimedOut,
        }
    }
}

/// `attempt` until it succeeds, fails in a way `again` does not try again, or `wait` is over:
/// sqlx's backoff while a database starts (10 ms, doubled, at most a fifth of the wait). A wait too
/// long to add to the clock (`Duration::MAX`) is no deadline: the backoff then stops at a second.
pub(crate) async fn retrying<T, E, F>(
    wait: Duration,
    mut attempt: impl FnMut() -> F,
    again: impl Fn(&E) -> bool,
) -> Result<T, Retried<E>>
where
    F: Future<Output = Result<T, E>>,
{
    let deadline = tokio::time::Instant::now().checked_add(wait);
    let mut backoff = Duration::from_millis(10);
    let max_backoff = match deadline {
        Some(_) => wait / 5,
        None => Duration::from_secs(1),
    };
    loop {
        let outcome = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(left, attempt()).await {
                    Ok(outcome) => outcome,
                    Err(_) => return Err(Retried::TimedOut),
                }
            }
            None => attempt().await,
        };
        let in_time = |deadline: tokio::time::Instant| {
            tokio::time::Instant::now()
                .checked_add(backoff)
                .is_some_and(|next| next < deadline)
        };
        match outcome {
            Ok(value) => return Ok(value),
            Err(e) if again(&e) && deadline.is_none_or(in_time) => {}
            Err(e) => return Err(Retried::Failed(e)),
        }
        tokio::time::sleep(backoff).await;
        backoff = backoff.saturating_mul(2).min(max_backoff);
    }
}

/// A transaction on a connection of a pool: begun by a statement of the caller's (`BEGIN`, `BEGIN
/// ISOLATION LEVEL READ COMMITTED`), ended by [`commit`](Self::commit) or
/// [`rollback`](Self::rollback). One that is dropped unfinished is rolled back before its
/// connection goes back to the pool (in a task of its own); a connection whose `ROLLBACK` fails is
/// closed instead.
///
/// It dereferences to its connection, so `&mut *tx` is an [`Executor`] whose statements run in it.
pub struct Tx {
    conn: Option<Object>,
}

impl std::fmt::Debug for Tx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tx")
    }
}

impl Tx {
    /// Runs `statement` (a `BEGIN ...`) on `conn`.
    ///
    /// # Errors
    /// The driver's error; the connection is then closed.
    pub async fn begin(conn: Object, statement: &str) -> Result<Self, SqlError> {
        if let Err(e) = batch_on(&conn, statement).await {
            drop(Object::take(conn));
            return Err(e);
        }
        Ok(Self { conn: Some(conn) })
    }

    /// `COMMIT`, and the connection back to its pool.
    ///
    /// # Errors
    /// The driver's error; the connection is then closed.
    pub async fn commit(mut self) -> Result<(), SqlError> {
        self.finish("COMMIT").await
    }

    /// `ROLLBACK`, and the connection back to its pool.
    ///
    /// # Errors
    /// The driver's error; the connection is then closed.
    pub async fn rollback(mut self) -> Result<(), SqlError> {
        self.finish("ROLLBACK").await
    }

    async fn finish(&mut self, statement: &str) -> Result<(), SqlError> {
        let Some(conn) = self.conn.take() else {
            return Ok(());
        };
        match batch_on(&conn, statement).await {
            Ok(()) => Ok(()),
            Err(e) => {
                drop(Object::take(conn));
                Err(e)
            }
        }
    }
}

impl std::ops::Deref for Tx {
    type Target = Object;

    fn deref(&self) -> &Object {
        // `conn` is `None` only inside `finish` and `drop`, which consume the transaction
        self.conn
            .as_ref()
            .unwrap_or_else(|| unreachable!("a finished transaction is used"))
    }
}

impl std::ops::DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut Object {
        self.conn
            .as_mut()
            .unwrap_or_else(|| unreachable!("a finished transaction is used"))
    }
}

impl Drop for Tx {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let mut rolling_back = RollingBack(Some(conn));
                runtime.spawn(async move {
                    let rolled_back = match &rolling_back.0 {
                        Some(conn) => conn.batch_execute("ROLLBACK").await.is_ok(),
                        None => false,
                    };
                    if rolled_back {
                        // back to the pool, out of the transaction
                        drop(rolling_back.0.take());
                    }
                });
            }
            // no runtime to roll back on: the connection is closed, and the server rolls back
            Err(_) => drop(Object::take(conn)),
        }
    }
}

/// The connection of a dropped transaction while its `ROLLBACK` runs: closed (taken out of the
/// pool) unless the `ROLLBACK` succeeded, also when the task is dropped before it ends (the runtime
/// shutting down), so a connection never goes back to the pool inside a transaction.
struct RollingBack(Option<Object>);

impl Drop for RollingBack {
    fn drop(&mut self) {
        if let Some(conn) = self.0.take() {
            drop(Object::take(conn));
        }
    }
}

/// A parameter as sqlx sent it: the value and the type sqlx declared for its Rust type.
pub trait Encode: ToSql + Send + Sync {
    /// The type the statement declares for the parameter.
    fn pg_type() -> Type;
}

macro_rules! encode {
    ($($t:ty => $ty:expr),+ $(,)?) => {
        $(impl Encode for $t {
            fn pg_type() -> Type {
                $ty
            }
        })+
    };
}

encode! {
    bool => Type::BOOL,
    i16 => Type::INT2,
    i32 => Type::INT4,
    i64 => Type::INT8,
    String => Type::TEXT,
    &str => Type::TEXT,
    uuid::Uuid => Type::UUID,
    jiff::Timestamp => Type::TIMESTAMPTZ,
    serde_json::Value => Type::JSONB,
    Vec<u8> => Type::BYTEA,
    &[u8] => Type::BYTEA,
    Vec<String> => Type::TEXT_ARRAY,
    &[String] => Type::TEXT_ARRAY,
    Vec<&str> => Type::TEXT_ARRAY,
    &[&str] => Type::TEXT_ARRAY,
    Vec<uuid::Uuid> => Type::UUID_ARRAY,
    &[uuid::Uuid] => Type::UUID_ARRAY,
    Vec<i64> => Type::INT8_ARRAY,
    &[i64] => Type::INT8_ARRAY,
    Vec<i32> => Type::INT4_ARRAY,
    &[i32] => Type::INT4_ARRAY,
    Vec<serde_json::Value> => Type::JSONB_ARRAY,
    &[serde_json::Value] => Type::JSONB_ARRAY,
    Vec<jiff::Timestamp> => Type::TIMESTAMPTZ_ARRAY,
    &[jiff::Timestamp] => Type::TIMESTAMPTZ_ARRAY,
}

impl<T: Encode> Encode for Option<T> {
    fn pg_type() -> Type {
        T::pg_type()
    }
}

impl<T: Encode> Encode for &T {
    fn pg_type() -> Type {
        T::pg_type()
    }
}

/// Where a statement runs: a pool (a connection of it, for the one statement), or a connection of
/// one (a transaction's, through `&mut *tx`). Made from `&PgPool`, `&Object` and `&mut Object`.
#[derive(Clone, Copy)]
pub enum Executor<'c> {
    /// A connection of the pool, for the one statement.
    Pool(&'c PgPool),
    /// This connection.
    Conn(&'c Object),
    /// A connection of no pool (its statements are prepared each time: it has no cache).
    Client(&'c tokio_postgres::Client),
}

impl<'c> From<&'c tokio_postgres::Client> for Executor<'c> {
    fn from(client: &'c tokio_postgres::Client) -> Self {
        Executor::Client(client)
    }
}

impl<'c> From<&'c mut tokio_postgres::Client> for Executor<'c> {
    fn from(client: &'c mut tokio_postgres::Client) -> Self {
        Executor::Client(client)
    }
}

impl<'c> From<&'c PgPool> for Executor<'c> {
    fn from(pool: &'c PgPool) -> Self {
        Executor::Pool(pool)
    }
}

impl<'c> From<&'c Object> for Executor<'c> {
    fn from(conn: &'c Object) -> Self {
        Executor::Conn(conn)
    }
}

impl<'c> From<&'c mut Object> for Executor<'c> {
    fn from(conn: &'c mut Object) -> Self {
        Executor::Conn(conn)
    }
}

impl<'c> From<&'c Tx> for Executor<'c> {
    fn from(tx: &'c Tx) -> Self {
        Executor::Conn(tx)
    }
}

impl<'c> From<&'c mut Tx> for Executor<'c> {
    fn from(tx: &'c mut Tx) -> Self {
        Executor::Conn(tx)
    }
}

impl Executor<'_> {
    async fn rows(
        self,
        sql: &str,
        types: &[Type],
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, SqlError> {
        match self {
            Executor::Pool(pool) => rows_on(&pool.acquire().await?, sql, types, params).await,
            Executor::Conn(conn) => rows_on(conn, sql, types, params).await,
            Executor::Client(client) => {
                let started = Instant::now();
                let statement = client.prepare_typed(sql, types).await?;
                let rows = client.query(&statement, params).await;
                logged(sql, started);
                Ok(rows?)
            }
        }
    }

    async fn exec(
        self,
        sql: &str,
        types: &[Type],
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<u64, SqlError> {
        match self {
            Executor::Pool(pool) => exec_on(&pool.acquire().await?, sql, types, params).await,
            Executor::Conn(conn) => exec_on(conn, sql, types, params).await,
            Executor::Client(client) => {
                let started = Instant::now();
                let statement = client.prepare_typed(sql, types).await?;
                let done = client.execute(&statement, params).await;
                logged(sql, started);
                Ok(done?)
            }
        }
    }

    async fn batch(self, sql: &str) -> Result<(), SqlError> {
        match self {
            Executor::Pool(pool) => batch_on(&pool.acquire().await?, sql).await,
            Executor::Conn(conn) => batch_on(conn, sql).await,
            Executor::Client(client) => {
                let started = Instant::now();
                let done = client.batch_execute(sql).await;
                logged(sql, started);
                Ok(done?)
            }
        }
    }
}

async fn rows_on(
    conn: &Object,
    sql: &str,
    types: &[Type],
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Row>, SqlError> {
    let started = Instant::now();
    let statement = conn.prepare_typed_cached(sql, types).await?;
    let rows = conn.query(&statement, params).await;
    logged(sql, started);
    Ok(rows?)
}

async fn exec_on(
    conn: &Object,
    sql: &str,
    types: &[Type],
    params: &[&(dyn ToSql + Sync)],
) -> Result<u64, SqlError> {
    let started = Instant::now();
    let statement = conn.prepare_typed_cached(sql, types).await?;
    let done = conn.execute(&statement, params).await;
    logged(sql, started);
    Ok(done?)
}

async fn batch_on(conn: &Object, sql: &str) -> Result<(), SqlError> {
    let started = Instant::now();
    let done = conn.batch_execute(sql).await;
    logged(sql, started);
    Ok(done?)
}

fn logged(sql: &str, started: Instant) {
    let elapsed = started.elapsed();
    let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    if elapsed >= SLOW_STATEMENT {
        tracing::warn!(target: LOG_TARGET, sql, elapsed_ms, "slow statement");
    } else {
        tracing::debug!(target: LOG_TARGET, sql, elapsed_ms, "statement");
    }
}

/// A statement and its parameters, typed as sqlx typed them.
#[must_use = "a statement does nothing until it is run"]
pub struct Query<'q> {
    sql: Cow<'q, str>,
    params: Vec<Box<dyn ToSql + Send + Sync + 'q>>,
    types: Vec<Type>,
}

/// A statement: `query(sql).bind(a).bind(b).execute(&pool)`.
pub fn query<'q>(sql: impl Into<Cow<'q, str>>) -> Query<'q> {
    Query {
        sql: sql.into(),
        params: Vec::new(),
        types: Vec::new(),
    }
}

/// What [`Query::execute`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done(u64);

impl Done {
    /// How many rows the statement touched.
    pub fn rows_affected(self) -> u64 {
        self.0
    }
}

impl<'q> Query<'q> {
    /// The next parameter.
    pub fn bind<T: Encode + 'q>(mut self, value: T) -> Self {
        self.types.push(T::pg_type());
        self.params.push(Box::new(value));
        self
    }

    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.params
            .iter()
            .map(|p| &**p as &(dyn ToSql + Sync))
            .collect()
    }

    /// Runs the statement.
    ///
    /// # Errors
    /// The pool's or the driver's error.
    pub async fn execute(self, ex: impl Into<Executor<'_>>) -> Result<Done, SqlError> {
        let params = self.refs();
        ex.into()
            .exec(&self.sql, &self.types, &params)
            .await
            .map(Done)
    }

    /// Every row.
    ///
    /// # Errors
    /// As [`execute`](Self::execute).
    pub async fn fetch_all(self, ex: impl Into<Executor<'_>>) -> Result<Vec<Row>, SqlError> {
        let params = self.refs();
        ex.into().rows(&self.sql, &self.types, &params).await
    }

    /// The first row, if there is one.
    ///
    /// # Errors
    /// As [`execute`](Self::execute).
    pub async fn fetch_optional(
        self,
        ex: impl Into<Executor<'_>>,
    ) -> Result<Option<Row>, SqlError> {
        Ok(self.fetch_all(ex).await?.into_iter().next())
    }

    /// The first row, which must be there ([`SqlError::RowNotFound`] otherwise).
    ///
    /// # Errors
    /// As [`execute`](Self::execute), and [`SqlError::RowNotFound`].
    pub async fn fetch_one(self, ex: impl Into<Executor<'_>>) -> Result<Row, SqlError> {
        self.fetch_optional(ex).await?.ok_or(SqlError::RowNotFound)
    }
}

/// A row read into a value: a tuple of columns, read in order.
pub trait FromRow: Sized {
    /// Reads `row`.
    ///
    /// # Errors
    /// The driver's error for a column that is not there or not of the type.
    fn from_row(row: &Row) -> Result<Self, SqlError>;
}

macro_rules! tuple_from_row {
    ($($t:ident $i:tt),+) => {
        impl<$($t: for<'a> FromSql<'a>),+> FromRow for ($($t,)+) {
            fn from_row(row: &Row) -> Result<Self, SqlError> {
                Ok(($(row.try_get::<_, $t>($i)?,)+))
            }
        }
    };
}

tuple_from_row!(A 0);
tuple_from_row!(A 0, B 1);
tuple_from_row!(A 0, B 1, C 2);
tuple_from_row!(A 0, B 1, C 2, D 3);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9);

/// A statement whose rows are read with [`FromRow`].
#[must_use = "a statement does nothing until it is run"]
pub struct QueryAs<'q, T> {
    query: Query<'q>,
    row: PhantomData<fn() -> T>,
}

/// A statement whose rows are tuples: `query_as::<(i64, String)>(sql)`.
pub fn query_as<'q, T: FromRow>(sql: impl Into<Cow<'q, str>>) -> QueryAs<'q, T> {
    QueryAs {
        query: query(sql),
        row: PhantomData,
    }
}

impl<'q, T: FromRow> QueryAs<'q, T> {
    /// The next parameter.
    pub fn bind<V: Encode + 'q>(self, value: V) -> Self {
        Self {
            query: self.query.bind(value),
            row: PhantomData,
        }
    }

    /// Every row.
    ///
    /// # Errors
    /// As [`Query::execute`], and a row that does not read as `T`.
    pub async fn fetch_all(self, ex: impl Into<Executor<'_>>) -> Result<Vec<T>, SqlError> {
        self.query
            .fetch_all(ex)
            .await?
            .iter()
            .map(T::from_row)
            .collect()
    }

    /// The first row, if there is one.
    ///
    /// # Errors
    /// As [`fetch_all`](Self::fetch_all).
    pub async fn fetch_optional(self, ex: impl Into<Executor<'_>>) -> Result<Option<T>, SqlError> {
        self.query
            .fetch_optional(ex)
            .await?
            .as_ref()
            .map(T::from_row)
            .transpose()
    }

    /// The first row, which must be there.
    ///
    /// # Errors
    /// As [`fetch_all`](Self::fetch_all), and [`SqlError::RowNotFound`].
    pub async fn fetch_one(self, ex: impl Into<Executor<'_>>) -> Result<T, SqlError> {
        self.fetch_optional(ex).await?.ok_or(SqlError::RowNotFound)
    }
}

/// A statement whose rows are one column each.
#[must_use = "a statement does nothing until it is run"]
pub struct QueryScalar<'q, T> {
    query: Query<'q>,
    value: PhantomData<fn() -> T>,
}

/// A statement of one column: `query_scalar::<i64>("SELECT count(*) ...")`.
pub fn query_scalar<'q, T: for<'a> FromSql<'a>>(
    sql: impl Into<Cow<'q, str>>,
) -> QueryScalar<'q, T> {
    QueryScalar {
        query: query(sql),
        value: PhantomData,
    }
}

impl<'q, T: for<'a> FromSql<'a>> QueryScalar<'q, T> {
    /// The next parameter.
    pub fn bind<V: Encode + 'q>(self, value: V) -> Self {
        Self {
            query: self.query.bind(value),
            value: PhantomData,
        }
    }

    /// Every row's value.
    ///
    /// # Errors
    /// As [`Query::execute`], and a value that does not read as `T`.
    pub async fn fetch_all(self, ex: impl Into<Executor<'_>>) -> Result<Vec<T>, SqlError> {
        self.query
            .fetch_all(ex)
            .await?
            .iter()
            .map(|row| row.try_get::<_, T>(0).map_err(SqlError::from))
            .collect()
    }

    /// The first row's value, if there is a row.
    ///
    /// # Errors
    /// As [`fetch_all`](Self::fetch_all).
    pub async fn fetch_optional(self, ex: impl Into<Executor<'_>>) -> Result<Option<T>, SqlError> {
        self.query
            .fetch_optional(ex)
            .await?
            .map(|row| row.try_get::<_, T>(0).map_err(SqlError::from))
            .transpose()
    }

    /// The first row's value, which must be there.
    ///
    /// # Errors
    /// As [`fetch_all`](Self::fetch_all), and [`SqlError::RowNotFound`].
    pub async fn fetch_one(self, ex: impl Into<Executor<'_>>) -> Result<T, SqlError> {
        self.fetch_optional(ex).await?.ok_or(SqlError::RowNotFound)
    }
}

/// Runs `sql` (several statements, no parameter) on the simple query protocol, as sqlx's
/// `raw_sql` did.
///
/// # Errors
/// As [`Query::execute`].
pub async fn raw_sql(sql: &str, ex: impl Into<Executor<'_>>) -> Result<(), SqlError> {
    ex.into().batch(sql).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameter_is_declared_with_the_type_sqlx_sent() {
        for (got, want) in [
            (<bool>::pg_type(), Type::BOOL),
            (<i16>::pg_type(), Type::INT2),
            (<i32>::pg_type(), Type::INT4),
            (<i64>::pg_type(), Type::INT8),
            (<&str>::pg_type(), Type::TEXT),
            (<Option<&String>>::pg_type(), Type::TEXT),
            (<uuid::Uuid>::pg_type(), Type::UUID),
            (<jiff::Timestamp>::pg_type(), Type::TIMESTAMPTZ),
            (<serde_json::Value>::pg_type(), Type::JSONB),
            (<&[u8]>::pg_type(), Type::BYTEA),
            (<Vec<String>>::pg_type(), Type::TEXT_ARRAY),
            (<Option<Vec<uuid::Uuid>>>::pg_type(), Type::UUID_ARRAY),
        ] {
            assert_eq!(got, want);
        }
    }

    #[test]
    fn a_wait_too_long_for_the_clock_has_no_deadline() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build();
        let Ok(runtime) = runtime else {
            panic!("no runtime");
        };
        let mut tries = 0;
        let outcome: Result<u32, Retried<u32>> = runtime.block_on(retrying(
            Duration::MAX,
            || {
                tries += 1;
                let n = tries;
                async move { if n < 3 { Err(n) } else { Ok(n) } }
            },
            |_| true,
        ));
        assert!(matches!(outcome, Ok(3)));
    }

    #[test]
    fn the_backoff_doubles_from_ten_milliseconds_to_a_fifth_of_the_wait() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build();
        let Ok(runtime) = runtime else {
            panic!("no runtime");
        };
        let tries = std::sync::Mutex::new(Vec::new());
        let start = tokio::time::Instant::now();
        let outcome: Result<(), Retried<()>> = runtime.block_on(async {
            let tries = &tries;
            retrying(
                Duration::from_millis(500),
                || async move {
                    if let Ok(mut tries) = tries.lock() {
                        tries.push(start.elapsed());
                    }
                    Err(())
                },
                |_| true,
            )
            .await
        });
        assert!(matches!(outcome, Err(Retried::Failed(()))));
        let tries = tries.into_inner().unwrap_or_default();
        let gaps: Vec<u128> = tries
            .windows(2)
            .map(|w| ((w[1] - w[0]).as_micros() + 500) / 1000)
            .collect();
        // 10, 20, 40, 80, then a fifth of 500 ms, until the next would pass the deadline
        assert_eq!(gaps[..5], [10, 20, 40, 80, 100], "{gaps:?}");
        assert!(gaps.iter().all(|g| *g <= 100), "{gaps:?}");
        assert!(
            tries
                .last()
                .is_some_and(|t| *t < Duration::from_millis(500))
        );
    }
}
