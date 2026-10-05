//! `Wakeup` over Postgres `LISTEN/NOTIFY`.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::{AgentId, LiveChunk, LiveEnd, LiveKind, LiveText, ThreadId};
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
/// Channel carrying `Topic::Inbox`; the payload is empty.
pub(crate) const CHANNEL_INBOX: &str = "orch_inbox";
/// Channel carrying `Topic::Resync`; the payload is empty.
pub(crate) const CHANNEL_RESYNC: &str = "orch_resync";
/// Channel carrying live text (ADR 0027); the payload is the JSON of [`LiveWire`].
pub(crate) const CHANNEL_LIVE: &str = "orch_live";

const CHANNELS: [&str; 5] = [
    CHANNEL_THREAD,
    CHANNEL_OUTBOX,
    CHANNEL_INBOX,
    CHANNEL_RESYNC,
    CHANNEL_LIVE,
];
/// The most a live payload may be, in bytes. `NOTIFY` carries less than 8000 bytes in the default
/// configuration (*verified 2026-10-01*, postgresql.org/docs/16/sql-notify.html), and a margin is
/// kept. A piece whose JSON is longer is split by [`encode_live`], not refused.
const LIVE_PAYLOAD_LIMIT: usize = 7900;
/// The room a payload must leave for text, so that any one character (at most 6 bytes of JSON) fits
/// in a part of its own.
const LIVE_MIN_TEXT_ROOM: usize = 8;
const CAPACITY: usize = 1024;
const BACKOFF_START: Duration = Duration::from_millis(200);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

fn encode(topic: &Topic) -> (&'static str, String) {
    match topic {
        Topic::Thread(id) => (CHANNEL_THREAD, id.to_string()),
        Topic::Outbox => (CHANNEL_OUTBOX, String::new()),
        Topic::Inbox => (CHANNEL_INBOX, String::new()),
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
        CHANNEL_INBOX => Topic::Inbox,
        _ => Topic::Resync,
    }
}

struct Shared {
    pool: PgPool,
    tx: broadcast::Sender<Topic>,
    live: broadcast::Sender<LiveText>,
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
/// Live text (ADR 0027) travels on its own channel, `orch_live`, over the same listener: a
/// piece is a JSON payload ([`encode_live`] splits one that would not fit the 8000 bytes of a
/// `NOTIFY`), it is delivered to every subscriber of every process, and nothing stores it. It is
/// best effort in both directions: a reconnect or a lagging subscriber loses pieces without a
/// `Resync`, because the sender repeats the text so far and the log has the final one.
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
        let (live, _) = broadcast::channel(CAPACITY);
        let (listening_tx, listening) = watch::channel(false);
        let task = tokio::spawn(listen_loop(
            pool.clone(),
            tx.clone(),
            live.clone(),
            listening_tx,
        ));
        PgWakeup {
            shared: Arc::new(Shared {
                pool,
                tx,
                live,
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
        WakeupCapabilities {
            push: true,
            live: true,
        }
    }

    async fn publish_live(&self, live: LiveText) -> Result<(), WakeupError> {
        // The pieces go out one `NOTIFY` after the other, each in its own transaction, so the
        // listeners hear them in this order.
        for payload in encode_live(&live)? {
            sqlx::query("SELECT pg_notify($1, $2)")
                .bind(CHANNEL_LIVE)
                .bind(payload)
                .execute(&self.shared.pool)
                .await
                .map_err(wakeup_err)?;
        }
        Ok(())
    }

    fn subscribe_live(&self) -> BoxStream<'static, LiveText> {
        // A lagging subscriber loses pieces without a word: the sender repeats the text so far.
        BroadcastStream::new(self.shared.live.subscribe())
            .filter_map(|item| std::future::ready(item.ok()))
            .boxed()
    }
}

/// The payload of a live piece: `t` thread, `a` agent, `m` stream id, `o` byte offset, `e` the end
/// (`o`, `l` or `a`, [`LiveEnd::code`]) and the text, in `x` for a reply (the payload of every build
/// before reasoning, byte for byte) or in `y`, with `k: "r"`, for reasoning (ADR 0044).
///
/// Reasoning has **its own member for the text** so that a replica that predates it, which requires `x`,
/// cannot read the payload and drops it (debug log) instead of showing reasoning as the reply that is
/// being written: the same rule as the agent's side of the wire (a new `RunEvent` variant).
#[derive(serde::Serialize, serde::Deserialize)]
struct LiveWire {
    t: ThreadId,
    a: String,
    m: String,
    o: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<String>,
    e: char,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    k: Option<char>,
}

/// What `c` takes in a JSON string, in bytes (`serde_json`: the quote, the backslash and the
/// control characters are escaped, everything else is written as it is).
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// The payloads that carry `live`, in order: one, or several when the JSON of the text would not
/// fit [`LIVE_PAYLOAD_LIMIT`]. A split is at a character boundary, each part keeps its byte offset
/// in the stream, and only the last one carries the piece's end, so a receiver that joins them by
/// offset reads the same text.
fn encode_live(live: &LiveText) -> Result<Vec<String>, WakeupError> {
    let wire = |offset: u64, text: &str, end: LiveEnd| LiveWire {
        t: live.thread,
        a: live.agent.as_str().to_string(),
        m: live.chunk.message_id.clone(),
        o: offset,
        x: (live.chunk.kind == LiveKind::Reply).then(|| text.to_string()),
        y: (live.chunk.kind == LiveKind::Reasoning).then(|| text.to_string()),
        e: end.code(),
        k: (live.chunk.kind == LiveKind::Reasoning).then(|| LiveKind::Reasoning.code()),
    };
    let to_json = |w: &LiveWire| {
        // Plain strings and numbers: this cannot fail.
        serde_json::to_string(w).unwrap_or_default()
    };
    // The envelope at its widest (the offset with the most digits), with no text.
    let overhead = to_json(&wire(u64::MAX, "", LiveEnd::Abandoned)).len();
    if overhead + LIVE_MIN_TEXT_ROOM > LIVE_PAYLOAD_LIMIT {
        return Err(WakeupError::PayloadTooLarge {
            len: overhead,
            limit: LIVE_PAYLOAD_LIMIT,
        });
    }
    let budget = LIVE_PAYLOAD_LIMIT - overhead;

    let mut parts: Vec<(u64, &str)> = Vec::new();
    let text = live.chunk.text.as_str();
    let (mut start, mut used) = (0usize, 0usize);
    for (at, c) in text.char_indices() {
        let width = json_len(c);
        if used + width > budget {
            parts.push((live.chunk.offset + start as u64, &text[start..at]));
            start = at;
            used = 0;
        }
        used += width;
    }
    parts.push((live.chunk.offset + start as u64, &text[start..]));

    let last = parts.len() - 1;
    Ok(parts
        .into_iter()
        .enumerate()
        .map(|(i, (offset, piece))| {
            let end = if i == last {
                live.chunk.end
            } else {
                LiveEnd::Open
            };
            to_json(&wire(offset, piece, end))
        })
        .collect())
}

/// A payload that is not what this build writes (another version, a stray `NOTIFY`) is not live
/// text: it is dropped, and the viewers see the words at the next refresh or in the log.
fn decode_live(payload: &str) -> Option<LiveText> {
    let wire: LiveWire = serde_json::from_str(payload).ok()?;
    let kind = match wire.k {
        None => LiveKind::Reply,
        Some(code) => LiveKind::from_code(code)?,
    };
    // A reply says its text in `x`, reasoning in `y`; a payload that says it in the other is not ours.
    let text = match kind {
        LiveKind::Reply => wire.x.filter(|_| wire.y.is_none())?,
        LiveKind::Reasoning => wire.y.filter(|_| wire.x.is_none())?,
    };
    Some(LiveText {
        thread: wire.t,
        agent: AgentId::new(wire.a),
        chunk: LiveChunk {
            message_id: wire.m,
            offset: wire.o,
            text,
            end: LiveEnd::from_code(wire.e)?,
            kind,
        },
    })
}

async fn attach(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen_all(CHANNELS).await?;
    Ok(listener)
}

async fn listen_loop(
    pool: PgPool,
    tx: broadcast::Sender<Topic>,
    live: broadcast::Sender<LiveText>,
    listening: watch::Sender<bool>,
) {
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
                Ok(Some(n)) if n.channel() == CHANNEL_LIVE => match decode_live(n.payload()) {
                    // No subscriber is not an error: live text is best effort.
                    Some(text) => {
                        let _ = live.send(text);
                    }
                    None => tracing::debug!("a live payload this build cannot read was dropped"),
                },
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn piece(offset: u64, text: &str, end: LiveEnd) -> LiveText {
        LiveText {
            thread: ThreadId(uuid::Uuid::from_u128(
                0x7000_8000_0000_0000_0000_0000_0000_0042,
            )),
            agent: AgentId::new("coder"),
            chunk: LiveChunk {
                message_id: "S".into(),
                offset,
                text: text.into(),
                end,
                kind: LiveKind::Reply,
            },
        }
    }

    fn thought(offset: u64, text: &str, end: LiveEnd) -> LiveText {
        let mut live = piece(offset, text, end);
        live.chunk.kind = LiveKind::Reasoning;
        live
    }

    /// Reads every payload back and joins the text by byte offset.
    fn rejoin(payloads: &[String]) -> (Vec<LiveText>, String) {
        let decoded: Vec<LiveText> = payloads.iter().map(|p| decode_live(p).unwrap()).collect();
        let mut text = String::new();
        let first = decoded[0].chunk.offset;
        let mut next = first;
        for d in &decoded {
            assert_eq!(d.chunk.offset, next, "parts follow one another");
            next += d.chunk.text.len() as u64;
            text.push_str(&d.chunk.text);
        }
        (decoded, text)
    }

    /// Reasoning round-trips, and its payload is not a reply's: it says its text in `y` and its kind in
    /// `k`, so a replica that predates it (which needs `x`) drops it instead of showing it as the reply
    /// being written. A reply's payload is what it always was.
    #[test]
    fn reasoning_round_trips_in_a_payload_a_replica_that_predates_it_cannot_read() {
        let live = thought(0, "The user asks \"why\"", LiveEnd::Open);
        let payloads = encode_live(&live).unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(decode_live(&payloads[0]), Some(live));
        let value: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert!(value.get("x").is_none(), "{value}");
        assert_eq!(value["k"], "r");
        // What a replica of the earlier build reads: `x` is required.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Earlier {
            t: ThreadId,
            a: String,
            m: String,
            o: u64,
            x: String,
            e: char,
        }
        assert!(serde_json::from_str::<Earlier>(&payloads[0]).is_err());
        let reply = encode_live(&piece(0, "hi", LiveEnd::Open)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&reply[0]).unwrap();
        assert_eq!(value.get("k"), None);
        assert_eq!(value["x"], "hi");
        assert!(serde_json::from_str::<Earlier>(&reply[0]).is_ok());
        // A payload that says its text in the member of the other kind is not ours.
        assert_eq!(
            decode_live(
                r#"{"t":"70008000-0000-0000-0000-000000000042","a":"c","m":"S","o":0,"y":"x","e":"o"}"#
            ),
            None
        );
        assert_eq!(
            decode_live(
                r#"{"t":"70008000-0000-0000-0000-000000000042","a":"c","m":"S","o":0,"x":"x","e":"o","k":"r"}"#
            ),
            None
        );
    }

    #[test]
    fn a_piece_round_trips_through_its_payload() {
        for end in [LiveEnd::Open, LiveEnd::Last, LiveEnd::Abandoned] {
            let live = piece(18, "onacci \u{e9} \"q\"\n\u{0}\u{1f980}", end);
            let payloads = encode_live(&live).unwrap();
            assert_eq!(payloads.len(), 1);
            assert_eq!(decode_live(&payloads[0]), Some(live));
        }
        // Empty text is a piece too (the last one of a stream often is).
        let empty = piece(0, "", LiveEnd::Last);
        assert_eq!(decode_live(&encode_live(&empty).unwrap()[0]), Some(empty));
    }

    #[test]
    fn the_payload_is_the_documented_json() {
        let live = piece(3, "hi", LiveEnd::Open);
        let payload = &encode_live(&live).unwrap()[0];
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "t": "70008000-0000-0000-0000-000000000042",
                "a": "coder", "m": "S", "o": 3, "x": "hi", "e": "o"
            })
        );
    }

    #[test]
    fn what_this_build_does_not_write_is_not_live_text() {
        for payload in [
            "",
            "not json",
            "{}",
            r#"{"t":"nope","a":"coder","m":"S","o":0,"x":"","e":"o"}"#,
            r#"{"t":"70008000-0000-0000-0000-000000000042","a":"coder","m":"S","o":0,"x":"","e":"?"}"#,
            r#"{"t":"70008000-0000-0000-0000-000000000042","a":"coder","m":"S","o":-1,"x":"","e":"o"}"#,
        ] {
            assert_eq!(decode_live(payload), None, "{payload:?}");
        }
    }

    #[test]
    fn no_payload_reaches_the_notify_limit_even_when_every_character_is_escaped() {
        // Each of these costs 2 to 6 bytes of JSON per byte of text.
        for unit in [
            "\"",
            "\n",
            "\u{0}",
            "\\",
            "\u{e9}",
            "\u{4e2d}",
            "\u{1f980}",
            "a",
        ] {
            let text = unit.repeat(orch_core::MAX_LIVE_PIECE_BYTES / unit.len());
            let live = piece(u64::MAX - 1_000_000, &text, LiveEnd::Last);
            let payloads = encode_live(&live).unwrap();
            for p in &payloads {
                assert!(p.len() <= LIVE_PAYLOAD_LIMIT, "{unit:?}: {} bytes", p.len());
                assert!(p.len() < 8000);
            }
            let (decoded, joined) = rejoin(&payloads);
            assert_eq!(joined, text, "{unit:?}");
            // Only the last part ends the stream.
            let (last, rest) = decoded.split_last().unwrap();
            assert_eq!(last.chunk.end, LiveEnd::Last);
            assert!(rest.iter().all(|d| d.chunk.end == LiveEnd::Open));
            assert!(decoded.iter().all(|d| d.chunk.message_id == "S"));
        }
    }

    #[test]
    fn a_piece_that_fits_is_one_payload() {
        let live = piece(
            0,
            &"x".repeat(orch_core::MAX_LIVE_PIECE_BYTES),
            LiveEnd::Open,
        );
        assert_eq!(encode_live(&live).unwrap().len(), 1);
    }

    #[test]
    fn an_abandoned_end_survives_a_split() {
        let live = piece(0, &"\"".repeat(10_000), LiveEnd::Abandoned);
        let payloads = encode_live(&live).unwrap();
        assert!(payloads.len() > 1);
        let (decoded, _) = rejoin(&payloads);
        assert_eq!(decoded.last().unwrap().chunk.end, LiveEnd::Abandoned);
        assert!(
            decoded[..decoded.len() - 1]
                .iter()
                .all(|d| d.chunk.end == LiveEnd::Open)
        );
    }

    #[test]
    fn an_envelope_that_cannot_fit_is_refused_and_nothing_is_split() {
        let mut live = piece(0, "x", LiveEnd::Open);
        live.chunk.message_id = "m".repeat(LIVE_PAYLOAD_LIMIT);
        assert!(matches!(
            encode_live(&live),
            Err(WakeupError::PayloadTooLarge { .. })
        ));
    }
}
