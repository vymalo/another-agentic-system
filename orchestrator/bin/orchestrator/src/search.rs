//! The subcommand `orchestrator search backfill` (ADR 0068): indexes the titles, descriptions and messages
//! that the event log holds and the search index has not been through, so that a deployment that had
//! threads before the index existed (migration `0025`) can search them. The work is in
//! [`ThreadSearch::backfill_search`] (`orch_ports`), implemented by the store; this module is the command
//! line, the configuration, the report and the exit code.
//!
//! It is `usage backfill`'s twin, and loads **the database of the configuration file and nothing else**
//! ([`crate::config::load_database`]). It applies the store's migrations (so a Job that runs before the
//! service has started finds the tables), then goes through the threads in pages, one transaction for each,
//! under the thread's own lock: **it can run while the service does**, and running it again finds nothing
//! left to do. The service indexes what it meets too (the next commit of a thread catches that thread up),
//! so the command only has to reach the threads that no one writes to any more. The logs go to **stderr**
//! so that stdout is the report: one line of JSON. Nothing a person wrote is in the report or the logs: only
//! counts.
//!
//! Exit codes (sysexits.h values):
//!
//! | Code | Meaning |
//! |---|---|
//! | `0` | the index has been through every thread's log |
//! | `2` | a usage error (clap) |
//! | `69` | the database could not be reached or failed; run it again, it resumes where the index is |
//! | `70` | the history of the store's migrations does not match this build, or another internal error |
//! | `75` | nothing failed, but some threads were left alone because their lock was held (`skipped` in the report, `complete: false`): run it again |
//! | `78` | the configuration (no file, no `database.url`) |

use std::process::ExitCode;

use clap::{ArgMatches, Args as ClapArgs, Subcommand};
use orch_core::{Classify as _, ErrorClass};
use orch_ports::{StoreError, ThreadSearch as _};
use orch_store_postgres::PgStore;
use secrecy::ExposeSecret as _;
use serde::Serialize;

use crate::config::{self, Args, LogFormat};
use crate::migrate::refuse_service_flags;
use crate::{EX_CONFIG, EX_SOFTWARE, EX_UNAVAILABLE, logging, report};

/// `orchestrator search ...`
#[derive(Debug, ClapArgs)]
pub struct SearchArgs {
    #[command(subcommand)]
    pub command: SearchCommand,
}

/// `orchestrator search <command>`
#[derive(Debug, Subcommand)]
pub enum SearchCommand {
    /// Index the titles, descriptions and messages the log holds and the search index has not been
    /// through (ADR 0068).
    ///
    /// For the threads that existed before migration 0025, and for those a build without the index wrote
    /// to: goes through the threads in pages of --page, each in a transaction of its own under the thread's
    /// lock, so it can run while the service does. Idempotent: a thread the index has been through is not
    /// read again, so a second run finds nothing, and the index is the same after it. A fork's copied
    /// events are indexed from the fork's own log (they are searched once its parent is gone).
    ///
    /// Waits at most 5 seconds for a thread's lock: a thread whose lock is held longer is skipped and
    /// counted (`skipped`), and a later run indexes it.
    ///
    /// Reads the configuration file (ORCH_CONFIG_FILE or --config), but only database.url. Applies the
    /// store's migrations, then indexes. Logs go to stderr; stdout is one line of JSON. Exit 0: the index
    /// has been through every thread's log. 69: the database failed, run it again; 70: an internal error;
    /// 75: threads were skipped, run it again; 78: the configuration.
    Backfill(BackfillArgs),
}

/// `orchestrator search backfill ...`
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
    pub messages: u64,
    /// Threads left alone because their lock was held: not gone through.
    pub skipped: u64,
}

/// Runs the command and returns the exit code. Usage errors exit 2 through clap.
pub async fn run_command(command: SearchArgs, args: Args, matches: &ArgMatches) -> ExitCode {
    let SearchArgs {
        command: SearchCommand::Backfill(backfill),
    } = command;
    refuse_service_flags(matches, "search backfill");

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

    match run(loaded.connection_url().expose_secret(), backfill.page).await {
        Ok(totals) => {
            if totals.skipped == 0 {
                tracing::info!(
                    pages = totals.pages,
                    threads = totals.threads,
                    messages = totals.messages,
                    "the search index has been through every thread's log"
                );
            } else {
                tracing::warn!(
                    pages = totals.pages,
                    threads = totals.threads,
                    messages = totals.messages,
                    skipped = totals.skipped,
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
            let done = store.backfill_search(after, page).await?;
            totals.pages += 1;
            totals.threads += u64::from(done.threads);
            totals.messages += done.messages;
            totals.skipped += u64::from(done.skipped);
            tracing::info!(
                page = totals.pages,
                threads = done.threads,
                messages = done.messages,
                skipped = done.skipped,
                "search index: a page of threads gone through"
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
/// left some alone because somebody held their lock (sysexits `EX_TEMPFAIL`): nothing failed, run it
/// again.
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
        messages: u64,
        skipped: u64,
    }
    serde_json::to_string(&Summary {
        complete: totals.skipped == 0,
        pages: totals.pages,
        threads: totals.threads,
        messages: totals.messages,
        skipped: totals.skipped,
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
        let Some(Command::Search(SearchArgs {
            command: SearchCommand::Backfill(backfill),
        })) = &args.command
        else {
            panic!("not the backfill command: {:?}", args.command);
        };
        backfill.page
    }

    #[test]
    fn the_command_is_search_backfill_and_a_page_is_a_hundred_threads_unless_told() {
        assert_eq!(
            page_of(&parse(&["orchestrator", "search", "backfill"]).unwrap()),
            100
        );
        assert_eq!(
            page_of(&parse(&["orchestrator", "search", "backfill", "--page", "7"]).unwrap()),
            7
        );
    }

    #[test]
    fn a_page_is_from_one_to_ten_thousand_threads() {
        for bad in ["0", "10001", "-1", "many", ""] {
            assert_eq!(
                parse(&["orchestrator", "search", "backfill", "--page", bad])
                    .unwrap_err()
                    .exit_code(),
                2,
                "{bad:?}"
            );
        }
        assert_eq!(
            page_of(&parse(&["orchestrator", "search", "backfill", "--page", "10000"]).unwrap()),
            10_000
        );
    }

    #[test]
    fn a_command_without_its_subcommand_or_with_an_unknown_one_is_a_usage_error() {
        let error = parse(&["orchestrator", "search"]).unwrap_err();
        assert!(
            error.use_stderr(),
            "a usage error goes to stderr and exits 2"
        );
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            parse(&["orchestrator", "search", "reset"])
                .unwrap_err()
                .exit_code(),
            2
        );
        assert_eq!(
            parse(&["orchestrator", "search", "backfill", "--all"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn help_for_the_command_says_what_it_does_and_how_it_ends() {
        let help = parse(&["orchestrator", "search", "backfill", "--help"])
            .unwrap_err()
            .to_string();
        for needle in [
            "index",
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
    fn a_database_that_fails_is_69_and_anything_else_70() {
        assert_eq!(error_exit_code(&StoreError::unavailable("down")), 69);
        assert_eq!(error_exit_code(&StoreError::corrupt("history")), 70);
        assert_eq!(error_exit_code(&StoreError::internal("bug")), 70);
    }

    #[test]
    fn the_summary_is_one_line_of_json_with_counts_only() {
        let line = summary(&Totals {
            pages: 3,
            threads: 250,
            messages: 4_100,
            skipped: 0,
        });
        assert_eq!(
            line,
            r#"{"complete":true,"pages":3,"threads":250,"messages":4100,"skipped":0}"#
        );
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_run_that_skipped_a_locked_thread_is_not_complete_and_exits_75_run_it_again() {
        let done = Totals {
            pages: 1,
            threads: 4,
            messages: 9,
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
