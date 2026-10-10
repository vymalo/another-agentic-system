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
use orch_app::{DeleteStats, HistoryStats, SharingStats, UsageStats};
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

/// The counters of sharing (ADR 0040, section 11), for this process: how many times a share
/// changed, by action, and how many times a shared thread was opened, by the visibility it was
/// served at. They count what happened and say nothing of who: a log of who read a conversation
/// would itself be personal data of the readers.
pub(crate) fn render_sharing(stats: &SharingStats) -> String {
    let mut out = String::from(
        "# HELP share_changes_total Changes of a thread's share by its owner, by action.\n\
         # TYPE share_changes_total counter\n",
    );
    for (action, n) in stats.changes {
        out.push_str(&format!(
            "share_changes_total{{action=\"{}\"}} {n}\n",
            action.as_str()
        ));
    }
    out.push_str(&format!(
        "# HELP shared_reads_total Times a shared thread was opened through its link, by the visibility it was served at.\n\
         # TYPE shared_reads_total counter\n\
         shared_reads_total{{visibility=\"internal\"}} {}\n\
         shared_reads_total{{visibility=\"public\"}} {}\n",
        stats.reads_internal, stats.reads_public
    ));
    out
}

/// What deleting threads counts and gauges (ADR 0043): `threads_deleted_total` (threads erased by
/// this process, edits included), `late_input_dropped_total{source}` (results and rows the
/// dispatcher or the inbox worker held for a thread that was deleted meanwhile, dropped and never
/// retried) and `thread_purges_pending` (threads deleted whose files are not yet erased, over every
/// replica's rows). None of them names a thread or a person. The two counters are this process's
/// own; only the gauge is read from the store, so `pending` is `None` when the store cannot say
/// and the gauge alone is left out.
pub(crate) fn render_deleting(stats: &DeleteStats, pending: Option<u64>) -> String {
    let mut out = format!(
        "# HELP threads_deleted_total Threads deleted by this process, the edits of a deleted thread included.\n\
         # TYPE threads_deleted_total counter\n\
         threads_deleted_total {}\n\
         # HELP late_input_dropped_total Results and rows held for a thread that was deleted meanwhile, dropped and never retried, by where.\n\
         # TYPE late_input_dropped_total counter\n",
        stats.threads_deleted
    );
    for (source, n) in stats.late_input_dropped {
        out.push_str(&format!(
            "late_input_dropped_total{{source=\"{}\"}} {n}\n",
            source.as_str()
        ));
    }
    if let Some(pending) = pending {
        out.push_str(&format!(
            "# HELP thread_purges_pending Deleted threads whose files are still to be erased.\n\
             # TYPE thread_purges_pending gauge\n\
             thread_purges_pending {pending}\n"
        ));
    }
    out
}

/// What the token usage reports count (ADR 0056): `usage_reports_dropped_total{reason}`, the reports
/// this process did not log, `invalid` (they broke the `usage/v1` contract) or `job_limit` (a valid
/// call report past the job's bound). It names no thread and no agent.
pub(crate) fn render_usage(stats: &UsageStats) -> String {
    let mut out = String::from(
        "# HELP usage_reports_dropped_total Token usage reports (usage/v1) that were not logged, by why.\n\
         # TYPE usage_reports_dropped_total counter\n",
    );
    for (reason, n) in stats.reports_dropped {
        out.push_str(&format!(
            "usage_reports_dropped_total{{reason=\"{}\"}} {n}\n",
            reason.as_str()
        ));
    }
    out
}

/// What the history reads of this process cost (ADR 0059): pages answered, log events folded for
/// them and the time that took. `rate(history_fold_seconds_total) / rate(history_pages_total)` is
/// the mean cost of a page. They name no thread and no person.
pub(crate) fn render_history(stats: &HistoryStats) -> String {
    format!(
        "# HELP history_pages_total Pages of thread history answered.\n\
         # TYPE history_pages_total counter\n\
         history_pages_total {}\n\
         # HELP history_events_folded_total Log events folded for those pages.\n\
         # TYPE history_events_folded_total counter\n\
         history_events_folded_total {}\n\
         # HELP history_fold_seconds_total Seconds spent reading the log and folding it for those pages.\n\
         # TYPE history_fold_seconds_total counter\n\
         history_fold_seconds_total {}.{:06}\n",
        stats.pages,
        stats.events_folded,
        stats.fold_micros / 1_000_000,
        stats.fold_micros % 1_000_000,
    )
}

pub(crate) async fn serve<P: Ports>(State(state): State<ApiState<P>>) -> Response {
    match state.app.outbox_stats().await {
        Ok((now, stats)) => {
            let mut text = render(&stats, now);
            text.push_str(&render_sharing(&state.app.sharing_stats()));
            // The purges are the one count that is the store's: a store that cannot say leaves
            // the gauge out, while the delete counters (this process's own) are still written,
            // and the scrape is still the outbox's (an alert on the gauge's absence is the
            // operator's).
            let pending = match state.app.purges_pending().await {
                Ok(pending) => Some(pending),
                Err(e) => {
                    tracing::warn!(error = %orch_core::report(&e), "cannot read the purges for /metrics");
                    None
                }
            };
            text.push_str(&render_deleting(&state.app.delete_stats(), pending));
            text.push_str(&render_usage(&state.app.usage_stats()));
            text.push_str(&render_history(&state.app.history_stats()));
            let mut response = text.into_response();
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

    #[test]
    fn the_history_counters_are_written_with_whole_microseconds() {
        let text = render_history(&HistoryStats {
            pages: 3,
            events_folded: 12_000,
            fold_micros: 2_500_007,
        });
        assert!(text.contains("history_pages_total 3\n"), "{text}");
        assert!(
            text.contains("history_events_folded_total 12000\n"),
            "{text}"
        );
        assert!(
            text.contains("history_fold_seconds_total 2.500007\n"),
            "{text}"
        );
        assert_eq!(text.matches("# TYPE").count(), 3);
    }

    #[test]
    fn the_deleting_counters_and_the_purge_gauge_are_written() {
        use orch_app::LateSource;
        let stats = DeleteStats {
            threads_deleted: 5,
            late_input_dropped: [(LateSource::Dispatcher, 2), (LateSource::Inbox, 1)],
        };
        assert_eq!(
            render_deleting(&stats, Some(3)),
            "\
# HELP threads_deleted_total Threads deleted by this process, the edits of a deleted thread included.
# TYPE threads_deleted_total counter
threads_deleted_total 5
# HELP late_input_dropped_total Results and rows held for a thread that was deleted meanwhile, dropped and never retried, by where.
# TYPE late_input_dropped_total counter
late_input_dropped_total{source=\"dispatcher\"} 2
late_input_dropped_total{source=\"inbox\"} 1
# HELP thread_purges_pending Deleted threads whose files are still to be erased.
# TYPE thread_purges_pending gauge
thread_purges_pending 3
"
        );
    }

    #[test]
    fn a_store_that_cannot_count_the_purges_leaves_only_the_gauge_out() {
        use orch_app::LateSource;
        let stats = DeleteStats {
            threads_deleted: 5,
            late_input_dropped: [(LateSource::Dispatcher, 2), (LateSource::Inbox, 1)],
        };
        let text = render_deleting(&stats, None);
        assert!(text.contains("threads_deleted_total 5\n"));
        assert!(text.contains("late_input_dropped_total{source=\"dispatcher\"} 2\n"));
        assert!(text.contains("late_input_dropped_total{source=\"inbox\"} 1\n"));
        assert!(!text.contains("thread_purges_pending"));
    }

    #[test]
    fn the_sharing_counters_are_written_by_action_and_visibility() {
        use orch_app::ShareAction;
        let stats = SharingStats {
            changes: ShareAction::ALL.map(|a| {
                let n = match a {
                    ShareAction::Share => 4,
                    ShareAction::Widen => 3,
                    ShareAction::Narrow => 2,
                    ShareAction::Rotate => 1,
                    ShareAction::Revoke => 5,
                };
                (a, n)
            }),
            reads_internal: 7,
            reads_public: 9,
        };
        assert_eq!(
            render_sharing(&stats),
            "\
# HELP share_changes_total Changes of a thread's share by its owner, by action.
# TYPE share_changes_total counter
share_changes_total{action=\"share\"} 4
share_changes_total{action=\"widen\"} 3
share_changes_total{action=\"narrow\"} 2
share_changes_total{action=\"rotate\"} 1
share_changes_total{action=\"revoke\"} 5
# HELP shared_reads_total Times a shared thread was opened through its link, by the visibility it was served at.
# TYPE shared_reads_total counter
shared_reads_total{visibility=\"internal\"} 7
shared_reads_total{visibility=\"public\"} 9
"
        );
    }

    #[test]
    fn the_usage_counter_says_each_reason() {
        let text = render_usage(&UsageStats {
            reports_dropped: [
                (orch_app::UsageDrop::Invalid, 4),
                (orch_app::UsageDrop::JobLimit, 0),
            ],
        });
        assert_eq!(
            text,
            "# HELP usage_reports_dropped_total Token usage reports (usage/v1) that were not logged, by why.\n\
             # TYPE usage_reports_dropped_total counter\n\
             usage_reports_dropped_total{reason=\"invalid\"} 4\n\
             usage_reports_dropped_total{reason=\"job_limit\"} 0\n"
        );
    }
}
