//! The subcommand `orchestrator search backfill` (ADR 0068): indexes the titles, descriptions and messages
//! that the event log holds and the search index has not been through, so that a deployment that had
//! threads before the index existed (migration `0025`), or whose fold version was raised
//! (`orch_core::search::FOLD_VERSION`), can search them. `--rebuild` goes through every thread.
//! [`warn_when_unindexed`] is what the service says at startup when threads wait for it. The work is in
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
use orch_ports::{StoreError, ThreadSearch};
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
    /// A thread the index is behind is indexed from the point it has reached; one stored under another
    /// fold version (the folding rules changed) is indexed again from the start. With --rebuild every
    /// thread is, whatever it has stored.
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
    /// Index every thread again from the start, replacing its rows, whatever the index has stored
    /// (after a change to the folding rules that raised no version, or to be sure).
    #[arg(long)]
    pub rebuild: bool,
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
    /// Threads the index is still behind when the run is over (counted up to [`BACKLOG_CAP`]): `0` when
    /// it went through everything, more when threads were skipped or were written to meanwhile.
    pub remaining: u32,
}

/// The most threads a count of those the index is behind reads: the startup check and the report of the
/// command stop here, so neither reads a large table for a number nobody needs to the unit.
pub const BACKLOG_CAP: u32 = 100_000;

/// What the service says at startup when `unindexed` threads wait for the index (`None`: nothing to say).
pub fn unindexed_warning(unindexed: u32) -> Option<String> {
    (unindexed > 0).then(|| {
        let count = if unindexed >= BACKLOG_CAP {
            format!("{BACKLOG_CAP} or more threads")
        } else if unindexed == 1 {
            "1 thread".to_owned()
        } else {
            format!("{unindexed} threads")
        };
        format!(
            "search: {count} are not in the search index yet (threads from before it existed, or indexed under \
             other folding rules) and are not found by a search until a new message or `orchestrator search \
             backfill` indexes them: run `orchestrator search backfill`"
        )
    })
}

/// Logs the startup warning when the index is behind: one count, **capped at [`BACKLOG_CAP`]**, that reads
/// the table of threads once (it stops at the cap, but reads all of it when almost every thread is
/// indexed), once per process start and never on a request. A store that cannot answer is not a reason
/// to stop starting: it is logged at debug.
pub async fn warn_when_unindexed(store: &impl ThreadSearch) {
    match store.search_backlog(BACKLOG_CAP).await {
        Ok(n) => {
            if let Some(line) = unindexed_warning(n) {
                tracing::warn!(unindexed_threads = n, "{line}");
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "could not count the threads the search index is behind")
        }
    }
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

    match run(
        loaded.connection_url().expose_secret(),
        backfill.page,
        backfill.rebuild,
    )
    .await
    {
        Ok(totals) => {
            if totals.skipped == 0 {
                tracing::info!(
                    pages = totals.pages,
                    threads = totals.threads,
                    messages = totals.messages,
                    remaining = totals.remaining,
                    "the search index has been through every thread's log"
                );
            } else {
                tracing::warn!(
                    pages = totals.pages,
                    threads = totals.threads,
                    messages = totals.messages,
                    skipped = totals.skipped,
                    remaining = totals.remaining,
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
async fn run(database_url: &str, page: u32, rebuild: bool) -> Result<Totals, StoreError> {
    let store = PgStore::connect_with(database_url, CONNECTIONS).await?;
    let outcome = async {
        store.migrate().await?;
        let mut totals = Totals::default();
        let mut after = None;
        loop {
            let done = store.backfill_search(after, page, rebuild).await?;
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
                None => {
                    // how many are still behind: those skipped, and any written to since their turn
                    totals.remaining = store.search_backlog(BACKLOG_CAP).await?;
                    return Ok(totals);
                }
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
        remaining: u32,
    }
    serde_json::to_string(&Summary {
        complete: totals.skipped == 0,
        pages: totals.pages,
        threads: totals.threads,
        messages: totals.messages,
        skipped: totals.skipped,
        remaining: totals.remaining,
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
        backfill_of(args).page
    }

    fn backfill_of(args: &Args) -> BackfillArgs {
        let Some(Command::Search(SearchArgs {
            command: SearchCommand::Backfill(backfill),
        })) = &args.command
        else {
            panic!("not the backfill command: {:?}", args.command);
        };
        backfill.clone()
    }

    #[test]
    fn rebuild_is_a_flag_and_is_off_unless_given() {
        assert!(!backfill_of(&parse(&["orchestrator", "search", "backfill"]).unwrap()).rebuild);
        assert!(
            backfill_of(&parse(&["orchestrator", "search", "backfill", "--rebuild"]).unwrap())
                .rebuild
        );
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
            "--rebuild",
            "fold version",
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
            remaining: 0,
        });
        assert_eq!(
            line,
            r#"{"complete":true,"pages":3,"threads":250,"messages":4100,"skipped":0,"remaining":0}"#
        );
        assert!(!line.contains('\n'));
    }

    #[test]
    fn the_startup_warning_names_the_count_and_the_command_and_says_nothing_for_none() {
        assert_eq!(unindexed_warning(0), None);
        let one = unindexed_warning(1).unwrap();
        assert!(one.contains("1 thread are"), "{one}");
        let many = unindexed_warning(42).unwrap();
        assert!(many.contains("42 threads"), "{many}");
        assert!(many.contains("orchestrator search backfill"), "{many}");
        let capped = unindexed_warning(BACKLOG_CAP).unwrap();
        assert!(capped.contains("100000 or more threads"), "{capped}");
    }

    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// A store with old threads logs one WARN line at startup with the count and the command; a store the
    /// index is through logs nothing.
    #[tokio::test(flavor = "current_thread")]
    async fn a_store_with_unindexed_threads_logs_one_warn_line_naming_the_command() {
        use orch_core::{
            Actor, AgentId, AgentTarget, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
        };
        use orch_ports::memory::MemoryStore;
        use orch_ports::{Commit, NewEvent, NewThreadRecord, ThreadStore as _};

        let store = MemoryStore::new();
        let alice = UserId::new("alice@example.com");
        let now: jiff::Timestamp = "2026-01-15T10:00:00Z".parse().unwrap();
        for n in 1..=3_u128 {
            store
                .create_thread(
                    NewThreadRecord {
                        id: ThreadId(uuid::Uuid::from_u128(
                            0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
                        )),
                        owner: alice.clone(),
                        title: "Old".to_owned(),
                        description: None,
                        target: AgentTarget {
                            agent_id: AgentId::new("coder"),
                            release: None,
                        },
                        context_id: None,
                        rail_parent: None,
                        child_of: None,
                        now,
                    },
                    Commit {
                        new_state: ThreadState::Done,
                        job: None,
                        events: vec![NewEvent {
                            at: now,
                            actor: Actor::user(&alice),
                            body: EventBody::UserMessage(UserMessageData::new("hello")),
                            idempotency_key: None,
                        }],
                        outbox: Vec::new(),
                        binding: None,
                        now,
                        lease: None,
                        watches: Vec::new(),
                        timers: Vec::new(),
                        inbox: None,
                        finishes_outbox: None,
                        title: None,
                        description: None,
                        sharing: None,
                        skip_unsent_delegates: false,
                    },
                )
                .await
                .unwrap();
        }
        let captured = Captured::default();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(captured.clone())
                .with_ansi(false)
                .finish(),
        );
        warn_when_unindexed(&store).await;
        let quiet = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(quiet.is_empty(), "an indexed store says nothing: {quiet}");

        store.forget_search_index();
        warn_when_unindexed(&store).await;
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("WARN"), "{log}");
        assert!(log.contains("unindexed_threads=3"), "{log}");
        assert!(log.contains("orchestrator search backfill"), "{log}");
        assert_eq!(store.search_backlog(BACKLOG_CAP).await.unwrap(), 3);
        store.backfill_search(None, 10, false).await.unwrap();
        assert_eq!(store.search_backlog(BACKLOG_CAP).await.unwrap(), 0);
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
