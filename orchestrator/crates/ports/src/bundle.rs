use crate::{
    AgentClient, AgentRegistry, ArtifactStore, Authenticator, ChatModel, Clock, CredentialBroker,
    EgressPolicy, FixedRegistry, IdGen, KeepsCredentials, NoArtifacts, NoBroker, NoUserToolServers,
    RefuseAll, ThreadStore, UserToolServerStore, Wakeup,
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
/// added later fails to compile wherever one is matched. It is not chosen separately from the ports:
/// each constructor of [`UserTools`] fixes it, from the composition root's reading of
/// `userToolServers.enabled` and the presence of the `secrets` section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserToolsMode {
    /// `userToolServers.enabled` is false or absent: **nothing is built**. The definitions store and
    /// the broker refuse every call, so a route that forgets to ask cannot write.
    Off,
    /// Enabled, with no `secrets` section: definitions are kept, and **only servers that want no
    /// authentication** (`none`) can be added. The broker refuses every call, so a credential has
    /// nowhere to be kept and nothing tries to (ADR 0046, decision 3, "fail closed").
    ///
    /// A definition may still hold a credential reference here: the operator took the `secrets`
    /// section away after credentials were stored (a lost key, a response to a compromise). Such a
    /// server is unavailable, and **its owner can still delete it**, which is why a delete in this mode
    /// does not call `revoke` (see [`UserToolPorts`]).
    WithoutCredentials,
    /// Enabled, with a broker that keeps credentials.
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
/// ADR 0009): the definitions ([`UserToolServerStore`]) and the broker ([`CredentialBroker`]) that
/// lends a credential for one call, with the [mode](UserToolsMode) the deployment chose and the
/// [`EgressPolicy`] it configured. The application and the API are written against this and never
/// name an adapter.
///
/// **There is no secret store in it.** The broker holds its own handle on the sealed store and is the
/// only one that reads or writes secret material; a handler that is given the bundle cannot read a
/// secret through it, and none ever needs to (the API's work is the definitions and the broker's
/// five operations).
///
/// # What a caller does
///
/// These are the rules the endpoints (slice 4b) and the relay's lease (slice 5) follow.
///
/// 1. **Ask [`mode`](Self::mode) first.** In [`Off`](UserToolsMode::Off) the ports refuse every call,
///    reads included, as `Unavailable` of class `Transient`: a caller that lists without asking
///    would fail every request of a deployment that has the feature off, which is every deployment by
///    default. `Off` is a 404 or a feature that is absent, decided from the mode and never from a
///    store error.
/// 2. **A server that wants a credential needs [`keeps_credentials`](UserToolsMode::keeps_credentials).**
///    One that wants none ([`UserAuth::None`](crate::UserAuth::None)) is added in
///    `WithoutCredentials` and `Full` alike.
/// 3. **Deleting a server.** In [`Full`](UserToolsMode::Full) it is **revoke, then delete**: the
///    broker reads the definition to know what to revoke, so it runs first, and a server with no
///    credential (`needs_credential() == false`) skips it. In
///    [`WithoutCredentials`](UserToolsMode::WithoutCredentials) it does **not** call
///    [`revoke`](CredentialBroker::revoke): [`NoBroker`] refuses it, and a person whose operator
///    removed the `secrets` section must still be able to delete a server and free the place it
///    holds in their eight. The definition is deleted; the sealed row, if there is one, stays in
///    `user_secrets`, unreachable without a key. It is never lent: the broker binds a credential to
///    the definition's URL and to the id made again, and an id is random (`u-` and ten characters), so
///    a later definition does not meet an orphan by chance. A sweep of such rows is a later job.
pub trait UserToolPorts: Send + Sync + 'static {
    /// Where the definitions are kept.
    type Servers: UserToolServerStore;
    /// The credential broker.
    type Broker: CredentialBroker;

    /// What the deployment made of the feature. Derived from what was built, never set beside it.
    fn mode(&self) -> UserToolsMode;
    /// The guard for the URL a person types, from `userToolServers.egress`: the same policy the
    /// broker was composed with, so a URL the API accepts is a URL the broker may fetch.
    fn egress(&self) -> &EgressPolicy;
    /// The definitions of people's servers.
    ///
    /// **Check [`mode`](Self::mode) before calling.** When the feature is [`Off`](UserToolsMode::Off)
    /// this is [`NoUserToolServers`], which refuses reads as well as writes (`Unavailable`, class
    /// `Transient`), so that a route that forgets the switch cannot write; one that forgets it and
    /// reads fails every request in a default deployment.
    fn servers(&self) -> &Self::Servers;
    /// The credential broker. In every mode but [`Full`](UserToolsMode::Full) it refuses every call
    /// ([`NoBroker`]); see the rules above for what a caller then skips.
    fn broker(&self) -> &Self::Broker;
}

/// The plain struct implementation of [`UserToolPorts`]. Its three constructors are the only ways to
/// make one, and each fixes the [mode](UserToolsMode) together with the ports it means, so a bundle
/// cannot say one thing and hold another: [`off`](Self::off) holds the refusing ports,
/// [`without_credentials`](Self::without_credentials) holds the given definitions and a [`NoBroker`],
/// and [`full`](Self::full) asks for a broker that [keeps credentials](KeepsCredentials), which
/// [`NoBroker`] does not.
///
/// The two type parameters are the composition root's own (in the binary, enums over the adapters it
/// was built with, by static dispatch); a refusing port is put in with `From`.
#[derive(Debug, Clone)]
pub struct UserTools<D, B> {
    mode: UserToolsMode,
    egress: EgressPolicy,
    servers: D,
    broker: B,
}

impl<D, B> UserTools<D, B>
where
    D: UserToolServerStore,
    B: CredentialBroker,
{
    /// The bundle of a deployment where the feature is off: [`UserToolsMode::Off`], the default
    /// [`EgressPolicy`], and the refusing ports ([`NoUserToolServers`], [`NoBroker`]).
    pub fn off() -> Self
    where
        D: From<NoUserToolServers>,
        B: From<NoBroker>,
    {
        UserTools {
            mode: UserToolsMode::Off,
            egress: EgressPolicy::new(),
            servers: D::from(NoUserToolServers),
            broker: B::from(NoBroker),
        }
    }

    /// The bundle of a deployment that enabled the feature and has no `secrets` section:
    /// [`UserToolsMode::WithoutCredentials`], with the definitions `servers` keeps and the refusing
    /// [`NoBroker`].
    pub fn without_credentials(egress: EgressPolicy, servers: D) -> Self
    where
        B: From<NoBroker>,
    {
        UserTools {
            mode: UserToolsMode::WithoutCredentials,
            egress,
            servers,
            broker: B::from(NoBroker),
        }
    }

    /// The bundle of a deployment that enabled the feature and keeps credentials:
    /// [`UserToolsMode::Full`], over `broker`, which must be one that
    /// [keeps credentials](KeepsCredentials) (the held type may be a wider enum of the root's, which
    /// `From` puts it in).
    ///
    /// A broker that refuses every call is not accepted, so a bundle cannot claim to keep what it
    /// cannot:
    ///
    /// ```compile_fail
    /// use orch_ports::{EgressPolicy, NoBroker, NoUserToolServers, UserTools};
    /// let _ = UserTools::<NoUserToolServers, NoBroker>::full(
    ///     EgressPolicy::new(),
    ///     NoUserToolServers,
    ///     NoBroker,
    /// );
    /// ```
    pub fn full<K>(egress: EgressPolicy, servers: D, broker: K) -> Self
    where
        K: KeepsCredentials,
        B: From<K>,
    {
        UserTools {
            mode: UserToolsMode::Full,
            egress,
            servers,
            broker: B::from(broker),
        }
    }
}

/// People's own MCP servers are off: both ports refuse every call.
pub type NoUserTools = UserTools<NoUserToolServers, NoBroker>;

impl<D, B> UserToolPorts for UserTools<D, B>
where
    D: UserToolServerStore,
    B: CredentialBroker,
{
    type Servers = D;
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
