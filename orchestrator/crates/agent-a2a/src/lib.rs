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
//! The thread tools (`thread-tools/v1`, ADR 0023) are another: when the live card lists the
//! extension and the adapter has an issuer ([`A2aConfig::thread_tools`]), the message carries
//! `{url, token, expiresAt}` for the thread's MCP endpoint, minted when it is sent from the
//! non-secret grant on the request and never stored or logged ([`thread_tools_metadata`]).
//!
//! The mentions of a message (`mentions/v1`, ADR 0026) are another: when the live card lists the
//! extension and the message mentions agents, the references ride in the message metadata under
//! the URI (the agent's name and card URL as the registry gave them when the message was sent) and
//! the URI is activated; an agent whose card does not list it is sent the text as it is.
//!
//! Steps (`steps/v1`, ADR 0025) are another: when the live card lists the extension
//! ([`steps_from_card`]) the URI is activated on `SendStreamingMessage` (header and
//! `message.extensions`) **and on `SubscribeToTask`** (the card is read for it too), so an agent
//! reports its work as nested steps; the response is read as data whether or not it was activated
//! (`orch_a2a_mapping`).
//!
//! Streamed text (`text-stream/v1`, ADR 0027) is activated the same way, on the same two calls,
//! when the live card lists it ([`text_stream_from_card`]): the agent then sends its reply as chunks
//! as it writes it, which the adapter maps to live pieces that are relayed and never applied
//! (`orch_a2a_mapping`), and states the whole text once.
//!
//! Token usage (`usage/v1`, ADR 0056) is activated the same way, on the same two calls, when the
//! live card lists it ([`usage_from_card`]): the agent then reports each model call's tokens on a
//! `working` status update and its task's totals when the task ends or pauses, which the mapper
//! reads (`orch_a2a_mapping`). A stream shows no task metadata, so when such a stream reaches a
//! status that ends or pauses the task **without** the totals in it, the adapter reads the task once
//! (`GetTask`) and passes its totals on before the status; a read that fails passes nothing.
//!
//! Files (ADR 0032): an artifact's `raw` part reaches the worker as `AgentUpdate::File` (the
//! mapper), and a `url` part on a host of [`A2aConfig::fetch_files`] is read here and reaches it the
//! same way ([`FileFetch`]); any other `url` part stays a link.
//!
//! Release channels (ADR 0008) are an optional extension: [`releases_from_card`] reads them
//! from the live card, and a selected release is sent as the `A2A-Extensions` header plus
//! namespaced message metadata. Nothing here depends on a specific agent host.

mod a2ui;
mod client;
mod errors;
mod extensions;
mod files;
mod mentions;
mod releases;
mod thread_tools;

pub use a2ui::{
    action_part, client_capabilities, inline_catalog, ui_catalog_metadata, ui_from_card,
};
pub use client::{A2aAgentClient, A2aConfig, BuildError, install_crypto_provider};
pub use extensions::{
    build_from_card, extensions_from_card, steps_from_card, text_stream_from_card, usage_from_card,
};
pub use files::FileFetch;
pub use releases::{RELEASE_CHANNELS_URI, releases_from_card};
pub use thread_tools::thread_tools_metadata;
