//! The subcommand `orchestrator secrets rewrap` (ADR 0046, slice 3): re-seals the secrets that
//! people keep for their own MCP servers under the key the configuration makes current, after a
//! key was added and made current and before the old one is removed. The work is in
//! `orch_secrets_pg::PgSecretStore::rewrap`; this module is the command line, the configuration,
//! the report and the exit code.
//!
//! It loads **the database and the secret store of the configuration file and nothing else**
//! ([`crate::config::load_user_secrets`]): no agents file, no thread-tools key, no model key is
//! read. It applies the crate's migrations (so a Job that runs before the service has started
//! finds its table), re-seals, and prints one line of JSON on **stdout**; the logs go to stderr.
//! Re-sealing keeps every secret's version, so it can run while the service is up.
//!
//! Exit codes (sysexits.h values, *verified 2026-10-14* against `/usr/include/sysexits.h` of glibc 2.39, package `libc6-dev`):
//!
//! | Code | Meaning |
//! |---|---|
//! | `0` | every secret is under the current key now: none failed and none changed while the run was under way |
//! | `2` | a usage error (clap) |
//! | `65` | a secret could not be re-sealed: sealed under a key the file does not list, or it does not open. It is left as it was; put the key back, or investigate |
//! | `69` | the database could not be reached or failed; run it again |
//! | `70` | the history of the crate's migrations does not match this build, or another internal error |
//! | `75` | nothing failed, but a secret was written or deleted while the run was under way and was left alone: run it again |
//! | `78` | the configuration (no file, no `secrets` section, a key that does not resolve), or a build without the Cargo feature `secrets-pg` |

use clap::{Args as ClapArgs, Subcommand};

/// `orchestrator secrets ...`
#[derive(Debug, ClapArgs)]
pub struct SecretsArgs {
    #[command(subcommand)]
    pub command: SecretsCommand,
}

/// `orchestrator secrets <command>`
#[derive(Debug, Subcommand)]
pub enum SecretsCommand {
    /// Re-seal every stored secret under the current key (ADR 0046).
    ///
    /// For a key rotation: add a key to `secrets.pg.keys`, make it `secrets.pg.current`, restart
    /// the service, run this, and when it exits 0 (and a second run finds everything current)
    /// remove the old key from the file. A secret keeps its version, so a refresh of a token that
    /// is under way is not disturbed.
    ///
    /// Reads the configuration file (ORCH_CONFIG_FILE or --config), but only database.url and the
    /// `secrets` section with its keys. Applies the migrations of the secret store, then re-seals.
    /// Logs go to stderr; stdout is one line of JSON. Exit 0: every secret is under the current key.
    /// 65: a secret could not be re-sealed (its key is not in the file, or it does not open); 69:
    /// the database failed; 70: an internal error; 75: a secret changed while the run was under
    /// way, run it again; 78: the configuration.
    Rewrap,
}

#[cfg(feature = "secrets-pg")]
pub use command::run_command;

#[cfg(not(feature = "secrets-pg"))]
pub use missing::run_command;

/// A build without the secret store cannot re-seal into it: a configuration error, as for any
/// store the build lacks (ADR 0046).
#[cfg(not(feature = "secrets-pg"))]
mod missing {
    use std::process::ExitCode;

    use clap::ArgMatches;

    use super::SecretsArgs;
    use crate::config::Args;
    use crate::migrate::refuse_service_flags;

    pub async fn run_command(_command: SecretsArgs, _args: Args, matches: &ArgMatches) -> ExitCode {
        refuse_service_flags(matches, "secrets rewrap");
        eprintln!(
            "configuration error: secrets rewrap needs the Cargo feature \"secrets-pg\", which this \
             build does not have (ADR 0046)"
        );
        ExitCode::from(crate::EX_CONFIG)
    }
}

#[cfg(feature = "secrets-pg")]
mod command {
    use std::process::ExitCode;

    use clap::ArgMatches;
    use orch_secrets_pg::{PgSecretStore, RewrapFailureKind, RewrapReport};
    use secrecy::ExposeSecret as _;
    use serde::Serialize;

    use super::{SecretsArgs, SecretsCommand};
    use crate::config::{self, Args, LogFormat, UserSecretsOnly};
    use crate::migrate::refuse_service_flags;
    use crate::{EX_CONFIG, EX_SOFTWARE, EX_UNAVAILABLE, logging, report};

    /// sysexits.h: a secret could not be re-sealed.
    const EX_DATAERR: u8 = 65;
    /// sysexits.h: a temporary condition; the invitation is to try again.
    const EX_TEMPFAIL: u8 = 75;

    /// The connections the run holds: it works one secret at a time.
    const CONNECTIONS: u32 = 2;

    /// Runs the command and returns the exit code. Usage errors exit 2 through clap.
    pub async fn run_command(command: SecretsArgs, args: Args, matches: &ArgMatches) -> ExitCode {
        let SecretsArgs {
            command: SecretsCommand::Rewrap,
        } = command;
        refuse_service_flags(matches, "secrets rewrap");

        let loaded = match config::load_user_secrets(&args) {
            Ok(loaded) => loaded,
            Err(e) => {
                report(&e);
                return ExitCode::from(EX_CONFIG);
            }
        };
        // A flag or variable wins over the file, as it does for the service.
        let format = args
            .log_format
            .as_deref()
            .map_or(loaded.log_format, |v| LogFormat::parse(Some(v)));
        logging::init_stderr(format);

        let current = loaded.keyring.current().to_string();
        match run(loaded).await {
            Ok(report) => {
                tracing::info!(
                    current_key = %current,
                    rewrapped = report.rewrapped,
                    already_current = report.already_current,
                    changed = report.changed,
                    failed = report.failed,
                    "rewrap finished"
                );
                if report.changed > 0 && report.failed == 0 {
                    eprintln!(
                        "{} secret(s) were written or deleted while the run was under way and were \
                         left alone: run it again before removing a key",
                        report.changed
                    );
                }
                // after the last log line, so that the report is the last line of a log that merges both streams
                println!("{}", summary(&report, &current));
                ExitCode::from(exit_code(&report))
            }
            Err(e) => {
                // the error's text only: the lower error is the driver's, and a connection error
                // can carry the address of the database
                eprintln!("error: {e}");
                ExitCode::from(error_exit_code(&e))
            }
        }
    }

    /// Connects, applies the migrations, re-seals.
    async fn run(loaded: UserSecretsOnly) -> Result<RewrapReport, orch_ports::SecretError> {
        let store = PgSecretStore::connect_with(
            loaded.database_url.expose_secret(),
            loaded.keyring,
            CONNECTIONS,
        )
        .await?;
        let outcome = async {
            store.migrate().await?;
            tracing::info!(
                current_key = %store.keyring().current(),
                keys = ?store.keyring().ids().map(ToString::to_string).collect::<Vec<_>>(),
                "re-sealing the stored secrets under the current key"
            );
            store.rewrap().await
        }
        .await;
        store.close().await;
        outcome
    }

    /// The exit code of a finished run: `0` when every secret is under the current key, `65` when
    /// one could not be re-sealed (whatever else happened), `75` when none failed and some
    /// changed under the run.
    pub fn exit_code(report: &RewrapReport) -> u8 {
        if report.failed > 0 {
            EX_DATAERR
        } else if report.changed > 0 {
            EX_TEMPFAIL
        } else {
            0
        }
    }

    /// The exit code of a run that did not get to a report: `69` for a database that is down or
    /// refuses, `70` for anything else.
    pub fn error_exit_code(error: &orch_ports::SecretError) -> u8 {
        use orch_core::{Classify as _, ErrorClass};
        match error.class() {
            ErrorClass::Transient => EX_UNAVAILABLE,
            _ => EX_SOFTWARE,
        }
    }

    /// The report: one JSON object on one line, for the Job's log and for a script. A failure names
    /// the server (`u-…`) and the key it is sealed under, never the person.
    pub fn summary(report: &RewrapReport, current: &str) -> String {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct FailureLine<'a> {
            server: &'a str,
            kid: &'a str,
            kind: &'static str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Summary<'a> {
            complete: bool,
            current_key: &'a str,
            rewrapped: u64,
            already_current: u64,
            changed: u64,
            failed: u64,
            failures: Vec<FailureLine<'a>>,
        }
        let line = Summary {
            complete: report.is_complete(),
            current_key: current,
            rewrapped: report.rewrapped,
            already_current: report.already_current,
            changed: report.changed,
            failed: report.failed,
            failures: report
                .failures
                .iter()
                .map(|f| FailureLine {
                    server: &f.server,
                    kid: &f.kid,
                    kind: match f.kind {
                        RewrapFailureKind::KeyUnknown => "keyUnknown",
                        RewrapFailureKind::Corrupt => "corrupt",
                        // a new kind of failure of the library is worth a word, not a panic
                        _ => "other",
                    },
                })
                .collect(),
        };
        serde_json::to_string(&line).unwrap_or_else(|_| "{}".to_owned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use clap::{CommandFactory as _, FromArgMatches as _};

    use super::*;
    use crate::config::Args;
    use crate::migrate::Command;

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        let matches = Args::command().try_get_matches_from(args)?;
        Args::from_arg_matches(&matches)
    }

    #[test]
    fn the_command_is_secrets_rewrap_and_takes_no_flag_of_its_own() {
        let args = parse(&["orchestrator", "secrets", "rewrap"]).unwrap();
        let Some(Command::Secrets(SecretsArgs {
            command: SecretsCommand::Rewrap,
        })) = args.command
        else {
            panic!("not the rewrap command: {:?}", args.command);
        };
        // nothing to ask for: it is idempotent and keeps what it re-seals
        for extra in [["--dry-run"], ["--all"]] {
            let mut line = vec!["orchestrator", "secrets", "rewrap"];
            line.extend(extra);
            assert_eq!(
                parse(&line).unwrap_err().kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_command_without_its_subcommand_is_a_usage_error() {
        let error = parse(&["orchestrator", "secrets"]).unwrap_err();
        assert!(
            error.use_stderr(),
            "a usage error goes to stderr and exits 2"
        );
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            parse(&["orchestrator", "secrets", "unseal"])
                .unwrap_err()
                .exit_code(),
            2
        );
    }

    #[test]
    fn the_configuration_file_and_the_log_format_may_come_before_the_command_or_after_it() {
        let before = parse(&[
            "orchestrator",
            "--config",
            "/etc/a.yaml",
            "secrets",
            "rewrap",
        ])
        .unwrap();
        assert_eq!(before.config.as_deref(), Some("/etc/a.yaml"));
        let after = parse(&[
            "orchestrator",
            "secrets",
            "rewrap",
            "--config",
            "/etc/b.yaml",
            "--log-format",
            "text",
        ])
        .unwrap();
        assert_eq!(after.config.as_deref(), Some("/etc/b.yaml"));
        assert_eq!(after.log_format.as_deref(), Some("text"));
    }

    #[test]
    fn help_for_the_command_says_what_it_does_and_how_it_ends() {
        let error = parse(&["orchestrator", "secrets", "rewrap", "--help"]).unwrap_err();
        let help = error.to_string();
        for needle in [
            "current key",
            "secrets.pg.current",
            "ORCH_CONFIG_FILE",
            "one line of JSON",
            "65",
            "69",
            "75",
            "78",
        ] {
            assert!(help.contains(needle), "{needle} is not in:\n{help}");
        }
    }

    /// The service's `--print-config` takes no command, and a command takes none of the service's
    /// flags: the same refusals as for `artifacts migrate`.
    #[test]
    fn the_service_flags_stay_the_services() {
        let args = parse(&["orchestrator", "--role", "worker", "secrets", "rewrap"]).unwrap();
        assert!(args.command.is_some());
        assert_eq!(args.role.as_deref(), Some("worker"));
    }

    #[cfg(feature = "secrets-pg")]
    mod the_report {
        use orch_core::ErrorClass;
        use orch_ports::SecretError;
        use orch_secrets_pg::{RewrapFailure, RewrapFailureKind, RewrapReport};

        use super::super::command::{error_exit_code, exit_code, summary};

        fn report(rewrapped: u64, changed: u64, failed: u64) -> RewrapReport {
            RewrapReport {
                rewrapped,
                already_current: 3,
                changed,
                failed,
                failures: Vec::new(),
            }
        }

        #[test]
        fn zero_only_when_every_secret_is_under_the_current_key() {
            assert_eq!(exit_code(&report(0, 0, 0)), 0, "nothing to do is done");
            assert_eq!(exit_code(&report(5, 0, 0)), 0);
        }

        #[test]
        fn a_secret_that_changed_under_the_run_is_75_run_it_again() {
            assert_eq!(exit_code(&report(5, 1, 0)), 75);
        }

        #[test]
        fn a_secret_that_could_not_be_re_sealed_is_65_whatever_else_happened() {
            assert_eq!(exit_code(&report(5, 0, 1)), 65);
            assert_eq!(exit_code(&report(0, 2, 1)), 65, "failed outranks changed");
        }

        #[test]
        fn a_database_that_fails_is_69_and_anything_else_70() {
            assert_eq!(error_exit_code(&SecretError::unavailable("down")), 69);
            assert_eq!(error_exit_code(&SecretError::corrupt("history")), 70);
            assert_eq!(error_exit_code(&SecretError::KeyUnknown), 70);
            assert_eq!(
                orch_core::Classify::class(&SecretError::unavailable("x")),
                ErrorClass::Transient
            );
        }

        #[test]
        fn the_summary_is_one_line_of_json_that_names_each_failure_by_server_and_key_and_no_person()
        {
            let mut r = report(2, 1, 2);
            r.failures = vec![
                RewrapFailure {
                    server: "u-aaaaaaaaaa".to_owned(),
                    kid: "k0".to_owned(),
                    kind: RewrapFailureKind::KeyUnknown,
                },
                RewrapFailure {
                    server: "u-bbbbbbbbbb#flow".to_owned(),
                    kid: "k1".to_owned(),
                    kind: RewrapFailureKind::Corrupt,
                },
            ];
            let line = summary(&r, "k2");
            assert!(!line.contains('\n'));
            let json: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                json,
                serde_json::json!({
                    "complete": false,
                    "currentKey": "k2",
                    "rewrapped": 2,
                    "alreadyCurrent": 3,
                    "changed": 1,
                    "failed": 2,
                    "failures": [
                        {"server": "u-aaaaaaaaaa", "kid": "k0", "kind": "keyUnknown"},
                        {"server": "u-bbbbbbbbbb#flow", "kid": "k1", "kind": "corrupt"},
                    ],
                })
            );
            let clean = summary(&report(4, 0, 0), "k2");
            let json: serde_json::Value = serde_json::from_str(&clean).unwrap();
            assert_eq!(json["complete"], true);
            assert_eq!(json["failures"], serde_json::json!([]));
        }
    }
}
