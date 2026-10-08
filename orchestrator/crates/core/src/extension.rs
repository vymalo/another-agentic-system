//! The optional A2A extensions of the orchestrator's own (ADR 0008), by URI.
//!
//! Each one is a convenience an agent may offer: it is detected from the live agent card, read
//! for every call and never remembered, it fails closed (a URI that is not listed exactly is not
//! offered), and an agent without it is plain A2A. The set is closed on purpose (ADR 0004): a new
//! extension is a new variant, and the compiler lists every `match` that has to learn about it.
//!
//! The URIs follow the pattern of the release-channels extension,
//! `https://agents.vymalo.com/a2a/extensions/<name>/v1` (decided 2026-10-01 on the owner's
//! delegation; the contract of each is a page of `docs/api/`). A breaking change is a `v2` URI,
//! never an edit of `v1`.
//!
//! The release-channels extension and A2UI are not here: each has its own reading of the card
//! (the releases a card offers; the A2UI versions it speaks), because each carries parameters the
//! orchestrator uses. These seven carry none.

use std::fmt;

/// The URI of `thread-tools/v1`: the per-thread MCP endpoint an agent may call (ADR 0019).
pub const THREAD_TOOLS_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/thread-tools/v1";
/// The URI of `ui-catalog/v1`: the UI's component catalog sent with the messages (ADR 0023).
pub const UI_CATALOG_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/ui-catalog/v1";
/// The URI of `steps/v1`: the agent's work reported as nested steps (ADR 0025).
pub const STEPS_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/steps/v1";
/// The URI of `mentions/v1`: agents the person addressed, sent as structured references
/// (ADR 0026).
pub const MENTIONS_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/mentions/v1";
/// The URI of `text-stream/v1`: an agent's reply streamed as it is written, relayed live and
/// never stored (ADR 0027; the fifth extension, decided on the owner's delegation on 2026-10-01).
pub const TEXT_STREAM_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/text-stream/v1";
/// The URI of `steer/v1`: a message sent to a task that is running, which the agent reads at its
/// next step (ADR 0036; the contract is `docs/api/steer-v1.md`).
pub const STEER_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/steer/v1";

/// The URI of `usage/v1`: the tokens of each model call and a task's totals (ADR 0056; the
/// contract is `docs/api/usage-v1.md`).
pub const USAGE_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/usage/v1";

/// An extension of the orchestrator's own that a live agent card can list.
///
/// A variant names what a card may offer. Whether the orchestrator already acts on it is up to
/// the slice that builds it: until then the capabilities document tells a client that the agent
/// offers it, and no message changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KnownExtension {
    /// `thread-tools/v1` ([`THREAD_TOOLS_EXTENSION`]).
    ThreadTools,
    /// `ui-catalog/v1` ([`UI_CATALOG_EXTENSION`]).
    UiCatalog,
    /// `steps/v1` ([`STEPS_EXTENSION`]).
    Steps,
    /// `mentions/v1` ([`MENTIONS_EXTENSION`]).
    Mentions,
    /// `text-stream/v1` ([`TEXT_STREAM_EXTENSION`]).
    TextStream,
    /// `steer/v1` ([`STEER_EXTENSION`]).
    Steer,
    /// `usage/v1` ([`USAGE_EXTENSION`]).
    Usage,
}

impl KnownExtension {
    /// Every extension, in the order of the declaration.
    pub const ALL: [KnownExtension; 7] = [
        KnownExtension::ThreadTools,
        KnownExtension::UiCatalog,
        KnownExtension::Steps,
        KnownExtension::Mentions,
        KnownExtension::TextStream,
        KnownExtension::Steer,
        KnownExtension::Usage,
    ];

    /// The extension's URI, as a card lists it and as a message activates it.
    pub const fn uri(self) -> &'static str {
        match self {
            KnownExtension::ThreadTools => THREAD_TOOLS_EXTENSION,
            KnownExtension::UiCatalog => UI_CATALOG_EXTENSION,
            KnownExtension::Steps => STEPS_EXTENSION,
            KnownExtension::Mentions => MENTIONS_EXTENSION,
            KnownExtension::TextStream => TEXT_STREAM_EXTENSION,
            KnownExtension::Steer => STEER_EXTENSION,
            KnownExtension::Usage => USAGE_EXTENSION,
        }
    }

    /// The extension whose URI is exactly `uri`: a near miss (another version, a trailing slash,
    /// another scheme or case) is not an extension this build knows.
    pub fn from_uri(uri: &str) -> Option<Self> {
        KnownExtension::ALL.into_iter().find(|e| e.uri() == uri)
    }
}

impl fmt::Display for KnownExtension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.uri())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_extension_round_trips_through_its_uri() {
        for ext in KnownExtension::ALL {
            assert_eq!(KnownExtension::from_uri(ext.uri()), Some(ext));
        }
    }

    #[test]
    fn the_uris_are_distinct_and_follow_the_pattern() {
        let uris: std::collections::BTreeSet<_> =
            KnownExtension::ALL.iter().map(|e| e.uri()).collect();
        assert_eq!(uris.len(), KnownExtension::ALL.len());
        for uri in uris {
            assert!(uri.starts_with("https://agents.vymalo.com/a2a/extensions/"));
            assert!(uri.ends_with("/v1"));
        }
    }

    #[test]
    fn only_the_exact_uri_counts() {
        for near in [
            "https://agents.vymalo.com/a2a/extensions/ui-catalog/v2",
            "https://agents.vymalo.com/a2a/extensions/ui-catalog/v1/",
            "http://agents.vymalo.com/a2a/extensions/ui-catalog/v1",
            "https://agents.vymalo.com/a2a/extensions/UI-catalog/v1",
            "https://agents.vymalo.com/a2a/extensions/release-channels/v1",
            "",
        ] {
            assert_eq!(KnownExtension::from_uri(near), None, "{near}");
        }
    }
}
