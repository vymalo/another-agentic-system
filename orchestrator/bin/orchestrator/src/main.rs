//! The orchestrator service. Configuration is read from the environment (see the README);
//! `boot` composes the adapters and `config` parses the environment and the agent list.

mod boot;
mod config;

use config::{Config, LogFormat};
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

#[tokio::main]
async fn main() -> std::process::ExitCode {
    init_tracing(LogFormat::parse(
        std::env::var("LOG_FORMAT").ok().as_deref(),
    ));
    let result = async {
        let cfg = Config::from_process_env()?;
        boot::run(cfg, termination()).await
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // One structured line, so a log collector sees why the process died.
            tracing::error!(error = %format!("{e:#}"), "orchestrator failed");
            std::process::ExitCode::FAILURE
        }
    }
}
