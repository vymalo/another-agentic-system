//! The agent registry port (ADR 0022): which agents exist right now, and where each one lives.
//!
//! A registry is read **live**: [`AgentRegistry::list`] and [`AgentRegistry::get`] are asked
//! whenever a list or a lookup is needed, and an implementation may keep a copy in its own
//! process only as long as its source's HTTP cache headers allow (ADR 0008). Nothing here is
//! stored in the database.
//!
//! A registry **fails closed**: a source that cannot be read lists none of its agents, even if
//! it listed some a moment ago, and says so ([`SourceStatus`]), so that the UI can tell the
//! person the list is incomplete and a lookup can tell "no such agent" from "cannot tell".
//!
//! Two implementations live here: [`FixedRegistry`], the static list the deployment configures,
//! and [`CompositeRegistry`], which reads two registries and merges them (the first wins on an
//! id both list). The platform's registry is a crate of its own, `orch-registry-platform`.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use orch_core::{AgentId, AgentSource, BoxError, Classify, ErrorClass};

use crate::AgentEndpoint;

/// One agent a registry lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    /// Where the agent lives. For an agent a registry lists, the `id` is the registry's
    /// service id, which satisfies [`orch_core::is_valid_agent_id`].
    pub endpoint: AgentEndpoint,
    /// Display name.
    pub name: String,
    /// Labels the registry keeps on the agent. They classify; they route nothing.
    pub tags: Vec<String>,
    /// Which kind of source lists it.
    pub origin: AgentSource,
}

impl RegistryEntry {
    /// The agent's id.
    pub fn id(&self) -> &AgentId {
        &self.endpoint.id
    }
}

/// Whether one source of agents could be read, as the last read found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStatus {
    /// The source: `static` for the deployment's list, `platform` for the platform's registry
    /// (a short word an operator and a UI can show; never a URL).
    pub name: String,
    /// Whether it answered. `false` means none of its agents are in the listing.
    pub available: bool,
    /// Why it did not, in words fit for a person: never a URL, a token or a stack of causes.
    pub detail: Option<String>,
}

impl SourceStatus {
    /// A source that answered.
    pub fn ok(name: impl Into<String>) -> Self {
        SourceStatus {
            name: name.into(),
            available: true,
            detail: None,
        }
    }

    /// A source that did not, and why.
    pub fn unavailable(name: impl Into<String>, detail: impl Into<String>) -> Self {
        SourceStatus {
            name: name.into(),
            available: false,
            detail: Some(detail.into()),
        }
    }
}

/// What [`AgentRegistry::list`] answers: the agents, and how each source fared.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentListing {
    /// Every agent listed now, in display order. An id appears once.
    pub entries: Vec<RegistryEntry>,
    /// One status per source, in the order the sources were asked.
    pub sources: Vec<SourceStatus>,
}

impl AgentListing {
    /// The sources that could not be read.
    pub fn unavailable(&self) -> impl Iterator<Item = &SourceStatus> {
        self.sources.iter().filter(|s| !s.available)
    }

    /// Whether every source answered.
    pub fn is_complete(&self) -> bool {
        self.sources.iter().all(|s| s.available)
    }
}

/// A source that could have listed an agent did not answer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The source could not be read. Retrying may help.
    #[error("agent registry {name} unavailable: {detail}")]
    Unavailable {
        /// The source, as in [`SourceStatus::name`].
        name: String,
        /// Why, in words fit for a person (see [`SourceStatus::detail`]).
        detail: String,
        /// The cause, for the operator's log.
        #[source]
        source: Option<BoxError>,
    },
}

impl RegistryError {
    /// The source `name` could not be read.
    pub fn unavailable(name: impl Into<String>, detail: impl Into<String>) -> Self {
        RegistryError::Unavailable {
            name: name.into(),
            detail: detail.into(),
            source: None,
        }
    }

    /// Adds the cause.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            RegistryError::Unavailable { name, detail, .. } => RegistryError::Unavailable {
                name,
                detail,
                source: Some(source.into()),
            },
        }
    }
}

impl Classify for RegistryError {
    fn class(&self) -> ErrorClass {
        match self {
            RegistryError::Unavailable { .. } => ErrorClass::Transient,
        }
    }
}

/// The agents the orchestrator can delegate to (ADR 0022).
///
/// An implementation holds these rules, which [`testkit::registry`](crate::testkit) states as
/// cases:
///
/// - **Live.** A change in the source shows on the next read (within the freshness its own
///   protocol allows).
/// - **Fail closed.** A source that cannot be read lists nothing, never a stale copy, and says
///   so in [`AgentListing::sources`]; [`get`](Self::get) says it cannot tell.
/// - **Valid entries only.** An entry whose id is not an agent id
///   ([`orch_core::is_valid_agent_id`]) is not listed, and an id is listed once.
pub trait AgentRegistry: Send + Sync + 'static {
    /// Every agent listed now, in display order. A source that cannot be read is
    /// `available: false` and lists nothing.
    fn list(&self) -> impl Future<Output = AgentListing> + Send;

    /// The agent `id`: `Ok(Some)` when listed; `Ok(None)` when every source answered and none
    /// lists it; `Err` when a source that could list it did not answer, so that "no such agent"
    /// is never said while the registry is down.
    fn get(
        &self,
        id: &AgentId,
    ) -> impl Future<Output = Result<Option<RegistryEntry>, RegistryError>> + Send;
}

/// The name of [`FixedRegistry`] in [`SourceStatus`].
pub const STATIC_SOURCE: &str = "static";

/// A fixed list of agents: the deployment's own (`AGENTS_FILE`). It never changes after it is
/// built and is never unavailable; its source is named [`STATIC_SOURCE`].
///
/// Cheap to clone (the entries are shared).
#[derive(Debug, Clone, Default)]
pub struct FixedRegistry {
    entries: Arc<[RegistryEntry]>,
}

impl FixedRegistry {
    /// A registry of these entries, in this order. The caller has validated them (the binary
    /// does when it reads `AGENTS_FILE`): ids valid and unique.
    pub fn new(entries: impl IntoIterator<Item = RegistryEntry>) -> Self {
        FixedRegistry {
            entries: entries.into_iter().collect(),
        }
    }
}

impl AgentRegistry for FixedRegistry {
    async fn list(&self) -> AgentListing {
        AgentListing {
            entries: self.entries.to_vec(),
            sources: vec![SourceStatus::ok(STATIC_SOURCE)],
        }
    }

    async fn get(&self, id: &AgentId) -> Result<Option<RegistryEntry>, RegistryError> {
        Ok(self.entries.iter().find(|e| e.id() == id).cloned())
    }
}

/// Two registries read as one: `first`'s agents, then `second`'s. When both list an id, `first`
/// wins and the other is skipped (a warning in the log, once per read). The sources of both are
/// reported, in the same order.
#[derive(Debug, Clone)]
pub struct CompositeRegistry<A, B> {
    first: A,
    second: B,
}

impl<A, B> CompositeRegistry<A, B> {
    /// `first` has precedence.
    pub fn new(first: A, second: B) -> Self {
        CompositeRegistry { first, second }
    }
}

impl<A: AgentRegistry, B: AgentRegistry> AgentRegistry for CompositeRegistry<A, B> {
    async fn list(&self) -> AgentListing {
        let (first, second) = futures::join!(self.first.list(), self.second.list());
        let mut seen: BTreeSet<AgentId> = BTreeSet::new();
        let mut skipped: Vec<AgentId> = Vec::new();
        let mut entries = Vec::with_capacity(first.entries.len() + second.entries.len());
        for entry in first.entries.into_iter().chain(second.entries) {
            if seen.insert(entry.id().clone()) {
                entries.push(entry);
            } else {
                skipped.push(entry.id().clone());
            }
        }
        if !skipped.is_empty() {
            tracing::warn!(
                ids = ?skipped,
                "an agent id is listed by two sources; the first source's entry is used and the other skipped"
            );
        }
        let mut sources = first.sources;
        sources.extend(second.sources);
        AgentListing { entries, sources }
    }

    async fn get(&self, id: &AgentId) -> Result<Option<RegistryEntry>, RegistryError> {
        // `first` has precedence, so a `first` that cannot answer leaves the question open even
        // if `second` lists the id: it might not be the entry that wins.
        if let Some(entry) = self.first.get(id).await? {
            return Ok(Some(entry));
        }
        self.second.get(id).await
    }
}

/// No source at all: `None` lists nothing and knows no agent. It lets a composition root hold a
/// registry that a feature or a setting may leave out.
impl<R: AgentRegistry> AgentRegistry for Option<R> {
    async fn list(&self) -> AgentListing {
        match self {
            Some(registry) => registry.list().await,
            None => AgentListing::default(),
        }
    }

    async fn get(&self, id: &AgentId) -> Result<Option<RegistryEntry>, RegistryError> {
        match self {
            Some(registry) => registry.get(id).await,
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, origin: AgentSource) -> RegistryEntry {
        RegistryEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("http://{id}.test/.well-known/agent-card.json"),
                None,
            ),
            name: name.to_owned(),
            tags: Vec::new(),
            origin,
        }
    }

    fn ids(listing: &AgentListing) -> Vec<&str> {
        listing.entries.iter().map(|e| e.id().as_str()).collect()
    }

    #[tokio::test]
    async fn a_fixed_registry_lists_its_entries_in_order_and_is_never_unavailable() {
        let registry = FixedRegistry::new([
            entry("coder", "Coder", AgentSource::Static),
            entry("plain", "Plain", AgentSource::Static),
        ]);
        let listing = registry.list().await;
        assert_eq!(ids(&listing), ["coder", "plain"]);
        assert_eq!(listing.sources, [SourceStatus::ok("static")]);
        assert!(listing.is_complete());
        assert_eq!(listing.unavailable().count(), 0);
        let found = registry.get(&AgentId::new("plain")).await.unwrap().unwrap();
        assert_eq!(found.name, "Plain");
        assert!(
            registry
                .get(&AgentId::new("nobody"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_empty_fixed_registry_is_a_source_with_no_agents() {
        let listing = FixedRegistry::default().list().await;
        assert!(listing.entries.is_empty());
        assert_eq!(listing.sources, [SourceStatus::ok("static")]);
    }

    #[tokio::test]
    async fn no_source_at_all_lists_nothing_and_knows_nobody() {
        let none: Option<FixedRegistry> = None;
        assert_eq!(none.list().await, AgentListing::default());
        assert!(none.get(&AgentId::new("coder")).await.unwrap().is_none());
        let some = Some(FixedRegistry::new([entry(
            "coder",
            "Coder",
            AgentSource::Static,
        )]));
        assert_eq!(ids(&some.list().await), ["coder"]);
        assert!(some.get(&AgentId::new("coder")).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_composite_lists_the_first_then_the_second_and_reports_both_sources() {
        let registry = CompositeRegistry::new(
            FixedRegistry::new([entry("coder", "Coder", AgentSource::Static)]),
            FixedRegistry::new([
                entry("helper", "Helper", AgentSource::Registry),
                entry("writer", "Writer", AgentSource::Registry),
            ]),
        );
        let listing = registry.list().await;
        assert_eq!(ids(&listing), ["coder", "helper", "writer"]);
        assert_eq!(listing.sources.len(), 2);
        let found = registry
            .get(&AgentId::new("writer"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.origin, AgentSource::Registry);
    }

    #[tokio::test]
    async fn on_an_id_both_list_the_first_wins() {
        let registry = CompositeRegistry::new(
            FixedRegistry::new([entry("coder", "Static coder", AgentSource::Static)]),
            FixedRegistry::new([
                entry("coder", "Platform coder", AgentSource::Registry),
                entry("helper", "Helper", AgentSource::Registry),
            ]),
        );
        let listing = registry.list().await;
        assert_eq!(ids(&listing), ["coder", "helper"]);
        assert_eq!(listing.entries[0].name, "Static coder");
        assert_eq!(listing.entries[0].origin, AgentSource::Static);
        let found = registry.get(&AgentId::new("coder")).await.unwrap().unwrap();
        assert_eq!(found.name, "Static coder");
    }

    #[tokio::test]
    async fn a_composite_with_no_second_source_is_the_first() {
        let registry = CompositeRegistry::new(
            FixedRegistry::new([entry("coder", "Coder", AgentSource::Static)]),
            None::<FixedRegistry>,
        );
        let listing = registry.list().await;
        assert_eq!(ids(&listing), ["coder"]);
        assert_eq!(listing.sources, [SourceStatus::ok("static")]);
        assert!(registry.get(&AgentId::new("x")).await.unwrap().is_none());
    }

    #[test]
    fn an_unavailable_registry_is_transient_and_says_which_one() {
        let e = RegistryError::unavailable("platform", "the registry could not be reached")
            .with_source(std::io::Error::other("connection refused"));
        assert_eq!(e.class(), ErrorClass::Transient);
        assert!(e.is_retryable());
        assert_eq!(
            e.to_string(),
            "agent registry platform unavailable: the registry could not be reached"
        );
        assert_eq!(
            orch_core::report(&e),
            "agent registry platform unavailable: the registry could not be reached: connection refused"
        );
    }
}
