//! `CredentialBroker` for a person's MCP servers (ADR 0046, the amendment of 2026-10-11): the one
//! implementation of the port, written once, over any [`SecretStore`](orch_ports::SecretStore)
//! and [`UserToolServerStore`](orch_ports::UserToolServerStore).
//!
//! See the README for the flow, the egress guard, the secrets hygiene and the tests.

mod broker;
mod config;
mod discovery;
mod flights;
mod http;
mod material;
mod oauth;

pub use broker::{Broker, BrokerBuilder, egress_refusal};
pub use config::{
    BrokerConfig, DEFAULT_CLIENT_NAME, DEFAULT_CONNECT_TIMEOUT, DEFAULT_FLOW_TTL,
    DEFAULT_LEASE_TTL, DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_REFRESH_MARGIN, DEFAULT_REQUEST_TIMEOUT,
    MAX_FLOW_TTL, MAX_LEASE_TTL, MAX_REFRESH_MARGIN, MAX_RESPONSE_BYTES, MAX_TIMEOUT,
};
pub use http::{HostLookup, LookupFuture, SystemLookup};

/// Why a broker could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// A setting that cannot work: a duration or a limit of zero or above its bound, an empty
    /// client name.
    #[error(
        "the settings of the broker cannot work: a duration or a limit is zero or above its bound, or the client name is empty"
    )]
    Config,
    /// The client metadata URL is not an `https` URL with a path, no fragment and no user
    /// information.
    #[error(
        "the client metadata URL must be an https URL with a path, and no fragment or user information"
    )]
    ClientMetadataUrl,
    /// The HTTP client did not start (its TLS backend).
    #[error("the HTTP client of the broker did not start")]
    Http(#[source] reqwest::Error),
}
