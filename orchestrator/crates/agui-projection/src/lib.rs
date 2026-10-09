//! The AG-UI projection of the orchestrator's event log, as pure functions.
//!
//! The event log is the only source of truth (ADR 0001); AG-UI is a *view* of it
//! ([ADR 0012](../../../../docs/decisions/0012-ag-ui-user-facing-protocol.md), binding in
//! `docs/api/agui.md`). This crate computes that view and nothing else:
//!
//! - [`Projector`] folds core [`orch_core::Event`]s into AG-UI [`Frame`]s, one call per log
//!   event, for an [`Audience`]. It reads no clock and no store; the same events always give
//!   the same frames, so every replica and every replay agrees.
//! - [`LiveOverlay`] adds the words of a reply that is still being written (ADR 0027) to what a
//!   projector says, without touching the projector's fold: frames that are not in the log and
//!   are never resume points, merged by message id with the log's final message.
//! - [`Projector::resume_preamble`] re-opens the current run for a client that reconnects with a
//!   cursor in the middle of it.
//! - [`Connect`] is the fold behind the connect stream: the frames a client gets when it attaches
//!   with a cursor (folded silently up to the cursor, then the preamble, then everything after),
//!   and when a `?mode=run` stream ends.
//! - [`History`] is the fold behind the history read: a finite page of the same frames, a whole
//!   number of settled chains from the newest one back, or the chains after a point.
//! - [`agent_capabilities`] builds the capabilities document of an agent from its live card.
//! - [`translate`] turns a [`orch_agui_proto::RunAgentInput`] into core [`orch_core::Input`]s,
//!   given a [`ThreadView`] of what the log already holds.
//!
//! There is no `async` and no I/O here, on purpose: the compiler enforces that the projection
//! stays a function of the log. The HTTP surface (`orch-surface-agui`) is the adapter that feeds
//! it.

mod capabilities;
mod carry;
mod connect;
mod frame;
mod history;
mod live;
mod projector;
mod translate;
mod usage;
mod vocab;

pub use capabilities::{CardFacts, agent_capabilities};
pub use carry::Carry;
pub use connect::{Connect, Follow};
pub use frame::{Audience, Frame};
pub use history::{Anchor, Flow, History, HistoryLimits, PROJECTION_VERSION, Page, Window};
pub use live::{LiveOverlay, MAX_LIVE_MESSAGE_BYTES};
pub use projector::{Projector, ThreadMeta};
pub use translate::{
    InputError, KnownThread, ThreadView, Translation, Warning, held_message_ids, release_selector,
    thread_id_of, translate, translate_with_warnings,
};
pub use vocab::{
    A2UI_OPERATIONS_KEY, ACTIVITY_A2UI_SURFACE, ACTIVITY_ACTION, ACTIVITY_ARTIFACT, ACTIVITY_CHECK,
    ACTIVITY_CI, ACTIVITY_ERROR, ACTIVITY_JOB, ACTIVITY_REWORK, ACTIVITY_STATUS, ACTIVITY_STEP,
    ACTIVITY_TOOLS, ACTOR_KEY, AT_KEY, CODE_AGENT_FAILED, CODE_CHECKS_FAILED, CODE_DELIVERY_FAILED,
    CODE_STEP_FAILED, CODE_VERIFIER_FAILED, CUSTOM_USAGE, CUSTOM_USAGE_TOTAL, DELIVERY_KEY,
    LIVE_KEY, MENTIONS_KEY, PROBLEM_KEY, PURPOSE_KEY, RELEASE_CHANNELS_URI, SEND_KEY, VIA_KEY,
    WHEN_KEY,
};
