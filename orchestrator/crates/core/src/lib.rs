//! Pure core of the orchestrator.
//!
//! This crate holds the types of the chat API contract (`docs/api/chat-api.yaml`)
//! and the one function that decides what happens next: [`transition`]. It has no
//! async runtime, no I/O and no protocol dependencies, so the compiler enforces
//! that it stays pure (ADR 0001, ADR 0004).

mod agent;
mod error;
mod event;
mod extension;
mod gate;
mod ids;
mod thread;
mod transition;
mod ui;
mod ui_catalog;
mod verify;

pub use agent::{AgentTaskState, AgentUpdate};
pub use error::{BoxError, Classify, ErrorClass, report};
pub use event::{
    Actor, ActorType, AgentMessageData, AgentStatus, AgentStatusData, ArtifactData, ErrorData,
    Event, EventBody, EventKind, JobStartedData, Origin, ThreadStateData, UserMessageData,
};
pub use extension::{
    KnownExtension, MENTIONS_EXTENSION, STEPS_EXTENSION, THREAD_TOOLS_EXTENSION,
    UI_CATALOG_EXTENSION,
};
pub use gate::{
    CheckResult, CheckSource, CheckStatus, ChecksReport, CiConclusion, CiPolicy, CiProvider,
    CiReport, DEFAULT_CI_TIMEOUT_SECS, DEFAULT_MAX_ATTEMPTS, DEFAULT_VERIFIER_TIMEOUT_SECS,
    GatePolicy, Hold, Job, JobView, KnownArtifact, MAX_FINDINGS, MAX_FINDINGS_BYTES,
    MAX_SUMMARY_BYTES, MAX_TASK_BYTES, MAX_URL_BYTES, PullRequestRef, PushedRef, Recognised,
    ReworkData, Snapshot, SourceFindings, Timer, Verdict, WatchKey, cap_findings, is_branch_name,
    is_commit_hash, parse_verdict, pull_request_url, recognise_artifact, repo_key,
    verifier_context,
};
pub use ids::{AgentId, ThreadId, UserId};
pub use thread::{AgentInfo, AgentTarget, Releases, ThreadRecord, ThreadState};
pub use transition::{Command, EventDraft, Input, TransitionError, transition};
pub use ui::{
    A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, A2UI_MEDIA_TYPE, MAX_ACTION_CONTEXT_BYTES,
    MAX_ID_BYTES, MAX_OPERATIONS, MAX_OPERATIONS_BYTES, MAX_SURFACE_BYTES, OperationError,
    OperationInfo, SurfaceOp, UiActionData, UiActionError, UiRejection, UiSurfaceData, UiVersion,
    check_operation_list, check_operations, inspect, serialized_len,
};
pub use ui_catalog::{
    Accepted, CatalogError, MAX_CATALOG_BYTES, MAX_CATALOG_COMPONENTS, MAX_CATALOG_DEPTH,
    MAX_CATALOG_ID_BYTES, MAX_CATALOG_VERSION, MAX_SEEN_CATALOGS, Observed, UiCatalogData,
    UiCatalogLedger, UiCatalogRef, UiDelivery, canonical_json, catalog_digest,
};

/// Timestamps are `jiff` instants everywhere (no `f64` time).
pub use jiff::Timestamp;
