use crate::{
    AgentClient, AgentRegistry, ArtifactStore, Authenticator, ChatModel, Clock, FixedRegistry,
    IdGen, NoArtifacts, RefuseAll, ThreadStore, Wakeup,
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
}

/// The plain struct implementation of [`Ports`]. The registry type defaults to the static list,
/// the authenticator to [`RefuseAll`] (which lets nobody in) and the artifact store to none, so a
/// `PortSet<S, W, A, C, I, M>` that never heard of registries, authentication or files still names
/// a complete bundle.
#[derive(Debug, Clone)]
pub struct PortSet<S, W, A, C, I, M, R = FixedRegistry, U = RefuseAll, X = NoArtifacts> {
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
}

impl<S, W, A, C, I, M, R, U, X> Ports for PortSet<S, W, A, C, I, M, R, U, X>
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
}
