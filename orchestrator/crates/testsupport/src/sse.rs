//! A tiny SSE client for tests: AG-UI streams, a `data:` JSON event per frame.

use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;

/// One AG-UI frame: a `data:` JSON event and, on a resume point, its `id:`. AG-UI streams carry
/// no `event:` name.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The `id:` field, the log's `seq`, when the frame is a resume point.
    pub id: Option<i64>,
    /// The event.
    pub event: serde_json::Value,
}

/// A streaming HTTP response parsed as server-sent events.
pub struct SseClient {
    stream: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
    buf: String,
    ended: bool,
    /// HTTP status of the response.
    pub status: reqwest::StatusCode,
    /// Response headers.
    pub headers: reqwest::header::HeaderMap,
}

impl SseClient {
    /// Wraps a response.
    pub fn from_response(resp: reqwest::Response) -> Self {
        SseClient {
            status: resp.status(),
            headers: resp.headers().clone(),
            stream: resp.bytes_stream().boxed(),
            buf: String::new(),
            ended: false,
        }
    }

    /// Whether the server closed the stream (as opposed to a read that timed out).
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// The next block (up to a blank line), or `None` on timeout or end of stream.
    async fn next_block(&mut self, deadline: tokio::time::Instant) -> Option<String> {
        loop {
            if let Some(pos) = self.buf.find("\n\n") {
                return Some(self.buf.drain(..pos + 2).collect());
            }
            let chunk = match tokio::time::timeout_at(deadline, self.stream.next()).await {
                Err(_) => return None,
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(_)) | None) => {
                    self.ended = true;
                    return None;
                }
            };
            self.buf.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    /// The next AG-UI frame (`data:` with an optional `id:`), skipping comments; `None` on
    /// timeout or end of stream.
    pub async fn next_frame(&mut self, within: Duration) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let block = self.next_block(deadline).await?;
            let (mut id, mut data) = (None, Vec::new());
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("id:") {
                    id = v.trim().parse::<i64>().ok();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push(v.strip_prefix(' ').unwrap_or(v).to_owned());
                }
            }
            if !data.is_empty() {
                return Some(Frame {
                    id,
                    event: serde_json::from_str(&data.join("\n"))
                        .unwrap_or_else(|e| panic!("a data line that is not JSON ({e}): {block}")),
                });
            }
        }
    }

    /// Every frame up to the end of the stream; panics when the stream does not end in time.
    pub async fn collect_frames(&mut self, within: Duration) -> Vec<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_frame(left).await {
                Some(frame) => out.push(frame),
                None => {
                    assert!(
                        self.ended,
                        "the stream did not end within {within:?}; frames so far: {out:?}"
                    );
                    return out;
                }
            }
        }
    }

    /// Collects AG-UI frames until one satisfies `stop` (inclusive); panics on timeout or end.
    /// The stream stays usable: this is for a stream that goes on (a connect stream).
    pub async fn frames_until(
        &mut self,
        within: Duration,
        stop: impl Fn(&Frame) -> bool,
    ) -> Vec<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = self.next_frame(left).await.unwrap_or_else(|| {
                panic!(
                    "the stream ended or timed out before the awaited frame; got so far: {out:?}"
                )
            });
            let done = stop(&frame);
            out.push(frame);
            if done {
                return out;
            }
        }
    }
}
