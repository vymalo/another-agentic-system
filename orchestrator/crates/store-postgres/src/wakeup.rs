//! `Wakeup` over Postgres `LISTEN/NOTIFY`.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::ThreadId;
use orch_ports::{Topic, Wakeup, WakeupCapabilities, WakeupError};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::error::wakeup_err;

/// Channel carrying `Topic::Thread`; the payload is the thread id.
pub(crate) const CHANNEL_THREAD: &str = "orch_thread";
/// Channel carrying `Topic::Outbox`; the payload is empty.
pub(crate) const CHANNEL_OUTBOX: &str = "orch_outbox";
/// Channel carrying `Topic::Resync`; the payload is empty.
pub(crate) const CHANNEL_RESYNC: &str = "orch_resync";

const CHANNELS: [&str; 3] = [CHANNEL_THREAD, CHANNEL_OUTBOX, CHANNEL_RESYNC];
const CAPACITY: usize = 1024;
const BACKOFF_START: Duration = Duration::from_millis(200);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

fn encode(topic: &Topic) -> (&'static str, String) {
    match topic {
        Topic::Thread(id) => (CHANNEL_THREAD, id.to_string()),
        Topic::Outbox => (CHANNEL_OUTBOX, String::new()),
        Topic::Resync => (CHANNEL_RESYNC, String::new()),
    }
}

/// An undecodable message is not dropped silently: the safe reading of "something changed
/// that I do not understand" is to re-read everything.
fn decode(channel: &str, payload: &str) -> Topic {
    match channel {
        CHANNEL_THREAD => payload
            .parse::<ThreadId>()
            .map_or(Topic::Resync, Topic::Thread),
        CHANNEL_OUTBOX => Topic::Outbox,
        _ => Topic::Resync,
    }
}

struct Shared {
    pool: PgPool,
    tx: broadcast::Sender<Topic>,
    listening: watch::Receiver<bool>,
    task: JoinHandle<()>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// [`Wakeup`] over Postgres `LISTEN/NOTIFY`.
///
/// One background task holds a dedicated listener connection (taken from the pool, so the
/// pool needs at least two connections) and fans notifications out over a broadcast channel.
/// Whenever the listener had to reconnect, notifications may have been lost, so every
/// subscriber receives [`Topic::Resync`]; the same happens to a subscriber that lags.
/// Notifications are hints: consumers re-read the store and also poll.
///
/// Clones share the task; it stops when the last clone is dropped or the pool is closed.
/// [`PgStore`](crate::PgStore) already sends the `NOTIFY`s for its own writes inside the
/// writing transaction, so the application only needs [`Wakeup::subscribe`].
#[derive(Clone)]
pub struct PgWakeup {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for PgWakeup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PgWakeup")
    }
}

impl PgWakeup {
    /// Spawns the listener task on the current tokio runtime. Returns immediately; the
    /// listener attaches in the background (see [`wait_listening`](Self::wait_listening)) and
    /// keeps retrying with backoff if Postgres is unreachable.
    pub fn start(pool: PgPool) -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        let (listening_tx, listening) = watch::channel(false);
        let task = tokio::spawn(listen_loop(pool.clone(), tx.clone(), listening_tx));
        PgWakeup {
            shared: Arc::new(Shared {
                pool,
                tx,
                listening,
                task,
            }),
        }
    }

    /// Waits until the listener is attached to all channels; `false` on timeout. Only
    /// subscribers created afterwards are guaranteed to see notifications sent afterwards.
    pub async fn wait_listening(&self, timeout: Duration) -> bool {
        let mut rx = self.shared.listening.clone();
        tokio::time::timeout(timeout, rx.wait_for(|up| *up))
            .await
            .is_ok_and(|r| r.is_ok())
    }
}

impl Wakeup for PgWakeup {
    async fn notify(&self, topic: Topic) -> Result<(), WakeupError> {
        let (channel, payload) = encode(&topic);
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(channel)
            .bind(payload)
            .execute(&self.shared.pool)
            .await
            .map(|_| ())
            .map_err(wakeup_err)
    }

    fn subscribe(&self) -> BoxStream<'static, Topic> {
        BroadcastStream::new(self.shared.tx.subscribe())
            .map(|item| match item {
                Ok(topic) => topic,
                Err(BroadcastStreamRecvError::Lagged(_)) => Topic::Resync,
            })
            .boxed()
    }

    fn capabilities(&self) -> WakeupCapabilities {
        WakeupCapabilities { push: true }
    }
}

async fn attach(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen_all(CHANNELS).await?;
    Ok(listener)
}

async fn listen_loop(pool: PgPool, tx: broadcast::Sender<Topic>, listening: watch::Sender<bool>) {
    let mut backoff = BACKOFF_START;
    let mut first = true;
    loop {
        let mut listener = match attach(&pool).await {
            Ok(listener) => listener,
            Err(sqlx::Error::PoolClosed) => return,
            Err(e) => {
                tracing::warn!(error = %e, "postgres listener could not attach; retrying");
                let _ = listening.send(false);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        backoff = BACKOFF_START;
        let _ = listening.send(true);
        if !first {
            // Whatever happened while we were away, we did not hear about it.
            let _ = tx.send(Topic::Resync);
        }
        first = false;
        loop {
            match listener.try_recv().await {
                Ok(Some(n)) => {
                    // No subscriber is not an error: notifications are hints.
                    let _ = tx.send(decode(n.channel(), n.payload()));
                }
                Ok(None) => {
                    tracing::warn!("postgres listener connection lost; reconnecting");
                    let _ = listening.send(false);
                    break;
                }
                Err(sqlx::Error::PoolClosed) => return,
                Err(e) => {
                    tracing::warn!(error = %e, "postgres listener failed; reconnecting");
                    let _ = listening.send(false);
                    break;
                }
            }
        }
        // Never spin, even if attaching keeps succeeding and receiving keeps failing.
        tokio::time::sleep(BACKOFF_START).await;
    }
}
