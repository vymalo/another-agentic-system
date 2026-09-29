//! `GET /metrics`: the outbox queue as Prometheus text, for dashboards and for autoscaling a
//! worker pool on the backlog (KEDA, see `docs/orchestrator.md`, "Observability and scaling").
//!
//! The text exposition format is written by hand: four samples do not justify a metrics
//! crate. The numbers are read from the store on every scrape, so they are the same whichever
//! replica answers, and a scrape costs one aggregate query over the open rows.

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use orch_ports::{OutboxStats, Ports};

use crate::ApiState;

/// The Prometheus text exposition format, version 0.0.4.
const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The exposition text for `stats` as of `now`. Pure: the age is whole seconds of `now` minus
/// the oldest due row's due time (never negative, `0` when nothing is due).
pub(crate) fn render(stats: &OutboxStats, now: Timestamp) -> String {
    let age = stats
        .oldest_due_at
        .map_or(0, |due| now.duration_since(due).as_secs().max(0));
    format!(
        "# HELP orch_outbox_rows Open outbox rows by state.\n\
         # TYPE orch_outbox_rows gauge\n\
         orch_outbox_rows{{state=\"due\"}} {due}\n\
         orch_outbox_rows{{state=\"waiting\"}} {waiting}\n\
         orch_outbox_rows{{state=\"leased\"}} {leased}\n\
         # HELP orch_outbox_oldest_due_age_seconds Age of the oldest due outbox row, 0 when none.\n\
         # TYPE orch_outbox_oldest_due_age_seconds gauge\n\
         orch_outbox_oldest_due_age_seconds {age}\n",
        due = stats.due,
        waiting = stats.waiting,
        leased = stats.leased,
    )
}

pub(crate) async fn serve<P: Ports>(State(state): State<ApiState<P>>) -> Response {
    match state.app.outbox_stats().await {
        Ok((now, stats)) => {
            let mut response = render(&stats, now).into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
            response
        }
        Err(e) => {
            tracing::warn!(error = %orch_core::report(&e), "cannot read the outbox for /metrics");
            let mut response =
                (StatusCode::SERVICE_UNAVAILABLE, "store unavailable").into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use jiff::SignedDuration;

    use super::*;

    fn now() -> Timestamp {
        "2026-01-01T00:10:00Z".parse().unwrap()
    }

    fn ago(secs: i64) -> Timestamp {
        now().checked_sub(SignedDuration::from_secs(secs)).unwrap()
    }

    const GOLDEN: &str = "\
# HELP orch_outbox_rows Open outbox rows by state.
# TYPE orch_outbox_rows gauge
orch_outbox_rows{state=\"due\"} 3
orch_outbox_rows{state=\"waiting\"} 2
orch_outbox_rows{state=\"leased\"} 1
# HELP orch_outbox_oldest_due_age_seconds Age of the oldest due outbox row, 0 when none.
# TYPE orch_outbox_oldest_due_age_seconds gauge
orch_outbox_oldest_due_age_seconds 42
";

    #[test]
    fn renders_the_golden_text() {
        let stats = OutboxStats {
            due: 3,
            waiting: 2,
            leased: 1,
            oldest_due_at: Some(ago(42)),
        };
        assert_eq!(render(&stats, now()), GOLDEN);
    }

    #[test]
    fn an_empty_outbox_is_all_zeros() {
        let text = render(&OutboxStats::default(), now());
        assert_eq!(
            text,
            GOLDEN
                .replace("} 3", "} 0")
                .replace("} 2", "} 0")
                .replace("} 1", "} 0")
                .replace("seconds 42", "seconds 0")
        );
    }

    #[test]
    fn the_age_is_whole_seconds_and_never_negative() {
        let age_of = |oldest_due_at| {
            let stats = OutboxStats {
                due: 1,
                oldest_due_at: Some(oldest_due_at),
                ..OutboxStats::default()
            };
            render(&stats, now())
                .lines()
                .last()
                .unwrap()
                .rsplit(' ')
                .next()
                .unwrap()
                .to_owned()
        };
        assert_eq!(age_of(ago(0)), "0");
        assert_eq!(
            age_of(
                ago(90)
                    .checked_sub(SignedDuration::from_millis(900))
                    .unwrap()
            ),
            "90",
            "fractions of a second are dropped"
        );
        // The clock read behind the row's due time (skew between replicas): clamp to zero.
        assert_eq!(age_of(ago(-5)), "0");
    }
}
