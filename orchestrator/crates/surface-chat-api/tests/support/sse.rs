//! A tiny SSE client for tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Event {
        id: String,
        event: String,
        data: String,
    },
    Comment(String),
}

pub struct SseClient {
    stream: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
    buf: String,
    pub status: reqwest::StatusCode,
    pub headers: reqwest::header::HeaderMap,
}

impl SseClient {
    pub fn from_response(resp: reqwest::Response) -> Self {
        SseClient {
            status: resp.status(),
            headers: resp.headers().clone(),
            stream: resp.bytes_stream().boxed(),
            buf: String::new(),
        }
    }

    fn parse(block: &str) -> Option<Item> {
        let (mut id, mut event, mut data) = (None, None, Vec::new());
        let mut comment = None;
        for line in block.lines() {
            if let Some(c) = line.strip_prefix(':') {
                comment = Some(c.trim().to_owned());
            } else if let Some(v) = line.strip_prefix("id:") {
                id = Some(v.trim().to_owned());
            } else if let Some(v) = line.strip_prefix("event:") {
                event = Some(v.trim().to_owned());
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push(v.strip_prefix(' ').unwrap_or(v).to_owned());
            }
        }
        match (id, event, data.is_empty()) {
            (Some(id), Some(event), false) => Some(Item::Event {
                id,
                event,
                data: data.join("\n"),
            }),
            _ => comment.map(Item::Comment),
        }
    }

    /// The next item, or `None` on timeout or end of stream.
    pub async fn next(&mut self, within: Duration) -> Option<Item> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(pos) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..pos + 2).collect();
                if let Some(item) = Self::parse(&block) {
                    return Some(item);
                }
                continue;
            }
            let chunk = tokio::time::timeout_at(deadline, self.stream.next())
                .await
                .ok()??;
            self.buf.push_str(&String::from_utf8_lossy(&chunk.ok()?));
        }
    }

    /// The next *event* (comments skipped).
    pub async fn next_event(
        &mut self,
        within: Duration,
    ) -> Option<(i64, String, serde_json::Value)> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next(left).await? {
                Item::Event { id, event, data } => {
                    return Some((
                        id.parse().unwrap(),
                        event,
                        serde_json::from_str(&data).unwrap(),
                    ));
                }
                Item::Comment(_) => {}
            }
        }
    }
}
