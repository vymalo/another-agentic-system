//! Test-only helpers shared by the adapter and end-to-end tests. Nothing here ships.
//!
//! - [`FakeAgent`]: an in-process A2A 1.0 agent built on `a2a-server-lf`, with scripted
//!   behaviour (see [`fake`]), optional bearer auth and an optional release-channels card;
//! - [`TestInstance`]: the orchestrator's router and dispatcher on a real TCP port;
//! - [`Chat`] and [`SseClient`]: small HTTP clients for the resource API and the AG-UI routes (a
//!   client from [`TestInstance::chat`] also reads the event log in-process);
//! - [`ui_catalog`] and [`with_ui_catalog`]: the UI catalog as the web sends it (ADR 0023);
//! - [`call_back`] and [`announce`]: the agent's side of the thread tools (`thread-tools/v1`), what
//!   the fake agent's `thread-tools` and `turn-output` scripts do with the grant in its message
//!   (ADR 0023, ADR 0031);
//! - [`call_tool`]: what the fake agent's `tool <name> <json>` script does with the endpoint, a call of a
//!   relayed tool (ADR 0024);
//! - [`FakeToolServer`]: a real MCP server over streamable HTTP with the four tools of the tool-server
//!   testkit, a bearer check and a journal (ADR 0024);
//! - [`eventually`]: wait-until with a deadline, instead of sleeping.
//!
//! The helpers panic on failure (they are for tests), hence the lint allowances.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod catalog;
pub mod fake;
mod instance;
mod sse;
mod thread_tools;
mod tool_server;
mod wait;

pub use catalog::{UI_CATALOG_ID, integral_numbers, ui_catalog, with_ui_catalog};
pub use fake::{
    Call, CallKind, FakeAgent, FakeAgentOptions, FakeReleases, STREAM_PIECES, VerifierScript,
    stream_id, stream_text,
};
pub use instance::{Chat, TestInstance, fast_dispatcher, shape};
pub use sse::{Frame, SseClient};
pub use thread_tools::{announce, call_back, call_tool};
pub use tool_server::{FakeToolServer, FakeToolServerOptions};
pub use wait::{DEFAULT_TIMEOUT, eventually, eventually_within};
