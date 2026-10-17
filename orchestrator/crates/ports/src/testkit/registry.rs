//! `AgentRegistry` conformance cases: what the orchestrator relies on from any source of agents
//! (ADR 0022), stated once and run against the in-memory registry, against a composition of
//! registries and against the platform's registry over real HTTP.
//!
//! An implementation supplies a [`RegistryFixture`]: the registry, and a way to change what its
//! source lists and to take the source down. Each case is `async fn(fixture)` and gives up
//! after 10 seconds.
//!
//! The fixture's registry reads live: a case that changes the source and reads again expects to
//! see the change on the very next read, so an implementation that keeps a copy (the platform's,
//! for as long as the source's cache headers say) is given a source that says not to.
//!
//! The audience of an entry (ADR 0045) round-trips: what the fixture makes the source say is what
//! [`list`](AgentRegistry::list) and [`get`](AgentRegistry::get) give back, for each of the three
//! audiences a registry's agent has. An entry whose `origin` is [`AgentSource::Registry`] never
//! carries [`AgentAudience::Static`], which is the deployment's own list's alone.
//!
//! What is deliberately not asserted: the wording of a detail, and which `origin` an entry has
//! (only that one which says `Registry` has no `Static` audience).

use std::future::Future;
use std::time::Duration;

use orch_core::{AgentId, AgentSource, Classify, ErrorClass, is_valid_agent_id};

use crate::{AgentAudience, AgentEndpoint, AgentRegistry, RegistryEntry};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(10);

/// A registry under test, with a source behind it that the cases change.
pub trait RegistryFixture: Send + Sync + 'static {
    /// The registry under test.
    type Registry: AgentRegistry;

    /// The registry.
    fn registry(&self) -> &Self::Registry;

    /// Makes the source list `entry`, after the agents it lists already, from the next read on.
    /// The entry is made by [`entry`]: an A2A agent with a card URL, a name and no tags.
    fn add(&self, entry: RegistryEntry);

    /// Makes the source stop listing the agent `id`, from the next read on.
    fn remove(&self, id: &AgentId);

    /// Takes the source down (`true`) or brings it back (`false`). While it is down a read
    /// fails the way an unreachable server does.
    fn set_down(&self, down: bool);

    /// Makes the source carry something that is not an agent: whatever its format can hold that
    /// has an id which is not [`is_valid_agent_id`]. The registry must keep listing the rest.
    fn add_invalid(&self);
}

/// The entry the cases add: an A2A agent `id` named `name`, with the card URL
/// `http://<id>.test/.well-known/agent-card.json`, no bearer, no tags and an audience of
/// [`AgentAudience::Everyone`].
pub fn entry(id: &str, name: &str) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("http://{id}.test/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
        audience: AgentAudience::Everyone,
    }
}

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, case)
        .await
        .expect("the case did not finish within 10 s")
}

fn id(raw: &str) -> AgentId {
    AgentId::new(raw)
}

/// An entry a registry lists never carries [`AgentAudience::Static`] (ADR 0045): the audience
/// that lets in whoever `auth.roles` lets is the deployment's own list's, and a registry agent
/// that had it would be open to every role that names its id.
fn assert_audience_is_not_static(entry: &RegistryEntry) {
    assert!(
        entry.origin != AgentSource::Registry || entry.audience != AgentAudience::Static,
        "the entry {} comes from a registry and carries the static audience",
        entry.id()
    );
}

async fn ids<F: RegistryFixture>(f: &F) -> Vec<String> {
    f.registry()
        .list()
        .await
        .entries
        .iter()
        .map(|e| e.id().to_string())
        .collect()
}

/// The agents come in the order the source lists them, with their names and card URLs, and a
/// registry that answered says so.
pub async fn lists_in_order<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        f.add(entry("beta", "Beta"));
        f.add(entry("gamma", "Gamma"));
        let listing = f.registry().list().await;
        let got: Vec<_> = listing
            .entries
            .iter()
            .map(|e| (e.id().as_str(), e.name.as_str()))
            .collect();
        assert_eq!(
            got,
            [("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")]
        );
        let want = entry("beta", "Beta");
        assert_eq!(
            listing.entries[1].endpoint.transport, want.endpoint.transport,
            "the card URL is the source's"
        );
        assert!(!listing.sources.is_empty(), "a registry names its sources");
        assert!(
            listing
                .sources
                .iter()
                .all(|s| s.available && !s.name.is_empty()),
            "every source answered and is named: {:?}",
            listing.sources
        );
        assert!(listing.is_complete());
    })
    .await;
}

/// `get` finds a listed agent by id, as `list` shows it.
pub async fn get_finds_by_id<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        f.add(entry("beta", "Beta"));
        let found = f
            .registry()
            .get(&id("beta"))
            .await
            .expect("the source answers")
            .expect("beta is listed");
        assert_eq!(found.id(), &id("beta"));
        assert_eq!(found.name, "Beta");
        let listed = f.registry().list().await;
        assert_eq!(
            listed.entries.iter().find(|e| e.id() == &id("beta")),
            Some(&found),
            "get and list agree"
        );
    })
    .await;
}

/// An id nobody lists is `Ok(None)` when every source answered.
pub async fn unknown_id_is_none<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        assert!(
            f.registry()
                .get(&id("nobody"))
                .await
                .expect("the source answers")
                .is_none()
        );
    })
    .await;
}

/// The registry is read live: an agent added shows on the next read, and one removed is gone
/// from the next.
pub async fn a_change_shows_on_the_next_read<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        assert_eq!(ids(&f).await, ["alpha"]);
        f.add(entry("beta", "Beta"));
        assert_eq!(ids(&f).await, ["alpha", "beta"]);
        assert!(f.registry().get(&id("beta")).await.unwrap().is_some());
        f.remove(&id("alpha"));
        assert_eq!(ids(&f).await, ["beta"]);
        assert!(
            f.registry().get(&id("alpha")).await.unwrap().is_none(),
            "a removed agent is not selectable"
        );
    })
    .await;
}

/// A source that is down lists none of its agents, however recently it listed some, and says
/// that it could not be read, in a way a person can read (no URL).
pub async fn down_lists_nothing_and_says_so<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        f.add(entry("beta", "Beta"));
        assert_eq!(ids(&f).await, ["alpha", "beta"]);
        f.set_down(true);
        let listing = f.registry().list().await;
        assert!(
            listing.entries.is_empty(),
            "a source that cannot be read lists nothing, not a stale copy: {:?}",
            listing.entries
        );
        assert!(!listing.is_complete());
        let down: Vec<_> = listing.unavailable().collect();
        assert!(!down.is_empty(), "the listing says which source is down");
        for source in down {
            let detail = source
                .detail
                .as_deref()
                .expect("a source that is down says why");
            assert!(!detail.is_empty());
            assert!(
                !detail.contains("://"),
                "the detail is fit for a person, not a URL: {detail}"
            );
        }
    })
    .await;
}

/// While the source is down, "is there such an agent?" is "cannot tell", never "no": for an
/// agent it listed a moment ago and for one nobody lists.
pub async fn get_while_down_is_unavailable_not_none<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        assert!(f.registry().get(&id("alpha")).await.unwrap().is_some());
        f.set_down(true);
        for who in ["alpha", "nobody"] {
            let err = f
                .registry()
                .get(&id(who))
                .await
                .expect_err("a source that is down cannot tell");
            assert_eq!(err.class(), ErrorClass::Transient, "{who}");
            assert!(err.is_retryable(), "{who}");
        }
    })
    .await;
}

/// A source that comes back is read again: its agents are listed and found, and it says it
/// answered.
pub async fn recovers_when_up<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        f.set_down(true);
        assert!(f.registry().list().await.entries.is_empty());
        f.set_down(false);
        let listing = f.registry().list().await;
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|e| e.id().as_str())
                .collect::<Vec<_>>(),
            ["alpha"]
        );
        assert!(listing.is_complete(), "{:?}", listing.sources);
        assert!(f.registry().get(&id("alpha")).await.unwrap().is_some());
    })
    .await;
}

/// The audience of an agent is the source's, whole: `["*"]`, a set of values and none (an agent the
/// source gives no audience), by `list` and by `get`, and a change of it shows on the next read.
/// No entry the registry lists carries [`AgentAudience::Static`], whatever the source says.
pub async fn audience_round_trips<F: RegistryFixture>(f: F) {
    within(async {
        let with = |id: &str, audience: AgentAudience| RegistryEntry {
            audience,
            ..entry(id, id)
        };
        let values = AgentAudience::from_values(["agent.use:beta", "agent.use:alpha"]);
        assert!(matches!(values, AgentAudience::Roles(_)));
        f.add(with("everyone", AgentAudience::Everyone));
        f.add(with("some", values.clone()));
        f.add(with("none", AgentAudience::AdminsOnly));
        let listing = f.registry().list().await;
        listing
            .entries
            .iter()
            .for_each(assert_audience_is_not_static);
        let audience_of = |id: &str| {
            listing
                .entries
                .iter()
                .find(|e| e.id().as_str() == id)
                .map(|e| e.audience.clone())
        };
        assert_eq!(audience_of("everyone"), Some(AgentAudience::Everyone));
        assert_eq!(audience_of("some"), Some(values.clone()));
        assert_eq!(audience_of("none"), Some(AgentAudience::AdminsOnly));
        for (id, want) in [
            ("everyone", AgentAudience::Everyone),
            ("some", values.clone()),
            ("none", AgentAudience::AdminsOnly),
        ] {
            let found = f.registry().get(&self::id(id)).await.unwrap().unwrap();
            assert_eq!(found.audience, want, "get {id}");
            assert_audience_is_not_static(&found);
        }

        // a change of it shows on the next read, like any other change (the source lists an id
        // once, so the change is the item taken out and put back)
        f.remove(&id("none"));
        f.add(with("none", AgentAudience::Everyone));
        f.remove(&id("some"));
        f.add(with("some", AgentAudience::AdminsOnly));
        let found = f.registry().get(&id("none")).await.unwrap().unwrap();
        assert_eq!(found.audience, AgentAudience::Everyone);
        assert_audience_is_not_static(&found);
        let found = f.registry().get(&id("some")).await.unwrap().unwrap();
        assert_eq!(found.audience, AgentAudience::AdminsOnly);
        assert_audience_is_not_static(&found);
    })
    .await;
}

/// Something that is not an agent in the source is never listed, and the agents around it are.
pub async fn invalid_entries_never_listed<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        f.add_invalid();
        f.add(entry("beta", "Beta"));
        let listing = f.registry().list().await;
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|e| e.id().as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert!(
            listing
                .entries
                .iter()
                .all(|e| is_valid_agent_id(e.id().as_str())),
            "every listed id is an agent id"
        );
        assert!(
            listing.is_complete(),
            "an invalid entry is skipped; it does not make the source unavailable"
        );
    })
    .await;
}

/// An id is listed once, even when the source says it twice.
pub async fn an_id_is_listed_once<F: RegistryFixture>(f: F) {
    within(async {
        f.add(entry("alpha", "Alpha"));
        f.add(entry("alpha", "Alpha again"));
        f.add(entry("beta", "Beta"));
        let got = ids(&f).await;
        assert_eq!(
            got.iter().filter(|id| id.as_str() == "alpha").count(),
            1,
            "{got:?}"
        );
        assert!(got.contains(&"beta".to_owned()));
        assert!(f.registry().get(&id("alpha")).await.unwrap().is_some());
    })
    .await;
}
