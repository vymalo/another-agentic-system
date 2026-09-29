//! The AG-UI projection of the orchestrator's event log, as pure functions.
//!
//! The event log is the only source of truth (ADR 0001); AG-UI is a *view* of it
//! ([ADR 0012](../../../../docs/decisions/0012-ag-ui-user-facing-protocol.md), binding in
//! `docs/api/agui.md`). This crate computes that view and nothing else:
//!
//! - [`Projector`] folds core [`orch_core::Event`]s into AG-UI [`Frame`]s, one call per log
//!   event, for an [`Audience`]. It reads no clock and no store; the same events always give
//!   the same frames, so every replica and every replay agrees.
//! - [`Projector::resume_preamble`] re-opens the current run for a client that reconnects with a
//!   cursor in the middle of it.
//! - [`translate`] turns a [`orch_agui_proto::RunAgentInput`] into core [`orch_core::Input`]s,
//!   given a [`ThreadView`] of what the log already holds.
//!
//! There is no `async` and no I/O here, on purpose: the compiler enforces that the projection
//! stays a function of the log. The HTTP surface (`orch-surface-agui`) is the adapter that feeds
//! it.

mod frame;
mod projector;
mod translate;
mod vocab;

pub use frame::{Audience, Frame};
pub use projector::{Projector, ThreadMeta};
pub use translate::{
    InputError, KnownThread, ThreadView, Translation, Warning, held_message_ids, release_selector,
    thread_id_of, translate, translate_with_warnings,
};
pub use vocab::{
    ACTIVITY_ARTIFACT, ACTIVITY_ERROR, ACTIVITY_STATUS, ACTOR_KEY, CODE_AGENT_FAILED,
    CODE_DELIVERY_FAILED, PROBLEM_KEY, RELEASE_CHANNELS_URI,
};
