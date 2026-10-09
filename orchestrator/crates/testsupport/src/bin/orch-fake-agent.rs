//! Test-only executable: two scripted A2A agents plus a control server, for the system
//! end-to-end tests (`web/e2e-system`), where a real orchestrator process delegates to them.
//!
//! It reuses [`FakeAgent`] unchanged (scripts: see `orch_testsupport::fake`). It is never built
//! into the orchestrator image, which builds `--package orchestrator` only.
//!
//! | Environment | Default | |
//! |---|---|---|
//! | `FAKE_CODER_ADDR` | `127.0.0.1:4021` | the `coder` agent, with the sample release channels |
//! | `FAKE_PLAIN_ADDR` | `127.0.0.1:4022` | the `plain` agent, no extension |
//! | `FAKE_CONTROL_ADDR` | `127.0.0.1:4020` | control endpoints, below |
//! | `FAKE_AGENT_EXTENSIONS` | none | comma-separated extensions both agents list in their cards: `ui-catalog` (also lists A2UI v0.9.1 with `acceptsInlineCatalogs: true`, so the catalog arrives inline), `thread-tools`, `steps`, `mentions`, `steer`, `usage` |
//!
//! Control endpoints (`<agent>` is `coder` or `plain`), so a browser test can drive the `gate`
//! script and assert what reached the agent:
//!
//! - `POST /__control/<agent>/release-gate`: lets one waiting `gate` task continue;
//! - `GET /__control/<agent>/calls`: JSON array of what the agent's executor saw, in order (with
//!   `uiCatalog`, the message's `ui-catalog/v1` metadata, `inlineCatalogs`, and `threadTools`, the
//!   grant `{url, token, expiresAt}` of the thread's MCP endpoint when the message carried one).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use orch_core::{A2UI_EXTENSION_V0_9_1, KnownExtension};
use orch_testsupport::{Call, CallKind, FakeAgent, FakeAgentOptions, FakeReleases};
use serde_json::{Value, json};

struct Agents {
    coder: FakeAgent,
    plain: FakeAgent,
}

impl Agents {
    fn get(&self, name: &str) -> Option<&FakeAgent> {
        match name {
            "coder" => Some(&self.coder),
            "plain" => Some(&self.plain),
            _ => None,
        }
    }
}

fn addr(var: &str, default: &str) -> SocketAddr {
    std::env::var(var)
        .unwrap_or_else(|_| default.to_owned())
        .parse()
        .unwrap_or_else(|e| panic!("{var} is not a socket address: {e}"))
}

fn call_json(c: &Call) -> Value {
    json!({
        "kind": match c.kind { CallKind::Execute => "execute", CallKind::Cancel => "cancel", CallKind::Steer => "steer" },
        "taskId": c.task_id,
        "contextId": c.context_id,
        "requestedContext": c.requested_context,
        "messageId": c.message_id,
        "text": c.text,
        "resuming": c.resuming,
        "extensionsHeader": c.extensions_header,
        "messageExtensions": c.message_extensions,
        "activatesReleaseChannels": c.activates_release_channels(),
        "release": c.release,
        "uiCatalog": c.ui_catalog,
        "inlineCatalogs": c.inline_catalogs,
        "threadTools": c.thread_tools,
    })
}

/// `FAKE_AGENT_EXTENSIONS`: the extensions the agents list, by short name (default: none).
fn extensions() -> Vec<KnownExtension> {
    let Ok(list) = std::env::var("FAKE_AGENT_EXTENSIONS") else {
        return Vec::new();
    };
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| match name {
            "ui-catalog" => KnownExtension::UiCatalog,
            "thread-tools" => KnownExtension::ThreadTools,
            "steps" => KnownExtension::Steps,
            "mentions" => KnownExtension::Mentions,
            "text-stream" => KnownExtension::TextStream,
            "steer" => KnownExtension::Steer,
            "usage" => KnownExtension::Usage,
            other => panic!("FAKE_AGENT_EXTENSIONS: unknown extension '{other}'"),
        })
        .collect()
}

/// The card options the extensions ask for: their URIs, and for `ui-catalog` the A2UI entry that
/// takes the catalog inline (the catalog rides on A2UI's `inlineCatalogs`).
fn with_extensions(options: FakeAgentOptions, extensions: &[KnownExtension]) -> FakeAgentOptions {
    let ui = extensions.contains(&KnownExtension::UiCatalog);
    FakeAgentOptions {
        extensions: extensions.iter().map(|e| e.uri().to_owned()).collect(),
        ui_extensions: if ui {
            vec![A2UI_EXTENSION_V0_9_1.to_owned()]
        } else {
            options.ui_extensions
        },
        accepts_inline_catalogs: ui,
        ..options
    }
}

async fn release_gate(State(agents): State<Arc<Agents>>, Path(name): Path<String>) -> StatusCode {
    match agents.get(&name) {
        Some(agent) => {
            agent.release_gate();
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

async fn calls(
    State(agents): State<Arc<Agents>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let agent = agents.get(&name).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(Value::Array(
        agent.calls().iter().map(call_json).collect(),
    )))
}

#[tokio::main]
async fn main() {
    let listed = extensions();
    let coder = FakeAgent::spawn(with_extensions(
        FakeAgentOptions {
            releases: Some(FakeReleases::sample()),
            bind: Some(addr("FAKE_CODER_ADDR", "127.0.0.1:4021")),
            ..FakeAgentOptions::default()
        },
        &listed,
    ))
    .await;
    let plain = FakeAgent::spawn(with_extensions(
        FakeAgentOptions {
            bind: Some(addr("FAKE_PLAIN_ADDR", "127.0.0.1:4022")),
            ..FakeAgentOptions::default()
        },
        &listed,
    ))
    .await;
    println!("coder: {}", coder.card_url());
    println!("plain: {}", plain.card_url());

    let app = Router::new()
        .route("/__control/{agent}/release-gate", post(release_gate))
        .route("/__control/{agent}/calls", get(calls))
        .with_state(Arc::new(Agents { coder, plain }));
    let control = addr("FAKE_CONTROL_ADDR", "127.0.0.1:4020");
    let listener = tokio::net::TcpListener::bind(control).await.unwrap();
    println!("control: http://{control}/__control");
    axum::serve(listener, app).await.unwrap();
}
