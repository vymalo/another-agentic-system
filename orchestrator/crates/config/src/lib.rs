//! The orchestrator's configuration file ([ADR 0034], [`docs/api/config.md`]).
//!
//! One YAML file, `version: 1`, closed (an unknown key is an error), with **secrets only by
//! reference** (`{ env: NAME }` or `{ file: PATH }`). This crate is the file's definition and its
//! validation, and it is **pure**: it reads no file and no environment variable, and starts no
//! thread. The binary reads the file and the environment and passes them in, as text and through a
//! [`Resolve`].
//!
//! Loading has three passes, so one run lists every error it can find:
//!
//! 1. [`parse_yaml`]: the text becomes a tree. Merge keys are applied; a tag, a key repeated in
//!    one mapping and a key that is not a string are refused. A syntax error is reported alone,
//!    by line and column.
//! 2. [`check`]: the tree is checked against the JSON Schema generated from the types
//!    ([`schema`]), which yields every violation (unknown keys, reserved keys, wrong types,
//!    missing keys, a plain string where a secret goes), then read into [`Config`].
//! 3. [`Config::validate`]: the rules between keys, and every secret reference resolved. It
//!    returns [`Validated`]: the keys and the [`Secrets`].
//!
//! **No error carries a value** ([`ConfigError`]): a message names the key path and the kind.
//! `jsonschema`'s messages, which quote the value they refuse, are never used.
//!
//! Configuration is the composition root's input, not a port ([ADR 0009]): it decides which
//! compiled-in implementation each port gets. This crate has no implementation type in its
//! public types.
//!
//! [ADR 0034]: ../../../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md
//! [ADR 0009]: ../../../docs/decisions/0009-swappable-implementations-at-build-time.md
//! [`docs/api/config.md`]: ../../../docs/api/config.md

mod error;
mod reserved;
mod rules;
mod secret;
mod shape;
mod tree;
mod types;

pub use error::{ConfigError, ErrorKind, render};
pub use reserved::{RESERVED, Reserved, reserved};
pub use rules::{
    DEFAULT_MAX_ATTEMPTS, MAX_ICON_BYTES, MAX_PROMPT_BYTES, MIN_SECRET_BYTES, Prompts, Secrets,
    ToolServerSecrets, Validated,
};
pub use secret::{MAX_SECRET_FILE_BYTES, Resolve, Secret};
pub use shape::{check, schema, schema_text};
pub use tree::parse_yaml;
pub use types::{
    Agents, ArtifactStoreKind, Artifacts, ArtifactsFs, ArtifactsS3, Auth, AuthMode, AuthPermission,
    AuthRole, AuthScope, AuthScopes, Config, DEFAULT_LISTEN, DEFAULT_MAX_FILE_BYTES,
    DEFAULT_MAX_PER_JOB_BYTES, DEFAULT_S3_REGION, DEFAULT_TOOL_SERVER_TIMEOUT_SECS, Database,
    DescriptionTask, Dispatcher, Endpoint, Environment, Gate, GateCi, GateSource, Inbox, Jwt,
    Language, Log, LogFormat, MAX_FILE_BYTES_LIMIT, MAX_PER_JOB_BYTES_LIMIT, Mcp, Models, Prompt,
    Recompute, Registry, Role, SecretRef, Server, SplitScope, Steps, Surface, Tasks, ThreadTools,
    TitleTask, ToolServer, Ui, WebhookGeneric, WebhookGithub, Webhooks,
};

/// The version of the file's format this build reads.
pub const SUPPORTED_VERSION: u32 = 1;

/// The three passes on `text`, for a caller that has nothing to overlay on the tree: the file
/// is `text`, relative paths are relative to `base_dir`, secrets are resolved through
/// `resolver`.
///
/// # Errors
///
/// The syntax error of pass 1, the shape errors of pass 2 or the rule errors of pass 3: the
/// first pass that has any, with all of that pass's errors.
pub fn load(
    text: &str,
    base_dir: &std::path::Path,
    resolver: &dyn Resolve,
) -> Result<Validated, Vec<ConfigError>> {
    let tree = parse_yaml(text)?;
    check(&tree)?.validate(base_dir, resolver)
}
