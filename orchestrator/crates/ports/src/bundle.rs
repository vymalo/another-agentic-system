use crate::{AgentClient, ChatModel, Clock, IdGen, ThreadStore, Wakeup};

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
}

/// The plain struct implementation of [`Ports`].
#[derive(Debug, Clone)]
pub struct PortSet<S, W, A, C, I, M> {
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
}

impl<S, W, A, C, I, M> Ports for PortSet<S, W, A, C, I, M>
where
    S: ThreadStore,
    W: Wakeup,
    A: AgentClient,
    C: Clock,
    I: IdGen,
    M: ChatModel,
{
    type Store = S;
    type Wakeup = W;
    type Agents = A;
    type Clock = C;
    type Ids = I;
    type Model = M;

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
}
