//! The orchestrator service. Configuration is read from the environment (see the README);
//! `boot` composes the adapters and `config` parses the environment and the agent list.

mod boot;
mod config;

use std::process::ExitCode;

use anyhow::Context as _;
use boot::Fatal;
use config::{Config, ConfigError, LogFormat};
use orch_core::{Classify as _, ErrorClass};
use orch_ports::StoreError;
use tracing_subscriber::EnvFilter;

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Text => builder.init(),
    }
}

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
/// sysexits.h: an internal software error (a half of the service stopped or panicked).
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
        match cause.downcast_ref::<Fatal>() {
            Some(Fatal::Listen { .. }) => return EX_OSERR,
            Some(Fatal::Stopped { .. }) => return EX_SOFTWARE,
            None => {}
        }
        if cause.downcast_ref::<tokio::task::JoinError>().is_some() {
            return EX_SOFTWARE;
        }
    }
    1
}

async fn run() -> anyhow::Result<()> {
    let cfg = Config::from_process_env().context("reading the configuration")?;
    boot::run(cfg, termination()).await
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing(LogFormat::parse(
        std::env::var("LOG_FORMAT").ok().as_deref(),
    ));
    match run().await {
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

        let db_down = anyhow::Error::from(StoreError::unavailable(io::Error::other("refused")))
            .context("cannot connect to Postgres");
        assert_eq!(exit_code(&db_down), 69);

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

        let stopped = anyhow::Error::from(Fatal::Stopped {
            half: "the dispatcher",
            source: Some(boxed("panicked")),
        });
        assert_eq!(exit_code(&stopped), 70);

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
