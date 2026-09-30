//! Local agents (`transport: local`, ADR 0015): the one place the binary knows about
//! `orch-agent-adam`.
//!
//! Without the Cargo feature `agent-local` this module is a shim: [`Agents`] is the A2A client
//! alone, [`Local`] cannot be built, and nothing here links adam-rs's runtime. With it, the
//! binary hosts the agents `AGENTS_FILE` lists in this process:
//!
//! * [`Local::start`] builds a pool of its own on `DATABASE_URL`, sized for the agents, and
//!   creates the journal's tables (prefix `orch_agent_`). Both migrations, the orchestrator's
//!   and this one, are idempotent under their own advisory locks, so every role and every
//!   replica runs them at boot;
//! * [`compose`] routes each endpoint to its client by transport;
//! * [`Local::register`] runs the agents' worker beside the dispatcher, in the roles that run
//!   workers. The control plane steps nothing and runs no notifier: it only reads cards.

#[cfg(feature = "agent-local")]
pub use enabled::{Agents, Local, compose, is_unavailable};

#[cfg(not(feature = "agent-local"))]
pub use disabled::{Agents, Local, compose, is_unavailable};

#[cfg(feature = "agent-local")]
mod enabled {
    use std::time::Duration;

    use adam_host::Host;
    use anyhow::Context as _;
    use orch_agent_a2a::A2aAgentClient;
    use orch_agent_adam::{
        LocalAgentClient, LocalAgents, LocalAgentsError, LocalKind, LocalOptions,
    };
    use orch_core::BoxError;
    use orch_core::{Classify as _, ErrorClass};
    use orch_ports::ByTransport;

    use crate::config::{Config, LocalAgentKind};

    /// What the dispatcher calls: each endpoint goes to the client of its transport.
    pub type Agents = ByTransport<A2aAgentClient, LocalAgentClient>;

    /// How often an idle local worker, and every open subscription, looks at the journal.
    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    /// The kind this crate hosts for a configured kind. Exhaustive on purpose: a new kind in
    /// the configuration does not compile until it is hosted.
    fn hosted(kind: LocalAgentKind) -> LocalKind {
        match kind {
            LocalAgentKind::Echo => LocalKind::Echo,
        }
    }

    /// The local agents of this process, when `AGENTS_FILE` lists any.
    pub struct Local {
        agents: std::sync::Arc<LocalAgents>,
        steps: bool,
    }

    impl Local {
        /// Connects, migrates and builds the local agents; `None` when no entry has
        /// `transport: local`, and then nothing is connected.
        pub async fn start(cfg: &Config) -> anyhow::Result<Option<Local>> {
            let kinds = cfg.local_kinds();
            if kinds.is_empty() {
                return Ok(None);
            }
            let steps = cfg.role.runs_workers();
            let opts = LocalOptions {
                instance_id: cfg.instance_id.clone(),
                concurrency: cfg.agent_local_concurrency,
                lease_ttl: cfg.outbox_lease,
                poll_interval: POLL_INTERVAL,
                steps,
            };
            let hosted: Vec<LocalKind> = kinds.iter().copied().map(hosted).collect();
            let agents = LocalAgents::connect(&cfg.database_url, opts, &hosted)
                .await
                .context("cannot connect the local agents to Postgres (DATABASE_URL)")?;
            agents
                .migrate()
                .await
                .context("cannot apply the local agents' database migrations")?;
            tracing::info!(
                kinds = %kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(","),
                needs_model = kinds.iter().any(|k| k.needs_model()),
                steps,
                concurrency = cfg.agent_local_concurrency,
                "local agents ready"
            );
            Ok(Some(Local {
                agents: std::sync::Arc::new(agents),
                steps,
            }))
        }

        /// Adds the agents' worker to `host` in a role that runs workers. The control plane
        /// runs no local runtime and no notifier, so it adds nothing.
        pub fn register(&self, host: Host) -> Host {
            if !self.steps {
                return host;
            }
            let agents = std::sync::Arc::clone(&self.agents);
            host.worker("the local agents", move |stop| async move {
                agents.run(stop).await.map_err(|e| Box::new(e) as BoxError)
            })
        }

        /// Nothing to close: the pool lives in the agents and closes with the last reference.
        pub async fn close(self) {}
    }

    /// Routes each endpoint to the client of its transport.
    pub fn compose(a2a: A2aAgentClient, local: Option<&Local>) -> Agents {
        ByTransport {
            a2a,
            local: local.map(|l| l.agents.client()).unwrap_or_default(),
        }
    }

    /// Whether `cause` is a failure of the local agents that a restart may cure (the database is
    /// unreachable): the process exits with the "service unavailable" code, like the store's.
    pub fn is_unavailable(cause: &(dyn std::error::Error + 'static)) -> bool {
        cause
            .downcast_ref::<LocalAgentsError>()
            .is_some_and(|e| e.class() == ErrorClass::Transient)
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod tests {
        use super::*;

        #[test]
        fn only_a_transient_local_failure_is_unavailable() {
            let down = LocalAgentsError::unavailable(std::io::Error::other("connection refused"));
            assert!(is_unavailable(&down));
            let other = std::io::Error::other("nothing to do with local agents");
            assert!(!is_unavailable(&other));
        }
    }
}

#[cfg(not(feature = "agent-local"))]
mod disabled {
    use adam_host::Host;
    use orch_agent_a2a::A2aAgentClient;

    /// Without local agents the dispatcher calls the A2A client alone.
    pub type Agents = A2aAgentClient;

    /// Local agents are not in this build: the configuration refuses `transport: local`, so no
    /// value of this type exists.
    pub enum Local {}

    impl Local {
        /// Never hosts anything: there is no runtime to build.
        #[allow(
            clippy::unused_async,
            reason = "the same signature as the build with the feature"
        )]
        pub async fn start(_cfg: &crate::config::Config) -> anyhow::Result<Option<Local>> {
            Ok(None)
        }

        /// Unreachable: no value of `Local` exists.
        pub fn register(&self, _host: Host) -> Host {
            match *self {}
        }

        /// Unreachable: no value of `Local` exists.
        pub async fn close(self) {
            match self {}
        }
    }

    /// Without local agents there is only the A2A client.
    pub fn compose(a2a: A2aAgentClient, _local: Option<&Local>) -> Agents {
        a2a
    }

    /// No local failure exists in this build.
    pub fn is_unavailable(_cause: &(dyn std::error::Error + 'static)) -> bool {
        false
    }
}
