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
mod live;
mod step;
mod thread;
mod thread_tools;
mod title;
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
    KnownExtension, MENTIONS_EXTENSION, STEPS_EXTENSION, TEXT_STREAM_EXTENSION,
    THREAD_TOOLS_EXTENSION, UI_CATALOG_EXTENSION,
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
pub use ids::{AgentId, MAX_AGENT_ID_LEN, ThreadId, UserId, is_valid_agent_id};
pub use live::{LiveChunk, LiveEnd, LiveText, MAX_LIVE_PIECE_BYTES};
pub use step::{
    AgentStepData, MAX_OPEN_STEPS, MAX_STEP_DEPTH, MAX_STEP_DETAIL_CHARS, MAX_STEP_ID_BYTES,
    MAX_STEP_LABEL_CHARS, MAX_STEP_UPDATES, MAX_STEPS_PER_JOB, MCP_SERVER_ICON_PREFIX, STEP_ICONS,
    StepKind, StepLedger, StepPhase, StepReport, StepSource, StepState, record_step,
};
pub use thread::{AgentInfo, AgentSource, AgentTarget, Releases, ThreadRecord, ThreadState};
pub use thread_tools::{Caller, CallerError, ToolsGrant};
pub use title::{
    MAX_MODEL_TITLE_CHARS, MAX_TITLE_ASKS, MAX_TITLE_CHARS, ThreadTitledData, TitleError,
    TitleLedger, TitleSource, TitledBy, check_title, clean_title, title_prompt,
};
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
