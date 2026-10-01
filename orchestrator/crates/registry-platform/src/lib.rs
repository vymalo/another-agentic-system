//! The platform's agent registry as an [`AgentRegistry`] (ADR 0022).
//!
//! another-agentic-platform lists the A2A agents it provisions in one document, the contract
//! `agent-registry/v1` (`docs/extensions/agent-registry-v1.md` there): a linkset in the shape of
//! an RFC 9727 API catalog, with one `item` per agent service (its id, a display name, tags, and
//! the URL of its agent card). This crate reads it over plain HTTP: no Kubernetes access, no
//! platform SDK (ADR 0007), and no UI concept crosses to the platform.
//!
//! - [`PlatformRegistry`] is the port's implementation: **live** (the document is held in this
//!   process only, as long as its `Cache-Control` allows, never in the database), **single
//!   flight** (concurrent readers share one fetch) and **fail closed** (a read that fails makes
//!   the source unavailable and drops the copy, so an agent the platform removed is not
//!   selectable because the registry is down).
//! - [`linkset`] reads the document, purely; [`freshness`] reads its cache headers, purely.
//! - Releases are not here: each agent's card says which releases it offers (ADR 0008).
//!
//! The agents it lists are `AgentEndpoint::a2a(service id, card URL, agent token)` with origin
//! [`AgentSource::Registry`](orch_core::AgentSource::Registry).

mod client;
pub mod freshness;
pub mod linkset;

pub use client::{BuildError, PlatformConfig, PlatformRegistry, SOURCE};
pub use linkset::PROFILE;
