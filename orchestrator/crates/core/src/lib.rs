//! Pure core of the orchestrator.
//!
//! This crate holds the types of the chat API contract (`docs/api/chat-api.yaml`)
//! and the one function that decides what happens next: [`transition`]. It has no
//! async runtime, no I/O and no protocol dependencies, so the compiler enforces
//! that it stays pure (ADR 0001, ADR 0004).

mod agent;
mod answer;
mod description;
mod error;
mod event;
mod extension;
mod fork;
mod gate;
mod ids;
mod language;
mod live;
mod redact;
mod step;
mod task;
mod thread;
mod thread_tools;
mod title;
mod tools;
mod transition;
mod ui;
mod ui_catalog;
mod verify;

pub use agent::{AgentTaskState, AgentUpdate, FileRefusal};
pub use answer::{AnswerError, AnswerLedger, MAX_ANSWER_BYTES, check_answer};
pub use description::{
    DEFAULT_DESCRIPTION_CHARS, DEFAULT_MIN_NEW_MESSAGES, DescribedBy, DescriptionError,
    DescriptionLedger, DescriptionSource, MAX_DESCRIPTION_CHARS, ThreadDescribedData,
    check_description, clean_description, messages_since_description,
};
pub use error::{BoxError, Classify, ErrorClass, report};
pub use event::{
    Actor, ActorType, AgentMessageData, AgentStatus, AgentStatusData, AnswerVia, ArtifactData,
    Delivery, ErrorData, Event, EventBody, EventKind, FileRef, JobStartedData, MessagePurpose,
    Origin, Preview, ThreadStateData, UserMessageData,
};
pub use extension::{
    KnownExtension, MENTIONS_EXTENSION, STEER_EXTENSION, STEPS_EXTENSION, TEXT_STREAM_EXTENSION,
    THREAD_TOOLS_EXTENSION, UI_CATALOG_EXTENSION,
};
pub use fork::{
    BranchPoint, EditLink, ForkError, ForkHistory, ForkKind, ForkNode, ForkPoint, ForkSource,
    ForkedFrom, HistoryEntry, HistoryRole, MAX_FORK_FAMILY, MAX_HISTORY_BYTES,
    MAX_HISTORY_ENTRY_BYTES, Replacement, Sibling, ThreadForkedData, branch_points, copied,
    family_root, fork_commit, fork_cut, fork_history, forked_snapshot, history_preamble,
};
pub use gate::{
    CheckResult, CheckSource, CheckStatus, ChecksReport, CiConclusion, CiPolicy, CiProvider,
    CiReport, DEFAULT_CI_TIMEOUT_SECS, DEFAULT_MAX_ATTEMPTS, DEFAULT_VERIFIER_TIMEOUT_SECS,
    GatePolicy, Hold, Job, JobView, KnownArtifact, MAX_AFTER_STOP_BYTES, MAX_FINDINGS,
    MAX_FINDINGS_BYTES, MAX_SUMMARY_BYTES, MAX_TASK_BYTES, MAX_URL_BYTES, PullRequestRef,
    PushedRef, Recognised, ReworkData, Snapshot, SourceFindings, Timer, Verdict, WatchKey,
    cap_findings, is_branch_name, is_commit_hash, parse_verdict, pull_request_url,
    recognise_artifact, repo_key, verifier_context,
};
pub use ids::{AgentId, MAX_AGENT_ID_LEN, ThreadId, UserId, is_valid_agent_id};
pub use language::{
    INSTRUCTION_UNKNOWN, Lang, Script, ScriptMismatch, detect, instruction_unknown_for,
    script_mismatch, script_mismatch_fixed, scripts_of,
};
pub use live::{LiveChunk, LiveEnd, LiveText, MAX_LIVE_PIECE_BYTES};
pub use redact::{REDACTED, is_secret_key, redact_text, redact_value};
pub use step::{
    AgentStepData, MAX_OPEN_STEPS, MAX_STEP_DEPTH, MAX_STEP_DETAIL_CHARS, MAX_STEP_ID_BYTES,
    MAX_STEP_IO_BYTES_PER_JOB, MAX_STEP_LABEL_CHARS, MAX_STEP_UPDATES, MAX_STEPS_PER_JOB,
    MCP_SERVER_ICON_PREFIX, STEP_ICONS, STEP_INPUT_MAX_BYTES, STEP_INPUT_STRING_MAX_CHARS,
    STEP_OUTPUT_MAX_BYTES, StepKind, StepLedger, StepOutput, StepPhase, StepReport, StepSource,
    StepState, record_step,
};
pub use task::{
    DESCRIPTION_MESSAGES, LanguageRule, MAX_GUIDANCE_BYTES, TaskKind, TaskLanguageError,
    TaskPrompt, check_task_language, conversation_language, task_prompt,
};
pub use thread::{AgentInfo, AgentSource, AgentTarget, Releases, ThreadRecord, ThreadState};
pub use thread_tools::{Caller, CallerError, ToolsGrant};
pub use title::{
    MAX_MODEL_TITLE_CHARS, MAX_TITLE_ASKS, MAX_TITLE_CHARS, ThreadTitledData, TitleError,
    TitleLanguageError, TitleLedger, TitleSource, TitledBy, check_title, check_title_language,
    clean_title, title_prompt, title_retry_prompt,
};
pub use tools::{
    AttachedServer, MAX_ATTACHED_SERVERS, MAX_SERVER_ID_BYTES, ToolsData, ToolsError, attached_by,
    check_servers, is_valid_server_id,
};
pub use transition::{Command, EventDraft, Input, TransitionError, start_thread, transition};
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
