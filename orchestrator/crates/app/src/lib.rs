//! The orchestrator application: the thread service ([`App`]), the durable outbox
//! [`Dispatcher`] and the [`InboxWorker`] that applies timers and reports. Everything here is
//! written against the ports (ADR 0009), so any composition of adapters can run it.

mod app;
mod asks;
mod authz;
mod catalog;
mod directory;
mod dispatcher;
mod error;
mod gate_config;
mod inbox;
pub mod mentions;
mod tasks;
mod tool_servers;

pub use app::{
    AgentDescription, AgentList, App, AppConfig, ApplyOutcome, BranchView, Branches, Creation,
    DEFAULT_MAX_EXPORT_BYTES, DEFAULT_MAX_EXPORT_EVENTS, FeedItem, ForkAt, ForkRequest, Forked,
    Inbound, NewThread, Owners, Received, SiblingView, ThreadExport,
};
pub use asks::{AskCall, AskHandle};
pub use authz::{
    Access, AgentScope, Denied, Permission, Policy, PolicyError, Requester, Resource, RoleGrant,
    Scope, built_in_roles,
};
pub use catalog::{CatalogSchemaError, THREAD_UI_CATALOG_KEY, check_catalog_schemas};
pub use directory::{AgentDirectory, AgentEntry};
pub use dispatcher::{Dispatcher, DispatcherConfig, FileLimits, MAX_FILES_PER_JOB};
pub use error::AppError;
pub use gate_config::{
    CiLayer, DEFAULT_MAX_ATTEMPTS_CAP, GateError, GateLayer, GateRules, Layer,
    MAX_ATTEMPTS_CAP_CEILING, SourceName, THREAD_GATE_KEY, known_sources, pending_reason,
};
pub use inbox::{
    DEFAULT_LEASE_SECS, DEFAULT_MAX_ATTEMPTS, DEFAULT_PARKED_TTL_SECS, DEFAULT_POLL_SECS,
    InboxConfig, InboxWorker,
};
pub use mentions::THREAD_MENTIONS_KEY;
pub use tasks::{PublicConfig, TaskSettings, UiSettings};
pub use tool_servers::{THREAD_TOOLS_KEY, ToolServerInfo};
