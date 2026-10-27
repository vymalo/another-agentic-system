//! The subcommand `orchestrator usage backfill` (ADR 0065): folds into the usage ledger of the people
//! the usage events that the event log holds and the ledger has not been through, so that a
//! deployment that had threads before the ledger existed (migration `0023`) has the usage of those
//! threads in the ledger too. The work is in [`UsageLedger::backfill`]
//! (`orch_ports`), implemented by the store; this module is the command line, the configuration, the
//! report and the exit code.
//!
//! It loads **the database of the configuration file and nothing else** ([`crate::config::load_database`]):
//! no agents file, no thread-tools key, no model key is read. It applies the store's migrations (so a
//! Job that runs before the service has started finds the tables), then goes through the threads in
//! pages, one transaction for each, under the thread's own lock: **it can run while the service
//! does**, and running it again finds nothing left to do. The service folds in what it meets
//! too (the next commit of a thread catches that thread up), so the command only has to reach the
//! threads that no one writes to any more. The logs go to **stderr** so that stdout is the report:
//! one line of JSON.
//!
//! Exit codes (sysexits.h values):
//!
//! | Code | Meaning |
//! |---|---|
//! | `0` | the ledger has been through every thread's log |
//! | `2` | a usage error (clap) |
//! | `69` | the database could not be reached or failed; run it again, it resumes where the ledger is |
//! | `70` | the history of the store's migrations does not match this build, or another internal error |
//! | `75` | nothing failed, but some threads were left alone because their lock was held (`skipped` in the report, `complete: false`): run it again |
//! | `78` | the configuration (no file, no `database.url`) |

use std::process::ExitCode;

use clap::{ArgMatches, Args as ClapArgs, Subcommand};
use orch_core::{Classify as _, ErrorClass};
use orch_ports::{StoreError, UsageLedger as _};
use orch_store_postgres::PgStore;
use secrecy::ExposeSecret as _;
use serde::Serialize;

use crate::config::{self, Args, LogFormat};
use crate::migrate::refuse_service_flags;
use crate::{EX_CONFIG, EX_SOFTWARE, EX_UNAVAILABLE, logging, report};

/// `orchestrator usage ...`
#[derive(Debug, ClapArgs)]
pub struct UsageArgs {
    #[command(subcommand)]
    pub command: UsageCommand,
}

/// `orchestrator usage <command>`
#[derive(Debug, Subcommand)]
pub enum UsageCommand {
    /// Fold into the usage ledger the usage events the log holds and the ledger has not been through
    /// (ADR 0065).
    ///
    /// For the threads that existed before migration 0023, and for those a build without the ledger
    /// wrote to: goes through the threads in pages of --page, each in a transaction of its own under
    /// the thread's lock, so it can run while the service does. Idempotent: a thread the ledger has
    /// been through is not read again, so a second run finds nothing, and the ledger's rows are the
    /// same after it. The events a fork copied from its parent are not counted again (their usage
    /// was counted in the parent); the usage of a thread that was erased before this ran is not in
    /// the log any more and cannot be recovered.
    ///
    /// Waits at most 5 seconds for a thread's lock: a thread whose lock is held longer is skipped and
    /// counted (`skipped`), and a later run folds it in. The first page also makes the rows of threads
    /// that an older build erased name no thread (`detached`).
    ///
    /// Reads the configuration file (ORCH_CONFIG_FILE or --config), but only database.url. Applies
    /// the store's migrations, then folds. Logs go to stderr; stdout is one line of JSON. Exit 0:
    /// the ledger has been through every thread's log. 69: the database failed, run it again;
    /// 70: an internal error; 75: threads were skipped, run it again; 78: the configuration.
    Backfill(BackfillArgs),
}

/// `orchestrator usage backfill ...`
#[derive(Debug, Clone, ClapArgs)]
pub struct BackfillArgs {
    /// How many threads a page goes through; each thread is a transaction of its own.
    #[arg(long, value_name = "N", default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=10_000))]
    pub page: u32,
}

/// The connections the run holds: it works one thread at a time.
const CONNECTIONS: u32 = 2;

/// sysexits `EX_TEMPFAIL`: a temporary failure, run it again.
const EX_TEMPFAIL: u8 = 75;

/// What a run did, in total.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub pages: u64,
    pub threads: u64,
    pub events: u64,
    /// Threads left alone because their lock was held: not gone through.
    pub skipped: u64,
    /// Rows made to name no thread because an older build erased theirs.
    pub detached: u64,
}

/// Runs the command and returns the exit code. Usage errors exit 2 through clap.
pub async fn run_command(command: UsageArgs, args: Args, matches: &ArgMatches) -> ExitCode {
    let UsageArgs {
        command: UsageCommand::Backfill(backfill),
    } = command;
    refuse_service_flags(matches, "usage backfill");

    let loaded = match config::load_database(&args) {
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

    match run(loaded.database_url.expose_secret(), backfill.page).await {
        Ok(totals) => {
            if totals.skipped == 0 {
                tracing::info!(
                    pages = totals.pages,
                    threads = totals.threads,
                    events = totals.events,
                    detached = totals.detached,
                    "the usage ledger has been through every thread's log"
                );
            } else {
                tracing::warn!(
                    pages = totals.pages,
                    threads = totals.threads,
                    events = totals.events,
                    skipped = totals.skipped,
                    detached = totals.detached,
                    "threads were skipped because their lock was held: run the command again"
                );
            }
            // after the last log line, so that the report is the last line of a log that merges both streams
            println!("{}", summary(&totals));
            ExitCode::from(exit_code(&totals))
        }
        Err(e) => {
            // the error's text only: the lower error is the driver's, and a connection error can
            // carry the address of the database
            eprintln!("error: {e}");
            ExitCode::from(error_exit_code(&e))
        }
    }
}

/// Connects, applies the migrations, goes through the threads page by page.
async fn run(database_url: &str, page: u32) -> Result<Totals, StoreError> {
    let store = PgStore::connect_with(database_url, CONNECTIONS).await?;
    let outcome = async {
        store.migrate().await?;
        let mut totals = Totals::default();
        let mut after = None;
        loop {
            let done = store.backfill(after, page).await?;
            totals.pages += 1;
            totals.threads += u64::from(done.threads);
            totals.events += done.events;
            totals.skipped += u64::from(done.skipped);
            totals.detached += done.detached;
            tracing::info!(
                page = totals.pages,
                threads = done.threads,
                events = done.events,
                skipped = done.skipped,
                detached = done.detached,
                "usage ledger: a page of threads gone through"
            );
            match done.next {
                Some(next) => after = Some(next),
                None => return Ok(totals),
            }
        }
    }
    .await;
    store.pool().close().await;
    outcome
}

/// The exit code of a run that got to a report: `0` when it went through every thread, `75` when it
/// left some alone because somebody held their lock (sysexits `EX_TEMPFAIL`, as `secrets rewrap`
/// uses it): nothing failed, run it again.
pub fn exit_code(totals: &Totals) -> u8 {
    if totals.skipped == 0 { 0 } else { EX_TEMPFAIL }
}

/// The exit code of a run that did not get to a report: `69` for a database that is down or
/// refuses, `70` for anything else.
pub fn error_exit_code(error: &StoreError) -> u8 {
    match error.class() {
        ErrorClass::Transient => EX_UNAVAILABLE,
        _ => EX_SOFTWARE,
    }
}

/// The report: one JSON object on one line, for the Job's log and for a script.
pub fn summary(totals: &Totals) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Summary {
        complete: bool,
        pages: u64,
        threads: u64,
        events: u64,
        skipped: u64,
        detached: u64,
    }
    serde_json::to_string(&Summary {
        complete: totals.skipped == 0,
        pages: totals.pages,
        threads: totals.threads,
        events: totals.events,
        skipped: totals.skipped,
        detached: totals.detached,
    })
    .unwrap_or_else(|_| "{}".to_owned())
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

    fn page_of(args: &Args) -> u32 {
        let Some(Command::Usage(UsageArgs {
            command: UsageCommand::Backfill(backfill),
        })) = &args.command
        else {
            panic!("not the backfill command: {:?}", args.command);
        };
        backfill.page
    }

    #[test]
    fn the_command_is_usage_backfill_and_a_page_is_a_hundred_threads_unless_told() {
        assert_eq!(
            page_of(&parse(&["orchestrator", "usage", "backfill"]).unwrap()),
            100
        );
        assert_eq!(
            page_of(&parse(&["orchestrator", "usage", "backfill", "--page", "7"]).unwrap()),
            7
        );
    }

    #[test]
    fn a_page_is_from_one_to_ten_thousand_threads() {
        for bad in ["0", "10001", "-1", "many", ""] {
            assert_eq!(
                parse(&["orchestrator", "usage", "backfill", "--page", bad])
                    .unwrap_err()
                    .exit_code(),
                2,
                "{bad:?}"
            );
        }
        assert_eq!(
            page_of(&parse(&["orchestrator", "usage", "backfill", "--page", "10000"]).unwrap()),
            10_000
        );
    }

    #[test]
    fn a_command_without_its_subcommand_or_with_an_unknown_one_is_a_usage_error() {
        let error = parse(&["orchestrator", "usage"]).unwrap_err();
        assert!(
            error.use_stderr(),
            "a usage error goes to stderr and exits 2"
        );
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            parse(&["orchestrator", "usage", "reset"])
                .unwrap_err()
                .exit_code(),
            2
        );
        assert_eq!(
            parse(&["orchestrator", "usage", "backfill", "--all"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn the_configuration_file_and_the_log_format_may_come_before_the_command_or_after_it() {
        let before = parse(&[
            "orchestrator",
            "--config",
            "/etc/a.yaml",
            "usage",
            "backfill",
        ])
        .unwrap();
        assert_eq!(before.config.as_deref(), Some("/etc/a.yaml"));
        let after = parse(&[
            "orchestrator",
            "usage",
            "backfill",
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
        let help = parse(&["orchestrator", "usage", "backfill", "--help"])
            .unwrap_err()
            .to_string();
        for needle in [
            "ledger",
            "ORCH_CONFIG_FILE",
            "one line of JSON",
            "Idempotent",
            "69",
            "75",
            "70",
            "78",
        ] {
            assert!(help.contains(needle), "{needle} is not in:\n{help}");
        }
    }

    #[test]
    fn the_service_flags_stay_the_services() {
        let args = parse(&["orchestrator", "--role", "worker", "usage", "backfill"]).unwrap();
        assert!(args.command.is_some());
        assert_eq!(args.role.as_deref(), Some("worker"));
    }

    #[test]
    fn a_database_that_fails_is_69_and_anything_else_70() {
        assert_eq!(error_exit_code(&StoreError::unavailable("down")), 69);
        assert_eq!(error_exit_code(&StoreError::corrupt("history")), 70);
        assert_eq!(error_exit_code(&StoreError::internal("bug")), 70);
    }

    #[test]
    fn the_summary_is_one_line_of_json() {
        let line = summary(&Totals {
            pages: 3,
            threads: 250,
            events: 4_100,
            skipped: 0,
            detached: 2,
        });
        assert_eq!(
            line,
            r#"{"complete":true,"pages":3,"threads":250,"events":4100,"skipped":0,"detached":2}"#
        );
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_run_that_skipped_a_locked_thread_is_not_complete_and_exits_75_run_it_again() {
        let done = Totals {
            pages: 1,
            threads: 4,
            events: 9,
            ..Totals::default()
        };
        assert_eq!(exit_code(&done), 0);
        let skipped = Totals { skipped: 2, ..done };
        assert_eq!(exit_code(&skipped), 75);
        let line = summary(&skipped);
        assert!(line.contains(r#""complete":false"#), "{line}");
        assert!(line.contains(r#""skipped":2"#), "{line}");
    }
}
