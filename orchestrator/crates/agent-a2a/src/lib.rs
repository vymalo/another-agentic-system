//! The A2A adapter: an [`orch_ports::AgentClient`] over `a2a-client-lf`.
//!
//! Protocol notes (A2A 1.0, verified against the SDK sources 2026-09-29):
//! - methods used: `SendStreamingMessage`, `SubscribeToTask` (the port's `resubscribe`),
//!   `GetTask`, `CancelTask`, `ListTasks`;
//! - `SubscribeToTask` only works while the task executes in the answering process; a finished
//!   task or one running elsewhere yields `TASK_NOT_FOUND`, which the dispatcher answers by
//!   polling `GetTask`;
//! - the SDK loses the HTTP status of failed calls; errors are classified by JSON-RPC code and
//!   by the SDK's message prefixes (see the `errors` module).
//!
//! A2UI (ADR 0013) is another: [`ui_from_card`] reads it from the live card, the capabilities of
//! the renderer are sent with a message only when the card lists it, and the user's action goes
//! back as an `application/a2ui+json` data part. The UI's component catalog (ADR 0023) rides on it
//! when the card also lists `ui-catalog/v1`: [`extensions_from_card`] reads which of the
//! orchestrator's own extensions a card lists.
//!
//! Release channels (ADR 0008) are an optional extension: [`releases_from_card`] reads them
//! from the live card, and a selected release is sent as the `A2A-Extensions` header plus
//! namespaced message metadata. Nothing here depends on a specific agent host.

mod a2ui;
mod client;
mod errors;
mod extensions;
mod releases;

pub use a2ui::{
    action_part, client_capabilities, inline_catalog, ui_catalog_metadata, ui_from_card,
};
pub use client::{A2aAgentClient, A2aConfig, BuildError, install_crypto_provider};
pub use extensions::extensions_from_card;
pub use releases::{RELEASE_CHANNELS_URI, releases_from_card};
