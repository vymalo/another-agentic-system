//! The orchestrator service. Configuration is read from flags with environment fallback (see
//! the README and `--help`); `boot` composes the adapters and `config` parses and validates the
//! flags, the environment and the agent list.

mod boot;
mod config;
mod local;
mod logging;
mod model;

use std::process::ExitCode;

use adam_host::HostError;
use boot::Fatal;
use clap::Parser as _;
use config::{Args, Config, ConfigError, LogFormat};
use orch_core::{Classify as _, ErrorClass};
use orch_ports::StoreError;

/// Resolves on SIGTERM (Kubernetes) or SIGINT (Ctrl-C).
async fn termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM; only Ctrl-C stops the service");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// sysexits.h: a configuration error.
const EX_CONFIG: u8 = 78;
/// sysexits.h: a service the process needs (Postgres) is unavailable.
const EX_UNAVAILABLE: u8 = 69;
/// sysexits.h: an operating system error (the listen address cannot be bound).
const EX_OSERR: u8 = 71;
/// sysexits.h: an internal software error (a component of the service stopped or panicked).
const EX_SOFTWARE: u8 = 70;

/// The exit code for a fatal error, found by walking its source chain for the first cause
/// that says what kind of failure it is (sysexits.h values, *unverified*: from memory of the
/// BSD header). Anything else exits 1.
fn exit_code(err: &anyhow::Error) -> u8 {
    for cause in err.chain() {
        if cause.downcast_ref::<ConfigError>().is_some() {
            return EX_CONFIG;
        }
        if let Some(store) = cause.downcast_ref::<StoreError>()
            && store.class() == ErrorClass::Transient
        {
            return EX_UNAVAILABLE;
        }
        // The local agents' database (a build with `agent-local`) is unreachable: same as the store.
        if local::is_unavailable(cause) {
            return EX_UNAVAILABLE;
        }
        if let Some(Fatal::Listen { .. }) = cause.downcast_ref::<Fatal>() {
            return EX_OSERR;
        }
        // A component (the HTTP server, the dispatcher) stopped, ended early or panicked.
        if cause.downcast_ref::<HostError>().is_some() {
            return EX_SOFTWARE;
        }
        if cause.downcast_ref::<tokio::task::JoinError>().is_some() {
            return EX_SOFTWARE;
        }
    }
    1
}

#[tokio::main]
async fn main() -> ExitCode {
    // `--help` and `--version` exit 0 here, and a malformed command line exits 2 (clap's usage
    // error); every problem with a *value* is a `ConfigError`, exit 78, after tracing is up.
    let args = Args::parse();
    let format = LogFormat::parse(args.log_format.as_deref());
    // The configuration is read before tracing is installed, so that every line carries the
    // role and the instance id. A bad configuration is logged like any other fatal error, just
    // without them (there is no role yet).
    let cfg = Config::from_args(args);
    logging::init(
        format,
        cfg.as_ref().ok().map(|cfg| logging::ProcessFields {
            role: cfg.role.as_str(),
            instance: cfg.instance_id.clone(),
        }),
    );
    let outcome = match cfg {
        Ok(cfg) => boot::run(cfg, termination()).await,
        Err(e) => Err(anyhow::Error::from(e).context("reading the configuration")),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // One structured line, so a log collector sees why the process died: every layer
            // once (`{:#}` joins the chain; no message repeats its source).
            let code = exit_code(&e);
            tracing::error!(error = %format!("{e:#}"), code, "orchestrator failed");
            ExitCode::from(code)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io;

    use super::*;

    fn boxed(msg: &str) -> orch_core::BoxError {
        msg.into()
    }

    #[test]
    fn exit_codes_follow_the_root_cause() {
        let config = anyhow::Error::from(ConfigError::Missing("DATABASE_URL"))
            .context("reading the configuration");
        assert_eq!(exit_code(&config), 78);

        let no_local = anyhow::Error::from(ConfigError::LocalAgentsNotCompiled {
            agent: "helper".to_owned(),
            feature: "agent-local",
        })
        .context("reading the configuration");
        assert_eq!(exit_code(&no_local), 78);

        let removed = anyhow::Error::from(ConfigError::RemovedSurface {
            name: "chat-api",
            removed: "2026-09-30",
            what: "the legacy chat API interaction routes",
            replacement: "the AG-UI routes",
        })
        .context("reading the configuration");
        assert_eq!(exit_code(&removed), 78);

        let gate = anyhow::Error::from(ConfigError::Gate {
            context: "AGENTS_FILE",
            reason: "the ci source is not available yet".to_owned(),
        })
        .context("reading the configuration");
        assert_eq!(exit_code(&gate), 78);
        // A token variable that is missing, or a file that cannot be read, is a configuration error.
        let no_token = anyhow::Error::from(ConfigError::McpTokenEnvMissing {
            user: "alice@example.com".to_owned(),
            var: "MCP_TOKEN_ALICE".to_owned(),
        })
        .context("reading the configuration");
        assert_eq!(exit_code(&no_token), 78);
        let no_file = anyhow::Error::from(ConfigError::McpTokensFileRead {
            path: "/etc/orch/mcp-tokens.yaml".into(),
            source: io::Error::from(io::ErrorKind::NotFound),
        })
        .context("reading the configuration");
        assert_eq!(exit_code(&no_file), 78);

        let db_down = anyhow::Error::from(StoreError::unavailable(io::Error::other("refused")))
            .context("cannot connect to Postgres");
        assert_eq!(exit_code(&db_down), 69);

        // The local agents' database (the feature `agent-local`) is the same outage: 69.
        #[cfg(feature = "agent-local")]
        {
            let down = anyhow::Error::from(orch_agent_adam::LocalAgentsError::unavailable(
                io::Error::other("refused"),
            ))
            .context("cannot connect the local agents to Postgres");
            assert_eq!(exit_code(&down), 69);
        }

        let db_bug = anyhow::Error::from(StoreError::internal(io::Error::other("syntax")))
            .context("cannot apply the database migrations");
        assert_eq!(
            exit_code(&db_bug),
            1,
            "only a transient store failure is 69"
        );

        let listen = anyhow::Error::from(Fatal::Listen {
            addr: "0.0.0.0:80".parse().unwrap(),
            source: io::Error::from(io::ErrorKind::AddrInUse),
        });
        assert_eq!(exit_code(&listen), 71);

        let stopped = anyhow::Error::from(HostError::Stopped {
            component: "the dispatcher".to_owned(),
            source: boxed("lost the database"),
        });
        assert_eq!(exit_code(&stopped), 70);
        let panicked = anyhow::Error::from(HostError::Panicked {
            component: "the HTTP server".to_owned(),
            source: boxed("boom"),
        });
        assert_eq!(exit_code(&panicked), 70);
        let early = anyhow::Error::from(HostError::EndedEarly {
            component: "the dispatcher".to_owned(),
        });
        assert_eq!(exit_code(&early), 70);
        assert_eq!(exit_code(&anyhow::Error::from(HostError::NothingToRun)), 70);

        assert_eq!(exit_code(&anyhow::anyhow!("something else")), 1);
    }

    #[tokio::test]
    async fn a_panicked_task_is_a_software_error_wherever_it_sits() {
        let join = tokio::spawn(async { panic!("boom") }).await.unwrap_err();
        let e = anyhow::Error::from(join).context("the dispatcher panicked");
        assert_eq!(exit_code(&e), 70);
    }

    #[test]
    fn the_first_cause_in_the_chain_decides() {
        // A Postgres outage carries an io::Error deep down; it is still 69, not an OS error.
        let e = anyhow::Error::from(StoreError::unavailable(io::Error::other("reset")))
            .context("cannot connect to Postgres");
        assert_eq!(exit_code(&e), 69);
    }

    #[test]
    fn a_fatal_error_prints_every_layer_once() {
        let e = anyhow::Error::from(StoreError::unavailable(io::Error::other(
            "connection refused",
        )))
        .context("cannot connect to Postgres");
        let line = format!("{e:#}");
        assert_eq!(
            line,
            "cannot connect to Postgres: store unavailable: connection refused"
        );
    }
}
