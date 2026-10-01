//! `GET /agui/agents/{agentId}/capabilities`: the `AgentCapabilities` document of an agent.
//!
//! The shape is the spec's; retrieval is ours (the spec leaves it open). The card is read live
//! on every request and never cached (ADR 0008), so the response says `Cache-Control: no-store`.
//! A card that cannot be read gives a smaller document, not an error: the identity, the
//! transport and the interrupt support are ours to state, and nothing the card would have added
//! (description, version, release channels, A2UI) is assumed.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use orch_agui_projection::{CardFacts, agent_capabilities};
use orch_api::{ApiError, Problem};
use orch_core::AgentId;
use orch_ports::Ports;

use crate::State as SurfaceState;

pub(crate) async fn capabilities<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Path(agent_id): Path<String>,
) -> Result<Response, ApiError> {
    let Some(agent) = state.app.describe_agent(&AgentId::new(agent_id)).await? else {
        return Err(Problem::not_found("no such agent").into());
    };
    let facts = agent.card.map(|card| CardFacts {
        description: card.description,
        version: card.version,
        releases: card.releases,
        ui: card.ui.map(|ui| ui.versions).unwrap_or_default(),
        extensions: card.extensions,
    });
    let document = agent_capabilities(&agent.id, &agent.name, facts.as_ref());
    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(document),
    )
        .into_response())
}
