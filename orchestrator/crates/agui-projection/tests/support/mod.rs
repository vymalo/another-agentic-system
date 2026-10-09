//! Shared test helpers: a model of the reference consumer's rules, legal log generation.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

pub mod goldens;
pub mod log;
pub mod verify;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::Event;

/// Projects every event for a viewer, keeping the frames of each event apart.
pub fn project_each(events: &[Event]) -> Vec<Vec<Frame>> {
    project_each_with(events, log::meta())
}

/// [`project_each`] for the thread `meta`.
pub fn project_each_with(
    events: &[Event],
    meta: orch_agui_projection::ThreadMeta,
) -> Vec<Vec<Frame>> {
    let mut projector = Projector::new(meta);
    events
        .iter()
        .map(|e| projector.apply(e, Audience::Viewer))
        .collect()
}

pub fn flatten(frames: &[Vec<Frame>]) -> Vec<Frame> {
    frames.iter().flatten().cloned().collect()
}

/// A one-line rendering of a frame for readable assertions: the type, the ids that name it, the
/// attribution, and (for the frames that carry one) the payload. An activity's `at` (the event's
/// time, on every `vymalo.*` activity) is left out: `projection.rs` checks it once.
pub fn line(frame: &Frame) -> String {
    use orch_agui_proto::Event as E;
    let id = frame
        .resume_id
        .map(|n| format!("  id:{n}"))
        .unwrap_or_default();
    let subagent = |s: Option<&orch_agui_proto::SubagentRunId>| {
        s.map(|s| format!(" @{s}")).unwrap_or_default()
    };
    let body = match &frame.event {
        E::RunStarted(e) => format!("RUN_STARTED {}", e.run_id),
        E::RunFinished(e) => {
            use orch_agui_proto::RunFinishedOutcome as O;
            let outcome = match &e.outcome {
                None | Some(O::Success { .. }) => "success".to_owned(),
                Some(O::Cancelled) => "cancelled".to_owned(),
                Some(O::Interrupt { interrupts }) => format!(
                    "interrupt[{}]",
                    interrupts
                        .iter()
                        .map(|i| format!(
                            "{}:{}{}",
                            i.id,
                            i.reason,
                            subagent(i.subagent_run_id.as_ref())
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            };
            format!(
                "RUN_FINISHED {} {outcome}{}",
                e.run_id,
                usage(e.usage.as_ref())
            )
        }
        E::RunError(e) => format!(
            "RUN_ERROR {} {:?}{}",
            e.code.as_deref().unwrap_or("-"),
            e.message,
            usage(e.usage.as_ref())
        ),
        E::Custom(e) => format!(
            "CUSTOM {} {}{}",
            e.name,
            e.value
                .get("call")
                .and_then(|c| c.as_str())
                .unwrap_or("total"),
            subagent(e.subagent_run_id.as_ref())
        ),
        E::StateSnapshot(e) => format!(
            "STATE_SNAPSHOT {}",
            e.snapshot["thread"]["state"].as_str().unwrap_or("?")
        ),
        E::TextMessageStart(e) => format!(
            "TEXT_MESSAGE_START {} {}{}",
            e.message_id,
            serde_json::to_string(&e.role).unwrap().trim_matches('"'),
            subagent(e.subagent_run_id.as_ref())
        ),
        E::TextMessageContent(e) => format!("TEXT_MESSAGE_CONTENT {} {:?}", e.message_id, e.delta),
        E::TextMessageEnd(e) => format!("TEXT_MESSAGE_END {}", e.message_id),
        E::ActivitySnapshot(e) => format!(
            "ACTIVITY_SNAPSHOT {} {} {}{}",
            e.message_id,
            e.activity_type,
            serde_json::to_string(&{
                let mut content = e.content.clone();
                content.remove("at");
                content
            })
            .unwrap(),
            subagent(e.subagent_run_id.as_ref())
        ),
        E::SubagentStarted(e) => format!(
            "SUBAGENT_STARTED {} {}{}",
            e.subagent_run_id,
            e.name,
            e.parent_subagent_run_id
                .as_ref()
                .map(|p| format!(" in {p}"))
                .unwrap_or_default()
        ),
        E::SubagentFinished(e) => {
            use orch_agui_proto::SubagentFinishedOutcome as O;
            let outcome = match &e.outcome {
                None | Some(O::Success) => "success".to_owned(),
                Some(O::Suspended { interrupt_ids }) => format!(
                    "suspended[{}]",
                    interrupt_ids
                        .iter()
                        .flatten()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            };
            format!(
                "SUBAGENT_FINISHED {} {outcome}{}",
                e.subagent_run_id,
                e.result
                    .as_ref()
                    .map(|r| format!(" result={r}"))
                    .unwrap_or_default()
            )
        }
        E::SubagentError(e) => format!(
            "SUBAGENT_ERROR {} {} {:?}",
            e.subagent_run_id,
            e.code.as_deref().unwrap_or("-"),
            e.message
        ),
        other => other.event_type().as_str().to_owned(),
    };
    format!("{body}{id}")
}

/// ` usage=[model:input/output,…]` of a run's terminal event, or nothing when it has none.
fn usage(usage: Option<&Vec<orch_agui_proto::TokenUsage>>) -> String {
    usage.map_or_else(String::new, |entries| {
        let said: Vec<String> = entries
            .iter()
            .map(|u| {
                format!(
                    "{}:{}/{}",
                    u.model.as_deref().unwrap_or("?"),
                    u.input_tokens.unwrap_or(0),
                    u.output_tokens.unwrap_or(0)
                )
            })
            .collect();
        format!(" usage=[{}]", said.join(","))
    })
}

pub fn lines(frames: &[Frame]) -> Vec<String> {
    frames.iter().map(line).collect()
}
