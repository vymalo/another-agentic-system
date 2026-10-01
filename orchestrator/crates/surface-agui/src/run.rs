//! `POST /agui/agents/{agentId}`.

use axum::Extension;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::sse::Sse;
use axum::response::{IntoResponse, Response};
use orch_agui_projection::{Audience, Projector};
use orch_agui_projection::{
    InputError, ThreadMeta, ThreadView, Translation, held_message_ids, release_selector,
    thread_id_of, translate_with_warnings,
};
use orch_agui_proto::RunAgentInput;
use orch_api::sse::{keep_alive, stream_headers};
use orch_api::{ApiError, Problem};
use orch_app::{
    App, AppError, ApplyOutcome, Creation, GateLayer, Inbound, NewThread, THREAD_GATE_KEY,
    THREAD_UI_CATALOG_KEY, check_catalog_schemas,
};
use orch_core::{
    AgentId, AgentTarget, Event, Input, Origin, ThreadId, ThreadRecord, UiCatalogData, UserId,
    report,
};
use orch_ports::Ports;

use crate::refuse::{check_accept, check_json, input_error};
use crate::stream::{Feed, Start, frames};
use crate::{MAX_BODY_BYTES, State as SurfaceState};

/// Longest `runId` or message id we record in the log.
const MAX_ID_BYTES: usize = 256;
/// Events per page when the log is read.
const PAGE: u32 = 500;
/// A request that keeps racing another for the same thread gives up after this many looks.
const MAX_ATTEMPTS: usize = 3;

pub(crate) async fn run<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Extension(user): Extension<UserId>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    check_accept(&parts.headers)?;
    check_json(&parts.headers)?;
    let bytes = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| {
        Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("the request body is larger than {MAX_BODY_BYTES} bytes, or could not be read"),
        )
    })?;
    let parsed = RunAgentInput::parse(&bytes)
        .map_err(|e| Problem::bad_request(format!("invalid run input: {}", report(&e))))?;
    if !parsed.dropped.is_empty() {
        tracing::warn!(dropped = ?parsed.dropped, "members of the run input that AG-UI does not define were dropped");
    }
    let input = parsed.input;
    check_ids(&input)?;
    let gate = gate_request(&input)?;
    let catalog = catalog_request(&input)?;
    let thread = thread_id_of(&input).map_err(|e| input_error(&e))?;
    let agent = AgentId::new(agent_id);
    if state.app.directory().get(&agent).is_none() {
        return Err(Problem::not_found("no such agent").into());
    }
    for _ in 0..MAX_ATTEMPTS {
        if let Some(feed) = attempt(
            &state.app,
            &user,
            &agent,
            thread,
            &input,
            gate.as_ref(),
            catalog.as_ref(),
        )
        .await?
        {
            let stream = frames(feed);
            let sse = Sse::new(stream).keep_alive(keep_alive(state.keepalive));
            return Ok((stream_headers(), sse).into_response());
        }
    }
    // Another request for this thread kept winning the race.
    Err(AppError::Contended.into())
}

/// The gate the run asks for, `forwardedProps["vymalo.gate"]` (ADR 0018): `{require?,
/// maxAttempts?}`. It is read whether or not the run creates the thread, so a malformed one is
/// a 400 every time. It applies when the run creates the thread, whose gate is fixed then; a run
/// that continues a thread and asks for a different gate is a 409 (see `attempt`). Whether the
/// request is allowed (it may add sources and change the attempts within the cap, never remove
/// a source) is decided by [`App::create_thread_as`].
fn gate_request(input: &RunAgentInput) -> Result<Option<GateLayer>, Problem> {
    let Some(value) = input
        .forwarded_props
        .as_ref()
        .and_then(|props| props.get(THREAD_GATE_KEY))
    else {
        return Ok(None);
    };
    GateLayer::from_json(value).map_err(|e| Problem::bad_request(e.to_string()))
}

/// The UI catalog the run carries, `forwardedProps["vymalo.uiCatalog"]` (ADR 0023): `{catalogId,
/// version, digest, catalog}`, the screen's component catalog. Like the gate it is read on every
/// run, so a malformed one is refused every time, before anything is written: the envelope rules
/// of [`UiCatalogData::from_json`] (**400**, **413** for a catalog over 64 KiB) and the schema
/// check of [`check_catalog_schemas`] (**400**). It is applied only when the run applies an input:
/// a run that attaches to one already in the log ignores it, which is what makes a retry of the
/// same request harmless.
fn catalog_request(input: &RunAgentInput) -> Result<Option<UiCatalogData>, ApiError> {
    let Some(value) = input
        .forwarded_props
        .as_ref()
        .and_then(|props| props.get(THREAD_UI_CATALOG_KEY))
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let catalog = UiCatalogData::from_json(value).map_err(|e| {
        let status = if e.is_too_large() {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        };
        Problem::new(status, format!("invalid {THREAD_UI_CATALOG_KEY}: {e}"))
    })?;
    check_catalog_schemas(&catalog)
        .map_err(|e| Problem::bad_request(format!("invalid {THREAD_UI_CATALOG_KEY}: {e}")))?;
    Ok(Some(catalog))
}

/// Gives the first input that is a message or an action the catalog the run carried, once.
fn carrying(input: Input, catalog: &mut Option<UiCatalogData>) -> Input {
    match input {
        Input::UserMessage {
            user,
            text,
            message_id,
            run_id,
            origin,
            catalog: _,
        } => Input::UserMessage {
            user,
            text,
            message_id,
            run_id,
            origin,
            catalog: catalog.take(),
        },
        Input::UiAction {
            user,
            action,
            catalog: _,
        } => Input::UiAction {
            user,
            action,
            catalog: catalog.take(),
        },
        other @ (Input::Redeliver { .. }
        | Input::Cancel { .. }
        | Input::Agent { .. }
        | Input::DeliveryFailed { .. }
        | Input::CancelledBeforeStart
        | Input::CancelRejected { .. }
        | Input::CiReported(_)
        | Input::VerifierReported { .. }
        | Input::VerifierFailed { .. }
        | Input::TimerFired(_)) => other,
    }
}

/// The ids we write into the log are bounded.
fn check_ids(input: &RunAgentInput) -> Result<(), Problem> {
    let too_long = std::iter::once(input.run_id.as_str())
        .chain(input.messages.iter().map(|m| m.id().as_str()))
        .any(|id| id.len() > MAX_ID_BYTES);
    if too_long {
        return Err(Problem::bad_request(format!(
            "ids must be at most {MAX_ID_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn meta_of(thread: &ThreadRecord) -> ThreadMeta {
    ThreadMeta {
        thread_id: thread.id,
        title: thread.title.clone(),
        target: thread.target.clone(),
        gate: thread.job.gate.clone(),
    }
}

/// Every event of the thread, oldest first.
async fn load_events<P: Ports>(
    app: &App<P>,
    user: &UserId,
    id: ThreadId,
) -> Result<Vec<Event>, AppError> {
    let mut all: Vec<Event> = Vec::new();
    loop {
        let after = all.last().map_or(0, |e| e.seq);
        let page = app.list_events(user, id, after, PAGE).await?;
        let full = page.len() >= PAGE as usize;
        all.extend(page);
        if !full {
            return Ok(all);
        }
    }
}

/// The idempotency key of an input: a retry of the same message (or of the same answer or
/// action, which have no message id) is the same input.
fn key_of(thread: ThreadId, input: &Input) -> Option<String> {
    match input {
        Input::UserMessage {
            message_id: Some(id),
            ..
        } => Some(format!("agui:{thread}:msg:{id}")),
        Input::UserMessage {
            run_id: Some(id), ..
        } => Some(format!("agui:{thread}:run:{id}")),
        // An action has no message id: its run id is the key, like an answer's.
        Input::UiAction { action, .. } => action
            .run_id
            .as_ref()
            .map(|id| format!("agui:{thread}:run:{id}")),
        Input::UserMessage { .. }
        | Input::Redeliver { .. }
        | Input::Cancel { .. }
        | Input::Agent { .. }
        | Input::DeliveryFailed { .. }
        | Input::CancelledBeforeStart
        | Input::CancelRejected { .. }
        | Input::CiReported(_)
        | Input::VerifierReported { .. }
        | Input::VerifierFailed { .. }
        | Input::TimerFired(_) => None,
    }
}

/// One look at the thread and one try at doing what the request asks. `None` means "look
/// again": a concurrent request with the same ids got there first, and this one is now a retry
/// of it (an attach).
async fn attempt<P: Ports>(
    app: &std::sync::Arc<App<P>>,
    user: &UserId,
    agent: &AgentId,
    thread: ThreadId,
    input: &RunAgentInput,
    gate: Option<&GateLayer>,
    catalog: Option<&UiCatalogData>,
) -> Result<Option<Feed>, ApiError> {
    // The thread as the log holds it now, if there is one.
    let known = match app.find_thread(user, thread).await? {
        Some(record) => {
            let events = load_events(app, user, thread).await?;
            let mut projector = Projector::new(meta_of(&record));
            for event in &events {
                let _ = projector.apply(event, Audience::Viewer);
            }
            Some((record, events, projector))
        }
        None => None,
    };
    // The ids the MCP surface derives from a user and a request id are not for a consumer to
    // choose (ADR 0019): a thread could be created under an id another user's `start_job` will
    // need. A job that exists is still continued here.
    if known.is_none() && thread.is_derived() {
        return Err(Problem::bad_request(format!(
            "thread ids of UUID version {} are reserved for jobs started over MCP; mint a \
             UUIDv7 (or any other version) for a new thread",
            ThreadId::DERIVED_VERSION
        ))
        .into());
    }
    let view = match &known {
        Some((_, _, projector)) => projector.view(user),
        None => ThreadView::new_thread(user.clone()),
    };
    view.ensure_agent(agent).map_err(|e| input_error(&e))?;
    // A thread's gate is fixed when it is created (ADR 0016). A run that continues the thread
    // (a follow-up, an answer, the loser of a race to create it) and asks for a different one
    // is refused, not silently served under the gate the thread has.
    if let (Some((record, _, _)), Some(request)) = (&known, gate)
        && app.gate_request_changes(&record.job.gate, request)?
    {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "this run asks for a verification gate different from the one the thread has; a \
             thread's gate is fixed when it is created: send the run without `vymalo.gate`, or \
             with the gate the thread has (see `job` in its state), or start a new thread",
        )
        .into());
    }
    let Translation { inputs, warnings } =
        translate_with_warnings(input, &view).map_err(|e| input_error(&e))?;
    for warning in &warnings {
        tracing::warn!(%thread, %warning, "part of the run input was ignored");
    }
    let held = held_message_ids(input);

    let last_seq = known
        .as_ref()
        .and_then(|(_, events, _)| events.last())
        .map_or(0, |e| e.seq);

    // Nothing new, and the run exists: an attach. Fold the whole log again, this time to write
    // that run's frames.
    if inputs.is_empty() {
        let Some((record, events, _)) = known else {
            // `translate` refuses this with 422; a new thread has no run to attach to.
            return Err(input_error(&InputError::NothingToRun {
                run_id: input.run_id.to_string(),
            }));
        };
        tracing::debug!(%thread, run = %input.run_id, "attaching to a run");
        return Ok(Some(Feed {
            projector: Projector::new(meta_of(&record)),
            backlog: events.into(),
            live: app.event_stream(user, thread, last_seq).await?,
            start: Start::Run(input.run_id.to_string()),
            held,
        }));
    }

    // Something new: apply it, then follow the log from where it landed.
    match known {
        None => {
            let [
                Input::UserMessage {
                    text,
                    message_id,
                    run_id,
                    ..
                },
            ] = inputs.as_slice()
            else {
                return Err(
                    AppError::internal("a new thread was given something else to do").into(),
                );
            };
            let target = AgentTarget {
                agent_id: agent.clone(),
                release: release_selector(input).map(str::to_owned),
            };
            let new = NewThread {
                title: None,
                target,
                text: text.clone(),
            };
            let inbound = Inbound {
                message_id: message_id.clone(),
                run_id: run_id.clone(),
                key: key_of(thread, &inputs[0]),
                gate: gate.cloned(),
                origin: Origin::Agui,
                ui_catalog: catalog.cloned(),
            };
            match app.create_thread_as(user, thread, new, inbound).await? {
                Creation::Created {
                    thread: record,
                    events,
                } => {
                    tracing::debug!(%thread, "thread created by a run");
                    Ok(Some(Feed {
                        projector: Projector::new(meta_of(&record)),
                        backlog: std::collections::VecDeque::new(),
                        live: app.event_stream(user, thread, 0).await?,
                        start: Start::Seq(events.first().map_or(1, |e| e.seq)),
                        held,
                    }))
                }
                Creation::Exists => Ok(None),
            }
        }
        Some((_, _, projector)) => {
            let mut start = None;
            // the catalog goes with the first message or action, which is the input it came with
            let mut carried = catalog.cloned();
            for next in inputs {
                let key = key_of(thread, &next);
                let next = carrying(next, &mut carried);
                match app.submit(user, thread, next, key).await? {
                    ApplyOutcome::Applied { events, .. } => {
                        start = start.or_else(|| events.first().map(|e| e.seq));
                    }
                    ApplyOutcome::Duplicate => return Ok(None),
                    ApplyOutcome::Fenced => {
                        return Err(
                            AppError::internal("a commit without a lease was fenced").into()
                        );
                    }
                }
            }
            // An input that writes no event (a cancel) is answered by whatever the log says next.
            let start = start.unwrap_or(last_seq + 1);
            Ok(Some(Feed {
                projector,
                backlog: std::collections::VecDeque::new(),
                live: app.event_stream(user, thread, last_seq).await?,
                start: Start::Seq(start),
                held,
            }))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::{UiActionData, UiVersion};
    use serde_json::json;

    use super::*;

    fn catalog() -> UiCatalogData {
        let value = json!({"catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
            "components": {"Note": {"type": "object"}}});
        UiCatalogData {
            catalog_id: "https://agents.vymalo.com/a2ui/catalogs/chat".to_owned(),
            version: 1,
            digest: orch_core::catalog_digest(&value).unwrap(),
            catalog: value,
        }
    }

    fn message() -> Input {
        Input::UserMessage {
            user: UserId::new("a@b.c"),
            text: "hi".to_owned(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
        }
    }

    fn action() -> Input {
        Input::UiAction {
            user: UserId::new("a@b.c"),
            action: UiActionData {
                surface_id: "s".to_owned(),
                name: "go".to_owned(),
                source_component_id: "b".to_owned(),
                context: serde_json::Map::new(),
                version: UiVersion::V0_9_1,
                run_id: None,
            },
            catalog: None,
        }
    }

    #[test]
    fn the_catalog_goes_with_the_first_message_or_action_and_only_once() {
        let mut carried = Some(catalog());
        let Input::UserMessage { catalog: first, .. } = carrying(message(), &mut carried) else {
            panic!("a message stays a message");
        };
        assert_eq!(first, Some(catalog()));
        assert_eq!(carried, None);
        let Input::UserMessage {
            catalog: second, ..
        } = carrying(message(), &mut carried)
        else {
            panic!("a message stays a message");
        };
        assert_eq!(second, None);

        let mut carried = Some(catalog());
        let Input::UiAction { catalog: taken, .. } = carrying(action(), &mut carried) else {
            panic!("an action stays an action");
        };
        assert_eq!(taken, Some(catalog()));
        assert_eq!(carried, None);
    }

    #[test]
    fn a_stop_does_not_take_the_catalog() {
        let mut carried = Some(catalog());
        let stop = Input::Cancel {
            user: UserId::new("a@b.c"),
        };
        assert_eq!(carrying(stop.clone(), &mut carried), stop);
        assert_eq!(carried, Some(catalog()), "left for an input that records");
    }
}
