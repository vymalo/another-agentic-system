//! Test-only helpers shared by the adapter and end-to-end tests. Nothing here ships.
//!
//! - [`FakeAgent`]: an in-process A2A 1.0 agent built on `a2a-server-lf`, with scripted
//!   behaviour (see [`fake`]), optional bearer auth and an optional release-channels card;
//! - [`TestInstance`]: the orchestrator's router and dispatcher on a real TCP port;
//! - [`Chat`] and [`SseClient`]: small HTTP clients for the chat API and the AG-UI run route;
//! - [`eventually`]: wait-until with a deadline, instead of sleeping.
//!
//! The helpers panic on failure (they are for tests), hence the lint allowances.
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod fake;
mod instance;
mod sse;
mod wait;

pub use fake::{Call, CallKind, FakeAgent, FakeAgentOptions, FakeReleases};
pub use instance::{Chat, TestInstance, fast_dispatcher, shape};
pub use sse::{Frame, Item, SseClient};
pub use wait::{DEFAULT_TIMEOUT, eventually, eventually_within};
