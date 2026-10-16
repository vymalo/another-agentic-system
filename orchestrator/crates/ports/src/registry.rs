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
//!
//! An entry says **whom it is for** ([`RegistryEntry::audience`], [`AgentAudience`], ADR 0045): the
//! deployment's own agents have none and `auth.roles` alone decides, and an agent a registry
//! lists carries the audience of its item, which fails closed.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use orch_core::{AgentId, AgentSource, BoxError, Classify, ErrorClass};

use crate::AgentEndpoint;

/// The most distinct values an audience may hold (`agent-registry/v1`, section *Audience*).
pub const MAX_AUDIENCE_VALUES: usize = 32;
/// The most bytes of one audience value.
pub const MAX_AUDIENCE_VALUE_BYTES: usize = 64;
/// The audience value that admits everybody. It is only allowed alone.
pub const AUDIENCE_EVERYONE: &str = "*";

/// Whom an agent is for (ADR 0045, decision 1): the `audience` of a registry item, which the
/// application reads against the roles claim of the person who asks.
///
/// Four cases that are never confused, because a bare list of texts could not tell them apart
/// (an empty one would have to mean either "nobody" or "everybody"):
///
/// | Variant | Whose agent | Who may use it |
/// |---|---|---|
/// | [`Static`](Self::Static) | the deployment's own (`AGENTS_FILE`) | whoever `auth.roles` lets |
/// | [`AdminsOnly`](Self::AdminsOnly) | a registry's, with *none* | people whose roles hold `admin` |
/// | [`Everyone`](Self::Everyone) | a registry's, `["*"]` | whoever `auth.roles` lets |
/// | [`Roles`](Self::Roles) | a registry's, with values | people whose roles claim holds one, and administrators |
///
/// Whoever builds one from the contract's text uses [`AgentAudience::from_values`], which is the
/// contract's *Shape* and *Malformed* as a function: a value outside the grammar, more than
/// [`MAX_AUDIENCE_VALUES`] distinct values, or `"*"` beside another value is *none*, so
/// the result is [`AdminsOnly`](Self::AdminsOnly), never a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAudience {
    /// An agent of the deployment's own agents file: it has no audience, and `auth.roles` alone
    /// decides who may use it (ADR 0045, decision 1.6). Only the static source builds this one.
    Static,
    /// An agent a registry lists with no audience: the item has no `audience`, has `[]`, or has one
    /// the contract calls malformed. It is for people whose roles hold `admin`, and nobody else
    /// (fail closed).
    AdminsOnly,
    /// `["*"]`: everybody whom `auth.roles` lets use the agent.
    Everyone,
    /// These values: the people whose roles claim holds one of them (byte for byte), and the
    /// administrators.
    Roles(AudienceRoles),
}

/// A set of 1 to [`MAX_AUDIENCE_VALUES`] role names that a registry item names as its audience:
/// each is 1 to [`MAX_AUDIENCE_VALUE_BYTES`] bytes of `0x21` to `0x7E`, and none is `"*"`. It is
/// built only by [`AgentAudience::from_values`], so a value of this type is always well formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceRoles(BTreeSet<String>);

impl AudienceRoles {
    /// Whether `role` is one of the values, byte for byte: no trimming, no case folding.
    pub fn contains(&self, role: &str) -> bool {
        self.0.contains(role)
    }

    /// The values, each once, in byte order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl AgentAudience {
    /// The audience a registry item's `audience` array says, read as the contract's *Shape* and
    /// *Malformed* say (`another-agentic-platform`, `docs/extensions/agent-registry-v1.md`):
    ///
    /// - the values are a **set**: a repeat counts once, and the order means nothing;
    /// - a value is 1 to 64 bytes, every byte in `0x21` to `0x7E`;
    /// - at most 32 **distinct** values;
    /// - `"*"` is only allowed alone: `["*", "*"]` is `["*"]`, and `"*"` beside another value is
    ///   malformed;
    /// - no values at all (an empty array, or no `audience` member) is *none*, and so is anything
    ///   malformed: [`AgentAudience::AdminsOnly`].
    ///
    /// A caller that read a member that is not an array of texts has nothing to pass: that is
    /// malformed too, and it builds [`AgentAudience::AdminsOnly`] itself.
    pub fn from_values<I, S>(values: I) -> AgentAudience
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for value in values {
            let value = value.as_ref();
            if !is_audience_value(value) {
                return AgentAudience::AdminsOnly;
            }
            if !set.contains(value) {
                if set.len() == MAX_AUDIENCE_VALUES {
                    return AgentAudience::AdminsOnly;
                }
                set.insert(value.to_owned());
            }
        }
        match (set.len(), set.contains(AUDIENCE_EVERYONE)) {
            (0, _) => AgentAudience::AdminsOnly,
            (1, true) => AgentAudience::Everyone,
            (_, true) => AgentAudience::AdminsOnly,
            (_, false) => AgentAudience::Roles(AudienceRoles(set)),
        }
    }
}

/// Whether `value` is a well-formed audience value: 1 to 64 bytes, each in `0x21` to `0x7E`.
/// Bytes, not characters: a non-ASCII character has bytes above `0x7E`.
fn is_audience_value(value: &str) -> bool {
    (1..=MAX_AUDIENCE_VALUE_BYTES).contains(&value.len())
        && value.bytes().all(|b| (0x21..=0x7E).contains(&b))
}

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
    /// Whom it is for (ADR 0045). [`AgentAudience::Static`] for an agent of the deployment's own
    /// list and for no other; a registry's agent always has one of the other three.
    pub audience: AgentAudience,
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
            audience: match origin {
                AgentSource::Static => AgentAudience::Static,
                AgentSource::Registry => AgentAudience::Everyone,
            },
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

    // ---- the audience of an item (ADR 0045; the platform's agent-registry/v1, section Audience) ----

    fn audience<const N: usize>(values: [&str; N]) -> AgentAudience {
        AgentAudience::from_values(values)
    }

    fn roles_of(audience: &AgentAudience) -> Vec<&str> {
        match audience {
            AgentAudience::Roles(roles) => roles.iter().collect(),
            other => panic!("not a set of values: {other:?}"),
        }
    }

    #[test]
    fn no_values_is_none_and_none_is_for_administrators_only() {
        assert_eq!(audience([]), AgentAudience::AdminsOnly);
        assert_eq!(
            AgentAudience::from_values(Vec::<String>::new()),
            AgentAudience::AdminsOnly
        );
    }

    #[test]
    fn a_star_alone_is_everyone_and_a_repeat_of_it_is_the_same() {
        assert_eq!(audience(["*"]), AgentAudience::Everyone);
        assert_eq!(audience(["*", "*"]), AgentAudience::Everyone);
        assert_eq!(audience(["*", "*", "*"]), AgentAudience::Everyone);
    }

    #[test]
    fn a_star_beside_another_value_is_malformed_whatever_the_order() {
        assert_eq!(audience(["*", "x"]), AgentAudience::AdminsOnly);
        assert_eq!(audience(["x", "*"]), AgentAudience::AdminsOnly);
        assert_eq!(audience(["x", "*", "x"]), AgentAudience::AdminsOnly);
        assert_eq!(audience(["*", "**"]), AgentAudience::AdminsOnly);
    }

    #[test]
    fn the_values_are_a_set_a_repeat_counts_once_and_the_order_means_nothing() {
        let a = audience(["agent.use:b", "agent.use:a", "agent.use:b"]);
        let b = audience(["agent.use:a", "agent.use:b"]);
        assert_eq!(a, b);
        assert_eq!(roles_of(&a), ["agent.use:a", "agent.use:b"]);
    }

    #[test]
    fn values_are_compared_byte_for_byte_with_no_folding_or_trimming() {
        let AgentAudience::Roles(roles) = audience(["Agent.Use:A"]) else {
            panic!("a set of one value");
        };
        assert!(roles.contains("Agent.Use:A"));
        assert!(!roles.contains("agent.use:a"), "case is kept");
        assert!(!roles.contains("Agent.Use:A "), "nothing is trimmed");
        assert!(!roles.contains(""), "the empty text is no value");
    }

    #[test]
    fn a_value_is_one_to_sixty_four_bytes() {
        assert_eq!(
            audience(["x".repeat(MAX_AUDIENCE_VALUE_BYTES).as_str()]),
            AgentAudience::Roles(AudienceRoles(BTreeSet::from(["x".repeat(64)])))
        );
        assert_eq!(
            audience(["x".repeat(MAX_AUDIENCE_VALUE_BYTES + 1).as_str()]),
            AgentAudience::AdminsOnly
        );
        assert_eq!(audience([""]), AgentAudience::AdminsOnly);
        // the whole item is malformed for one bad value, the good ones notwithstanding
        assert_eq!(audience(["good", ""]), AgentAudience::AdminsOnly);
    }

    #[test]
    fn a_byte_is_between_0x21_and_0x7e() {
        // the edges that are in
        assert_eq!(roles_of(&audience(["!"])), ["!"]);
        assert_eq!(roles_of(&audience(["~"])), ["~"]);
        // the edges that are out: space (0x20), DEL (0x7F), controls, the line ends, non-ASCII
        for bad in [
            " ",
            "a b",
            " a",
            "a ",
            "\u{7f}",
            "a\u{7f}",
            "\t",
            "a\n",
            "\n",
            "a\r",
            "\0",
            "é",
            "agent.use:é",
            "\u{a0}",
            "\u{3000}",
            "日本",
        ] {
            assert_eq!(audience([bad]), AgentAudience::AdminsOnly, "{bad:?}");
        }
    }

    #[test]
    fn thirty_two_distinct_values_are_allowed_and_thirty_three_are_not() {
        let many = |n: usize| (0..n).map(|i| format!("role-{i:02}")).collect::<Vec<_>>();
        assert_eq!(roles_of(&AgentAudience::from_values(many(32))).len(), 32);
        assert_eq!(
            AgentAudience::from_values(many(33)),
            AgentAudience::AdminsOnly
        );
        // distinct, not listed: any number of repeats of 32 is 32
        let mut repeated = many(32);
        repeated.extend(many(32));
        repeated.extend(many(32));
        assert_eq!(roles_of(&AgentAudience::from_values(repeated)).len(), 32);
        // a thirty-third that is a repeat of "*" is not distinct either: *alone* needs one value
        let mut with_star = many(31);
        with_star.push("*".to_owned());
        assert_eq!(
            AgentAudience::from_values(with_star),
            AgentAudience::AdminsOnly
        );
    }

    mod audience_properties {
        use proptest::prelude::*;

        use super::*;

        /// Any text, and the ones near the grammar's edges.
        fn text() -> impl Strategy<Value = String> {
            prop_oneof![
                "[!-~]{1,64}",
                "[!-~]{0,70}",
                ".{0,70}",
                Just("*".to_owned()),
                Just(String::new()),
                "[ -\u{7f}]{1,8}",
            ]
        }

        /// The contract's rule for one value, written out again by hand.
        fn well_formed(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|b| matches!(b, 0x21..=0x7E))
        }

        proptest! {
            #[test]
            fn the_result_is_what_the_contracts_rules_say(values in prop::collection::vec(text(), 0..40)) {
                let distinct: BTreeSet<&str> = values.iter().map(String::as_str).collect();
                let result = AgentAudience::from_values(&values);
                let ok = distinct.iter().all(|v| well_formed(v)) && distinct.len() <= 32;
                match result {
                    AgentAudience::Static => prop_assert!(false, "a registry never builds a static audience"),
                    AgentAudience::AdminsOnly => prop_assert!(
                        !ok || distinct.is_empty() || (distinct.contains("*") && distinct.len() > 1),
                        "{distinct:?}"
                    ),
                    AgentAudience::Everyone => prop_assert!(ok && distinct == BTreeSet::from(["*"])),
                    AgentAudience::Roles(roles) => {
                        prop_assert!(ok && !distinct.contains("*") && !distinct.is_empty());
                        prop_assert_eq!(roles.iter().collect::<BTreeSet<_>>(), distinct);
                    }
                }
            }

            #[test]
            fn order_and_repeats_change_nothing(
                values in prop::collection::vec(text(), 0..20),
                again in prop::collection::vec(any::<prop::sample::Index>(), 0..20),
            ) {
                let base = AgentAudience::from_values(&values);
                let mut reversed: Vec<String> = values.iter().rev().cloned().collect();
                prop_assert_eq!(&AgentAudience::from_values(&reversed), &base);
                for i in again {
                    if !values.is_empty() {
                        reversed.push(values[i.index(values.len())].clone());
                    }
                }
                prop_assert_eq!(&AgentAudience::from_values(&reversed), &base);
            }

            #[test]
            fn a_set_of_values_never_holds_a_star_or_a_value_the_grammar_refuses(
                values in prop::collection::vec(text(), 0..40)
            ) {
                if let AgentAudience::Roles(roles) = AgentAudience::from_values(&values) {
                    let held: Vec<&str> = roles.iter().collect();
                    prop_assert!(!held.is_empty() && held.len() <= 32);
                    prop_assert!(held.iter().all(|v| well_formed(v) && *v != "*"));
                    prop_assert!(held.windows(2).all(|w| w[0] < w[1]), "byte order, each once");
                }
            }
        }
    }
}
