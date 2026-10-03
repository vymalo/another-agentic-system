//! What every server-sent-events response of the orchestrator shares: the keepalive comment
//! and the headers that keep a proxy from buffering or transforming the stream.

use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, header};
use axum::response::sse::KeepAlive;
use futures::{Stream, StreamExt as _};
use orch_core::Timestamp;
use orch_ports::Principal;

/// The longest a stream stays open on one credential: an hour (ADR 0033). A client reconnects with
/// `Last-Event-ID` (the connect stream is resumable) and the proxy's session gives it a fresh token.
pub const MAX_STREAM: Duration = Duration::from_secs(3600);

/// The leeway of a token's `exp` (60 seconds, as the authenticator allows it): a stream may outlive
/// the expiry by that much and no more.
pub const STREAM_LEEWAY: Duration = Duration::from_secs(60);

/// How long a stream may stay open for `principal`, at `now` (ADR 0033): until the credential's
/// expiry plus the leeway, and at most [`MAX_STREAM`]. `None` for a credential that does not run
/// out (the proxy header, a static token): such a stream is not bounded here, only by the
/// process going away.
pub fn stream_budget(principal: &Principal, now: Timestamp) -> Option<Duration> {
    let expires_at = principal.expires_at?;
    let left = expires_at
        .as_second()
        .saturating_add(i64::try_from(STREAM_LEEWAY.as_secs()).unwrap_or(0))
        .saturating_sub(now.as_second());
    Some(Duration::from_secs(u64::try_from(left.max(0)).unwrap_or(0)).min(MAX_STREAM))
}

/// `stream`, ended when `budget` has passed (never, for `None`). The events already sent stay
/// sent: the client resumes from the last `id:` it saw.
pub fn bounded<S: Stream>(stream: S, budget: Option<Duration>) -> impl Stream<Item = S::Item> {
    // A timer that is due at once may still be pending when the stream is first polled: a budget
    // of nothing ends the stream before its first item, not after a few.
    let spent = budget.is_some_and(|budget| budget.is_zero());
    stream
        .take_while(move |_| std::future::ready(!spent))
        .take_until(async move {
            match budget {
                Some(budget) => tokio::time::sleep(budget).await,
                None => std::future::pending::<()>().await,
            }
        })
}

/// The `: keepalive` comment at `interval`. The interval is clamped to at least 10 ms so a
/// misconfiguration cannot turn the keepalive into a busy loop.
pub fn keep_alive(interval: Duration) -> KeepAlive {
    KeepAlive::new()
        .interval(Duration::max(interval, Duration::from_millis(10)))
        .text("keepalive")
}

/// `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`, for the response head
/// of a stream.
pub fn stream_headers() -> [(HeaderName, HeaderValue); 2] {
    [
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-transform"),
        ),
        (
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        ),
    ]
}

/// The head of the response of a stream of a **shared** thread (ADR 0040): `Cache-Control:
/// no-store, no-transform`, `X-Accel-Buffering: no` and `X-Robots-Tag: noindex, nofollow`.
pub fn shared_stream_headers() -> [(HeaderName, HeaderValue); 3] {
    [
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-transform"),
        ),
        (
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        ),
        (
            HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex, nofollow"),
        ),
    ]
}

/// `stream` with `guard` held until the stream ends or is dropped: a response body that carries a
/// permit (a [`StreamPermit`](crate::StreamPermit)) is the stream the permit stands for.
pub fn hold<S, T>(stream: S, guard: T) -> impl Stream<Item = S::Item>
where
    S: Stream + Send + 'static,
    T: Send + 'static,
{
    futures::stream::unfold(
        (Box::pin(stream), guard),
        |(mut stream, guard)| async move {
            let item = stream.next().await?;
            Some((item, (stream, guard)))
        },
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::UserId;

    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    fn expiring(seconds: Option<i64>) -> Principal {
        Principal {
            expires_at: seconds.map(at),
            ..Principal::of(UserId::new("a@b.c"))
        }
    }

    #[test]
    fn a_credential_that_does_not_run_out_does_not_bound_a_stream() {
        assert_eq!(stream_budget(&expiring(None), at(1_000)), None);
    }

    #[test]
    fn a_stream_lasts_until_the_expiry_and_the_leeway_and_at_most_an_hour() {
        let now = 1_000_000;
        let budget = |exp: i64| {
            stream_budget(&expiring(Some(exp)), at(now))
                .unwrap()
                .as_secs()
        };
        // Five minutes left: five minutes and the leeway.
        assert_eq!(budget(now + 300), 360);
        // Expired within the leeway: what is left of it.
        assert_eq!(budget(now - 10), 50);
        // Expired beyond it (a clock apart from the authenticator's): none.
        assert_eq!(budget(now - 61), 0);
        assert_eq!(budget(now - 100_000), 0);
        // A token for a day is still an hour.
        assert_eq!(budget(now + 86_400), 3600);
        assert_eq!(budget(now + 3600 - STREAM_LEEWAY.as_secs() as i64), 3600);
        assert_eq!(budget(253_402_207_200 - 100), 3600);
    }

    #[tokio::test(start_paused = true)]
    async fn a_bounded_stream_ends_at_the_budget_and_an_unbounded_one_does_not() {
        let ticks = || {
            futures::stream::unfold(0_u32, |n| async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Some((n, n + 1))
            })
        };
        let seen: Vec<u32> = bounded(ticks(), Some(Duration::from_millis(3500)))
            .collect()
            .await;
        assert_eq!(seen, [0, 1, 2]);
        let none: Vec<u32> = bounded(ticks(), Some(Duration::ZERO)).collect().await;
        assert!(none.is_empty());
        let mut open = Box::pin(bounded(ticks(), None));
        for n in 0..10 {
            assert_eq!(open.next().await, Some(n));
        }
    }
}
