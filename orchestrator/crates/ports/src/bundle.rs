use crate::{
    AgentClient, AgentRegistry, ArtifactStore, Authenticator, ChatModel, Clock, CredentialBroker,
    EgressPolicy, FixedRegistry, IdGen, NoArtifacts, NoBroker, NoSecrets, NoUserToolServers,
    RefuseAll, SecretStore, ThreadStore, UserToolServerStore, Wakeup,
};

/// A static-dispatch bundle of every port (ADR 0009: composition happens at build time).
pub trait Ports: Send + Sync + 'static {
    /// The thread/event/outbox store.
    type Store: ThreadStore;
    /// The wakeup channel.
    type Wakeup: Wakeup;
    /// The delegated-agent client.
    type Agents: AgentClient;
    /// The clock.
    type Clock: Clock;
    /// The id generator.
    type Ids: IdGen;
    /// The language model (`NoModel` in a deployment without one).
    type Model: ChatModel;
    /// Who is calling: the authenticator of the HTTP edge (ADR 0033; `RefuseAll` in a process that
    /// serves no routes).
    type Auth: Authenticator;
    /// The agents that exist right now (ADR 0022).
    type Registry: AgentRegistry;
    /// The files agents hand over (`NoArtifacts` in a deployment without a store, ADR 0032).
    type Artifacts: ArtifactStore;
    /// The ports of people's own MCP servers, held together (`NoUserTools` in a deployment where
    /// they are off, ADR 0046).
    type UserTools: UserToolPorts;

    /// The store.
    fn store(&self) -> &Self::Store;
    /// The wakeup channel.
    fn wakeup(&self) -> &Self::Wakeup;
    /// The agent client.
    fn agents(&self) -> &Self::Agents;
    /// The clock.
    fn clock(&self) -> &Self::Clock;
    /// The id generator.
    fn ids(&self) -> &Self::Ids;
    /// The language model.
    fn model(&self) -> &Self::Model;
    /// The authenticator.
    fn auth(&self) -> &Self::Auth;
    /// The agent registry.
    fn registry(&self) -> &Self::Registry;
    /// The artifact store.
    fn artifacts(&self) -> &Self::Artifacts;
    /// The ports of people's own MCP servers.
    fn user_tools(&self) -> &Self::UserTools;
}

/// What a deployment has made of people's own MCP servers (ADR 0046): a closed enum, so a mode
/// added later fails to compile wherever one is matched. Decided by the composition root from
/// `userToolServers.enabled` and the presence of the `secrets` section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserToolsMode {
    /// `userToolServers.enabled` is false or absent: **nothing is built**. The definitions store,
    /// the secret store and the broker refuse every call, so a route that forgets to ask cannot
    /// write.
    Off,
    /// Enabled, with no `secrets` section: definitions are kept, and **only servers that want no
    /// authentication** (`none`) can be added. The secret store and the broker refuse every call, so
    /// a credential has nowhere to be kept and nothing tries to (ADR 0046, decision 3, "fail
    /// closed").
    WithoutCredentials,
    /// Enabled, with a secret store and a broker that keeps credentials.
    Full,
}

impl UserToolsMode {
    /// Whether people may have servers of their own: every mode but [`Off`](Self::Off).
    pub const fn is_enabled(self) -> bool {
        !matches!(self, UserToolsMode::Off)
    }

    /// Whether a credential can be kept (a bearer token, a key, an OAuth grant): only
    /// [`Full`](Self::Full).
    pub const fn keeps_credentials(self) -> bool {
        matches!(self, UserToolsMode::Full)
    }
}

/// The ports of people's own MCP servers, held together as the bundle holds the others (ADR 0046,
/// ADR 0009): the definitions ([`UserToolServerStore`]), the sealed material of their credentials
/// ([`SecretStore`]) and the broker ([`CredentialBroker`]) that is composed over those two and
/// lends a credential for one call, with the [mode](UserToolsMode) the deployment chose and the
/// [`EgressPolicy`] it configured. The application and the API are written against this and never
/// name an adapter.
///
/// In [`Off`](UserToolsMode::Off) all three refuse every call ([`NoUserToolServers`],
/// [`NoSecrets`], [`NoBroker`]); in [`WithoutCredentials`](UserToolsMode::WithoutCredentials) the
/// last two do.
pub trait UserToolPorts: Send + Sync + 'static {
    /// Where the definitions are kept.
    type Servers: UserToolServerStore;
    /// Where the sealed material is kept.
    type Secrets: SecretStore;
    /// The credential broker.
    type Broker: CredentialBroker;

    /// What the deployment made of the feature.
    fn mode(&self) -> UserToolsMode;
    /// The guard for the URL a person types, from `userToolServers.egress`: the same policy the
    /// broker was composed with, so a URL the API accepts is a URL the broker may fetch.
    fn egress(&self) -> &EgressPolicy;
    /// The definitions of people's servers.
    fn servers(&self) -> &Self::Servers;
    /// The secret store.
    fn secrets(&self) -> &Self::Secrets;
    /// The credential broker.
    fn broker(&self) -> &Self::Broker;
}

/// The plain struct implementation of [`UserToolPorts`]. The composition root chooses the three
/// ports and the mode together and builds one of these; nothing checks that they agree (a `Full` mode
/// over a [`NoBroker`] is a root that is wrong, and every call then refuses).
#[derive(Debug, Clone)]
pub struct UserTools<D, S, B> {
    mode: UserToolsMode,
    egress: EgressPolicy,
    servers: D,
    secrets: S,
    broker: B,
}

impl<D, S, B> UserTools<D, S, B>
where
    D: UserToolServerStore,
    S: SecretStore,
    B: CredentialBroker,
{
    /// The ports of people's servers, in `mode`, with the deployment's `egress`.
    pub fn new(
        mode: UserToolsMode,
        egress: EgressPolicy,
        servers: D,
        secrets: S,
        broker: B,
    ) -> Self {
        UserTools {
            mode,
            egress,
            servers,
            secrets,
            broker,
        }
    }
}

/// People's own MCP servers are off: all three ports refuse every call.
pub type NoUserTools = UserTools<NoUserToolServers, NoSecrets, NoBroker>;

impl NoUserTools {
    /// The bundle of a deployment where the feature is off ([`UserToolsMode::Off`], the default
    /// [`EgressPolicy`], and the three refusing ports).
    pub fn off() -> Self {
        UserTools::new(
            UserToolsMode::Off,
            EgressPolicy::new(),
            NoUserToolServers,
            NoSecrets,
            NoBroker,
        )
    }
}

impl<D, S, B> UserToolPorts for UserTools<D, S, B>
where
    D: UserToolServerStore,
    S: SecretStore,
    B: CredentialBroker,
{
    type Servers = D;
    type Secrets = S;
    type Broker = B;

    fn mode(&self) -> UserToolsMode {
        self.mode
    }
    fn egress(&self) -> &EgressPolicy {
        &self.egress
    }
    fn servers(&self) -> &D {
        &self.servers
    }
    fn secrets(&self) -> &S {
        &self.secrets
    }
    fn broker(&self) -> &B {
        &self.broker
    }
}

/// The plain struct implementation of [`Ports`]. The registry type defaults to the static list,
/// the authenticator to [`RefuseAll`] (which lets nobody in), the artifact store to none and
/// people's own MCP servers to off, so a `PortSet<S, W, A, C, I, M>` that never heard of
/// registries, authentication, files or those servers still names a complete bundle.
#[derive(Debug, Clone)]
pub struct PortSet<
    S,
    W,
    A,
    C,
    I,
    M,
    R = FixedRegistry,
    U = RefuseAll,
    X = NoArtifacts,
    T = NoUserTools,
> {
    /// The store.
    pub store: S,
    /// The wakeup channel.
    pub wakeup: W,
    /// The agent client.
    pub agents: A,
    /// The clock.
    pub clock: C,
    /// The id generator.
    pub ids: I,
    /// The language model.
    pub model: M,
    /// The authenticator.
    pub auth: U,
    /// The agent registry.
    pub registry: R,
    /// The artifact store.
    pub artifacts: X,
    /// The ports of people's own MCP servers.
    pub user_tools: T,
}

impl<S, W, A, C, I, M, R, U, X, T> Ports for PortSet<S, W, A, C, I, M, R, U, X, T>
where
    S: ThreadStore,
    W: Wakeup,
    A: AgentClient,
    C: Clock,
    I: IdGen,
    M: ChatModel,
    R: AgentRegistry,
    U: Authenticator,
    X: ArtifactStore,
    T: UserToolPorts,
{
    type Store = S;
    type Wakeup = W;
    type Agents = A;
    type Clock = C;
    type Ids = I;
    type Model = M;
    type Auth = U;
    type Registry = R;
    type Artifacts = X;
    type UserTools = T;

    fn store(&self) -> &S {
        &self.store
    }
    fn wakeup(&self) -> &W {
        &self.wakeup
    }
    fn agents(&self) -> &A {
        &self.agents
    }
    fn clock(&self) -> &C {
        &self.clock
    }
    fn ids(&self) -> &I {
        &self.ids
    }
    fn model(&self) -> &M {
        &self.model
    }
    fn auth(&self) -> &U {
        &self.auth
    }
    fn registry(&self) -> &R {
        &self.registry
    }
    fn artifacts(&self) -> &X {
        &self.artifacts
    }
    fn user_tools(&self) -> &T {
        &self.user_tools
    }
}
