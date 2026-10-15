//! Ports of the orchestrator (ADR 0009): one trait per infrastructure boundary.
//!
//! - [`ThreadStore`]: threads, per-thread events with a strictly increasing `seq`, an outbox;
//! - the inbox (unsolicited webhook reports and timers) is part of [`ThreadStore`]: a thread commit
//!   must be atomic with the inbox row it applies;
//! - [`Wakeup`]: notify/listen hints, and the live text of a reply being written (never stored,
//!   ADR 0027);
//! - [`AgentClient`]: talking to a delegated agent (send, stream, resubscribe, poll, cancel, card),
//!   and [`ByTransport`], which serves one endpoint set from two clients (remote A2A, in-process);
//! - [`Authenticator`]: who is calling, from the credentials a request carried (a bearer token or
//!   the identity header of a proxy), failing closed (ADR 0033);
//! - [`ChatModel`]: one question to a language model and its answer (the orchestrator's titles);
//! - [`AgentRegistry`]: which agents exist right now, read live and failing closed (ADR 0022), with
//!   [`FixedRegistry`] (the static list) and [`CompositeRegistry`] (two registries as one);
//! - [`ArtifactStore`]: where the files agents hand over are kept, by the hash of their content;
//!   the log keeps only the reference (ADR 0032); [`NoArtifacts`] is a deployment without one;
//! - [`ToolServerClient`]: the orchestrator as a client of an MCP server, to list its tools and call
//!   one (ADR 0024); what a server answers is untrusted text, and its credentials are
//!   [`ToolSecret`]s that never print;
//! - [`CredentialBroker`]: the credentials of a person's MCP servers: store a key, run an OAuth
//!   authorization, lend a fresh [`CredentialLease`] for one call, revoke (ADR 0046); the port only,
//!   `orch-credential-broker` implements it;
//! - [`SecretStore`]: where the opaque, sealed material of a person's credentials is kept, by
//!   `(owner, server)`, with a compare-and-swap `rotate` (ADR 0046); [`SecretMaterial`] never prints;
//! - [`UserToolServerStore`]: the definitions of a person's own MCP servers, by `(owner, id)`, at most
//!   eight per person; a definition holds no secret and a URL the guard accepted, and every write of
//!   an existing one is a compare and swap on a [`ServerRevision`] (ADR 0046);
//! - [`UserToolPorts`] (with [`UserTools`], [`UserToolsMode`] and the refusing [`NoUserTools`]): the
//!   definitions store and the broker held together as the bundle holds the others, with the mode a
//!   deployment chose (fixed by the constructor, never set beside the ports) and its egress policy,
//!   and no secret store; [`NoUserToolServers`], [`NoSecrets`] and [`NoBroker`] refuse every call, and
//!   [`KeepsCredentials`] marks the brokers that do not (ADR 0046);
//! - [`PendingAuthorizations`]: the OAuth authorizations that have begun and whose callback has not
//!   come, `state → (owner, server, until)`, used at most once, kept by the state's digest (ADR 0046,
//!   slice 4c); [`NoPendingAuthorizations`] refuses every call;
//! - [`EgressPolicy`]: whether a URL a person typed may be fetched and whether a connection may be
//!   made to the addresses its host resolved to (ADR 0046, decision 5); pure, so the broker and the
//!   relay apply one function;
//! - [`Clock`], [`IdGen`]: time and identifiers.
//!
//! No implementation type appears in any signature. Implementations live in separate crates;
//! this crate ships in-memory ones behind the `testkit` feature together with a conformance
//! testkit every `ThreadStore` / `Wakeup` / `AgentClient` / `AgentRegistry` / `ChatModel` / `ArtifactStore` /
//! `ToolServerClient` implementation must pass.

mod agent;
mod artifacts;
mod auth;
mod bundle;
mod clock;
mod credentials;
mod egress;
mod inbox;
mod model;
mod pending;
mod registry;
mod route;
mod secrets;
mod store;
mod tools;
mod user_tools;
mod wakeup;

#[cfg(feature = "testkit")]
pub mod memory;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use agent::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream,
    AgentTransport, IdemKey, MentionInfo, SendContent, SendRequest, TaskHandle, TaskSnapshot,
    UiSupport,
};
pub use artifacts::{
    ArtifactError, ArtifactHead, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, CHUNK_BYTES,
    MAX_NAME_BYTES, MAX_OVERRIDE_BYTES, MAX_PRESIGN_TTL, NoArtifacts, PresignedGet, PresignedPut,
    ResponseOverrides, stream_of,
};
pub use auth::{
    AuthError, Authenticator, ByCredential, CredentialKind, Credentials, DpopCredentials,
    Principal, RefuseAll, Role,
};
pub use bundle::{NoUserTools, PortSet, Ports, UserToolPorts, UserTools, UserToolsMode};
pub use clock::{Clock, IdGen, SystemClock, UuidV7Ids};
pub use credentials::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, KeepsCredentials,
    MAX_CREDENTIAL_REF_BYTES, MAX_OAUTH_STATE_BYTES, MIN_OAUTH_STATE_BYTES, NoBroker, OAuthStart,
    OAuthState, OAuthStateError, RedirectUri, RedirectUriError,
};
pub use egress::{
    AddressClass, CheckedAddrs, EgressHost, EgressPolicy, EgressRefusal, EgressTarget, HostPattern,
    HostPatternError, MAX_URL_BYTES, classify_address, is_public_address,
};
pub use inbox::{
    InboxFinal, InboxId, InboxItem, InboxLease, InboxPayload, InboxStatus, NewInbox, NewTimer,
    Parking, Received, TIMER_SOURCE, UndecodablePayload,
};
pub use model::{ChatModel, ChatRequest, ModelError, NoModel};
pub use pending::{
    NoPendingAuthorizations, PendingAuthorization, PendingAuthorizationError,
    PendingAuthorizations, StateDigest,
};
pub use registry::{
    AgentListing, AgentRegistry, CompositeRegistry, FixedRegistry, RegistryEntry, RegistryError,
    STATIC_SOURCE, SourceStatus,
};
pub use route::ByTransport;
pub use secrets::{
    MAX_SECRET_BYTES, MaterialTooLarge, NoSecrets, SecretError, SecretMaterial, SecretServer,
    SecretStore, SecretVersion,
};
pub use store::{
    AgentBinding, ArchivedFilter, Arrangement, BindingUpdate, Commit, CommitOutcome, ForkOrigin,
    Lease, ListOrder, NewEvent, NewOutbox, NewThreadRecord, OutboxFinal, OutboxId, OutboxItem,
    OutboxKind, OutboxPayload, OutboxStats, OutboxStatus, Place, SharingChange, StoreError,
    ThreadListing, ThreadStore,
};
pub use tools::{
    MAX_REMOTE_MESSAGE_BYTES, MAX_RESULT_BYTES, RESERVED_HEADERS, ToolCall, ToolCallOutput,
    ToolDef, ToolSecret, ToolServerClient, ToolServerEndpoint, ToolServerError, is_reserved_header,
};
pub use user_tools::{
    DefinitionError, MAX_CLIENT_ID_BYTES, MAX_HEADER_NAME_BYTES, MAX_NAME_CHARS, MAX_SCOPE_BYTES,
    MAX_SCOPES, MAX_USER_TOOL_SERVERS, NoUserToolServers, ServerRevision, StoredServer,
    ToolServerStatus, UnavailableReason, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};
pub use wakeup::{Topic, Wakeup, WakeupCapabilities, WakeupError};
