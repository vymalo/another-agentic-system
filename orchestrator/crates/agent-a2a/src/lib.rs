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
//! A2UI (ADR 0013, ADR 0075) is another: [`ui_from_card`] reads it from the live card, the
//! capabilities of the renderer are sent with a message only when the card lists it, and the
//! user's action goes back as an `application/a2ui+json` data part. The UI's component catalog
//! (ADR 0023) rides on it when the card also lists `ui-catalog/v2`: [`extensions_from_card`] reads
//! which of the orchestrator's own extensions a card lists.
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
//! The access tier of a message (`access-tier/v1`, ADR 0061) is another: when the message names a tier
//! and the live card read for the call honours it ([`access_tier_from_card`]), the tier rides in the
//! message metadata under the URI and the URI is activated; when the card no longer honours it,
//! nothing is sent and the call answers `Unsupported`, never the message without its tier.
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
//! reads (`orch_a2a_mapping`). The totals are in the task's metadata, which a stream does not show,
//! so when such a stream reaches a status that ends or pauses the task, the adapter reads the task
//! once (`GetTask`) and passes its totals on before the status; a read that fails passes nothing.
//!
//! Child sessions (`child-sessions/v1`, ADR 0072) are activated on a send (a steer too), a
//! resubscribe, a `GetTask`, a `CancelTask` and a `ListTasks`, each only when the card read for that
//! very call offers the extension well formed ([`child_sessions_from_card`]); what an agent says of
//! sessions to a call that did not activate it is dropped. A stream that activated it also reads its
//! task for its sessions, once for the status that ends or pauses the turn (said after what that
//! update said itself, and before its status, so that the listing is the last word) and every
//! [`A2aConfig::child_sessions_poll`] while it runs and has carried no event, and passes them on
//! (`orch_a2a_mapping`). The reads run inline in the stream's loop.
//!
//! Files (ADR 0032): an artifact's `raw` part reaches the worker as `AgentUpdate::File` (the
//! mapper), and a `url` part on a host of [`A2aConfig::fetch_files`] is read here and reaches it the
//! same way ([`FileFetch`]); any other `url` part stays a link. A part that names an upload
//! (`uploads/v1`, ADR 0058) reaches it as `AgentUpdate::FileUploaded` (the mapper), and the
//! extension is activated on a message, with the deployment's limits in its metadata, only when the
//! live card lists `uploads/v1` and `thread-tools/v1`, a grant was minted for the message and the
//! request's grant offers uploads.
//!
//! Release channels (ADR 0008) are an optional extension: [`releases_from_card`] reads them
//! from the live card, and a selected release is sent as the `A2A-Extensions` header plus
//! namespaced message metadata. Nothing here depends on a specific agent host.
//!
//! The interface locale of a message (`locale/v1`, ADR 0085) is a hint, not a promise: when the
//! message names a BCP 47 tag and the live card read for the call lists the extension well formed
//! ([`locale_from_card`]), the tag rides in the message metadata under the URI and the URI is
//! activated; a card that does not list it, or lists it badly, gets the plain message.
//!
//! The model and the reasoning effort of a message (`model-pick/v1` and adam's `reasoning/v1`,
//! ADR 0086) are **not** hints: when the message names either, the live card read for the call must
//! offer it ([`pick_from_card`]); the choice then rides in the message metadata under each URI
//! and each URI is activated, and a card that does not offer it is [`AgentError::Unsupported`] and
//! nothing is sent.
//!
//! Trusted components (`trusted-components/v1`, ADR 0083) are activated on a send and a
//! resubscribe when the card read for that very call offers the extension well formed
//! ([`trusted_components_from_card`]); the agent then asks the client to draw one in its
//! `Message` metadata, and a request for a component the card did not offer is dropped
//! (`orch_a2a_mapping` reads the request).

mod a2ui;
mod access_tier;
mod child_sessions;
mod client;
mod commands;
mod errors;
mod extensions;
mod files;
mod locale;
mod mentions;
mod pick;
mod releases;
mod thread_tools;
mod trusted;
mod uploads;

pub use a2ui::{
    action_part, inline_catalog, renderer_capabilities, ui_catalog_metadata, ui_from_card,
};
pub use access_tier::access_tier_from_card;
pub use child_sessions::child_sessions_from_card;
pub use client::{A2aAgentClient, A2aConfig, BuildError, install_crypto_provider};
pub use commands::commands_from_card;
pub use extensions::{
    build_from_card, extensions_from_card, steps_from_card, text_stream_from_card, usage_from_card,
};
pub use files::FileFetch;
pub use locale::locale_from_card;
pub use pick::pick_from_card;
pub use releases::{RELEASE_CHANNELS_URI, releases_from_card};
pub use thread_tools::thread_tools_metadata;
pub use trusted::trusted_components_from_card;
