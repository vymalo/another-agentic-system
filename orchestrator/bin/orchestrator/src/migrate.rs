//! The subcommand `orchestrator artifacts migrate --from fs --to s3 --fs-root <dir> [--dry-run]`
//! (ADR 0058 A5.4): copies the files of the directory store, the volume the chart keeps when
//! `orchestrator.artifacts.store` becomes `s3`, into the configured bucket, under the same keys,
//! so the references in the event log stay valid. The work is in [`orch_artifacts_fs::migrate`];
//! this module is the command line, the configuration, the report and the exit code.
//!
//! It loads **the store of the configuration file and nothing else** ([`crate::config::load_store`]):
//! no database URL, no agents file, no key of another purpose is read, and Postgres is never opened.
//! The source is opened read-only (the Job mounts the claim read-only), nothing is deleted from
//! either side, and the logs go to **stderr** so that stdout is the report: one JSON object.
//!
//! Exit codes: `0` every file is in the bucket with the right size (on a dry run: nothing failed);
//! `2` a usage error (clap); `65` a file is damaged or cannot be migrated, whatever else failed;
//! `66` the directory cannot be read; `69` the bucket failed on a file and nothing else did, so
//! running it again resumes; `78` the configuration.

use clap::{Args as ClapArgs, Subcommand, ValueEnum};

/// The commands of the binary. No command means: serve.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Operations on the files agents hand over (ADR 0032).
    Artifacts(ArtifactsArgs),
}

/// `orchestrator artifacts ...`
#[derive(Debug, ClapArgs)]
pub struct ArtifactsArgs {
    #[command(subcommand)]
    pub command: ArtifactsCommand,
}

/// `orchestrator artifacts <command>`
#[derive(Debug, Subcommand)]
pub enum ArtifactsCommand {
    /// Copy the files of a directory store into the S3 bucket of the configuration (ADR 0058 A5.4).
    ///
    /// Reads the directory at --fs-root (read-only), vets each file (its bytes hash to its key, its
    /// meta passes the store's check) and puts what the bucket does not already hold under the same
    /// key, with its type and name as the object's metadata. A second run skips what is there; a
    /// run that stopped is finished by running it again. Nothing is deleted from either side.
    ///
    /// Reads the configuration file (ORCH_CONFIG_FILE or --config), which must say `artifacts.store:
    /// s3`; only that section and its two credentials are read. Logs go to stderr; stdout is a JSON
    /// report. Exit 0 only when every file is in the bucket with the right size (with --dry-run: when
    /// none failed); 65 a file is damaged or cannot be migrated; 66 the directory cannot be read; 69
    /// the bucket failed; 78 the configuration.
    Migrate(MigrateArgs),
}

/// `orchestrator artifacts migrate ...`
#[derive(Debug, Clone, ClapArgs)]
pub struct MigrateArgs {
    /// What to read: `fs`, the directory store.
    #[arg(long, value_enum, value_name = "STORE")]
    pub from: Source,

    /// What to write: `s3`, the bucket of `artifacts.s3` in the configuration.
    #[arg(long, value_enum, value_name = "STORE")]
    pub to: Target,

    /// The directory of the directory store: the volume `artifacts.fs.root` used to be. Opened
    /// read-only; nothing is created in it.
    #[arg(long, value_name = "DIR")]
    pub fs_root: std::path::PathBuf,

    /// Read, check and ask the bucket, and write nothing: say what would be put.
    #[arg(long)]
    pub dry_run: bool,
}

/// The stores a migration reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Source {
    /// The directory store (`artifacts.store: fs`).
    Fs,
}

/// The stores a migration writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// The S3 store (`artifacts.store: s3`).
    S3,
}

#[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
pub use command::run_command;

#[cfg(not(all(feature = "artifacts-fs", feature = "artifacts-s3")))]
pub use missing::run_command;

/// A build without the two stores cannot migrate between them: a configuration error, as for any
/// store the build lacks (ADR 0032).
#[cfg(not(all(feature = "artifacts-fs", feature = "artifacts-s3")))]
mod missing {
    use std::process::ExitCode;

    use clap::ArgMatches;

    use super::Command;
    use crate::config::Args;

    pub async fn run_command(_command: Command, _args: Args, _matches: &ArgMatches) -> ExitCode {
        eprintln!(
            "configuration error: artifacts migrate needs the Cargo features \"artifacts-fs\" and \
             \"artifacts-s3\", which this build does not have (ADR 0032)"
        );
        ExitCode::from(crate::EX_CONFIG)
    }
}

#[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
mod command {
    use std::process::ExitCode;

    use clap::error::ErrorKind;
    use clap::parser::ValueSource;
    use clap::{ArgMatches, CommandFactory as _};
    use orch_artifacts_fs::{
        Failure, FailureKind, FsArtifacts, Ignored, Options, Outcome, Report, migrate,
    };
    use serde::Serialize;

    use super::{ArtifactsArgs, ArtifactsCommand, Command, MigrateArgs};
    use crate::artifacts::{ArtifactSettings, ConfiguredArtifacts};
    use crate::config::{self, Args, LogFormat};
    use crate::{EX_CONFIG, EX_UNAVAILABLE, logging, report};

    /// sysexits.h: a file is damaged or cannot be migrated.
    const EX_DATAERR: u8 = 65;
    /// sysexits.h: the directory cannot be read.
    const EX_NOINPUT: u8 = 66;

    /// At most this many ignored entries are listed in the report (all are counted, and logged).
    const MAX_LISTED_IGNORED: usize = 100;

    /// Runs the command and returns the exit code. Usage errors exit 2 through clap.
    pub async fn run_command(command: Command, args: Args, matches: &ArgMatches) -> ExitCode {
        let Command::Artifacts(ArtifactsArgs {
            command: ArtifactsCommand::Migrate(migrate_args),
        }) = command;
        refuse_service_flags(matches);

        let store = match config::load_store(&args) {
            Ok(store) => store,
            Err(e) => {
                report(&e);
                return ExitCode::from(EX_CONFIG);
            }
        };
        // A flag or variable wins over the file, as it does for the service.
        let format = args
            .log_format
            .as_deref()
            .map_or(store.log_format, |v| LogFormat::parse(Some(v)));
        logging::init_stderr(format);

        match run(&store.settings, &migrate_args).await {
            Ok(report) => {
                tracing::info!(
                    files = report.files,
                    uploaded = report.uploaded,
                    skipped = report.skipped,
                    would_upload = report.would_upload,
                    failed = report.failed.len(),
                    not_attempted = report.not_attempted,
                    dry_run = report.dry_run,
                    "migration finished"
                );
                // after the last log line, so that the report is the last line of a log that merges both streams
                println!("{}", summary(&report));
                ExitCode::from(exit_code(&report))
            }
            Err(Stop::Config(reason)) => {
                eprintln!("configuration error: {reason}");
                ExitCode::from(EX_CONFIG)
            }
            Err(Stop::Directory(reason)) => {
                eprintln!("error: {reason}");
                ExitCode::from(EX_NOINPUT)
            }
        }
    }

    /// Why a run did not get to a report.
    #[derive(Debug)]
    pub enum Stop {
        /// The store cannot be built from the configuration.
        Config(String),
        /// The directory cannot be opened or listed.
        Directory(String),
    }

    /// The migration itself: open both sides, copy, return the report.
    pub async fn run(settings: &ArtifactSettings, args: &MigrateArgs) -> Result<Report, Stop> {
        let to = ConfiguredArtifacts::build(Some(settings))
            .await
            .map_err(|e| Stop::Config(e.to_string()))?;
        let from = FsArtifacts::open_read_only(&args.fs_root)
            .await
            .map_err(|e| Stop::Directory(e.to_string()))?;
        tracing::info!(
            dry_run = args.dry_run,
            max_file_bytes = settings.max_file_bytes,
            "migrating the files of the directory store into the bucket"
        );
        let options = Options {
            dry_run: args.dry_run,
            max_file_bytes: settings.max_file_bytes,
        };
        let report = migrate(&from, &to, options, log_outcome)
            .await
            .map_err(|e| Stop::Directory(e.to_string()))?;
        for Ignored { path, why } in &report.ignored {
            tracing::warn!(%path, %why, "not a file of the store: ignored");
        }
        Ok(report)
    }

    fn log_outcome(key: &orch_ports::ArtifactKey, outcome: &Outcome) {
        match outcome {
            Outcome::Uploaded { bytes, replaced } => {
                tracing::info!(%key, bytes, replaced, "uploaded");
            }
            Outcome::WouldUpload { bytes, replaced } => {
                tracing::info!(%key, bytes, replaced, "would upload");
            }
            Outcome::Skipped { bytes } => tracing::debug!(%key, bytes, "already in the bucket"),
            Outcome::Failed(Failure { kind, reason, .. }) => {
                tracing::warn!(%key, %kind, %reason, "not migrated");
            }
            // a new kind of outcome of the library is worth a line, not a panic
            other => tracing::info!(%key, ?other, "migrated"),
        }
    }

    /// The exit code of a finished run: `0` when every file is in the bucket (on a dry run: when
    /// none failed), `65` when a file is damaged or cannot be migrated (a person has to look,
    /// whatever else failed), `69` when the bucket alone failed (running it again resumes).
    pub fn exit_code(report: &Report) -> u8 {
        if report.is_complete() {
            0
        } else if report.failed.iter().any(|f| f.kind != FailureKind::Target) {
            EX_DATAERR
        } else {
            EX_UNAVAILABLE
        }
    }

    /// The flags of the service mean nothing here and must not be taken for something that works:
    /// one given on the command line is a usage error. The flags the command shares (`--config` and
    /// `--log-format`, the `global` ones) are fine anywhere; a variable the environment sets is not
    /// a mistake either, a Job's environment is the orchestrator's.
    ///
    /// The candidates are the arguments the struct declares, not `ArgMatches::ids()`: clap also
    /// reports the group the derive makes of the struct (`Args`), whose source is the command line
    /// as soon as any flag is.
    fn refuse_service_flags(matches: &ArgMatches) {
        let given: Vec<String> = Args::command()
            .get_arguments()
            .filter(|arg| !arg.is_global_set())
            .filter(|arg| {
                matches.value_source(arg.get_id().as_str()) == Some(ValueSource::CommandLine)
            })
            .map(|arg| {
                arg.get_long()
                    .map_or_else(|| arg.get_id().to_string(), |long| format!("--{long}"))
            })
            .collect();
        if !given.is_empty() {
            Args::command()
                .error(
                    ErrorKind::ArgumentConflict,
                    format!(
                        "{} belongs to the service and means nothing to `artifacts migrate` \
                         (only --config and --log-format are shared)",
                        given.join(", ")
                    ),
                )
                .exit();
        }
    }

    /// The report: one JSON object on one line, for the Job's log and for a script.
    pub fn summary(report: &Report) -> String {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct FailureLine<'a> {
            key: String,
            kind: &'static str,
            reason: &'a str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct IgnoredLine<'a> {
            path: &'a str,
            why: &'static str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Summary<'a> {
            dry_run: bool,
            complete: bool,
            files: u64,
            uploaded: u64,
            replaced: u64,
            skipped: u64,
            would_upload: u64,
            failed: usize,
            not_attempted: u64,
            ignored: usize,
            bytes_uploaded: u64,
            bytes_skipped: u64,
            bytes_would_upload: u64,
            failures: Vec<FailureLine<'a>>,
            ignored_entries: Vec<IgnoredLine<'a>>,
        }
        let line = Summary {
            dry_run: report.dry_run,
            complete: report.is_complete(),
            files: report.files,
            uploaded: report.uploaded,
            replaced: report.replaced,
            skipped: report.skipped,
            would_upload: report.would_upload,
            failed: report.failed.len(),
            not_attempted: report.not_attempted,
            ignored: report.ignored.len(),
            bytes_uploaded: report.bytes_uploaded,
            bytes_skipped: report.bytes_skipped,
            bytes_would_upload: report.bytes_would_upload,
            failures: report
                .failed
                .iter()
                .map(|f| FailureLine {
                    key: f.key.to_string(),
                    kind: f.kind.as_str(),
                    reason: &f.reason,
                })
                .collect(),
            ignored_entries: report
                .ignored
                .iter()
                .take(MAX_LISTED_IGNORED)
                .map(|i| IgnoredLine {
                    path: &i.path,
                    why: i.why.as_str(),
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

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        let matches = Args::command().try_get_matches_from(args)?;
        Args::from_arg_matches(&matches)
    }

    const MIGRATE: [&str; 8] = [
        "orchestrator",
        "artifacts",
        "migrate",
        "--from",
        "fs",
        "--to",
        "s3",
        "--fs-root",
    ];

    fn migrate_args(extra: &[&str]) -> Vec<String> {
        MIGRATE
            .iter()
            .copied()
            .chain(["/var/lib/orchestrator/artifacts"])
            .chain(extra.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    fn parse_owned(args: &[String]) -> Result<Args, clap::Error> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        parse(&refs)
    }

    fn migrate_of(args: Args) -> MigrateArgs {
        let Some(Command::Artifacts(ArtifactsArgs {
            command: ArtifactsCommand::Migrate(migrate),
        })) = args.command
        else {
            panic!("not the migrate command");
        };
        migrate
    }

    #[test]
    fn no_command_is_the_service_and_its_flags_are_what_they_were() {
        let args = parse(&["orchestrator"]).unwrap();
        assert!(args.command.is_none());
        let args = parse(&[
            "orchestrator",
            "--listen-addr",
            "127.0.0.1:9",
            "--role",
            "worker",
            "--print-config",
        ])
        .unwrap();
        assert!(args.command.is_none());
        assert_eq!(args.listen_addr.as_deref(), Some("127.0.0.1:9"));
        assert_eq!(args.role.as_deref(), Some("worker"));
        assert!(args.print_config);
    }

    #[test]
    fn the_command_reads_its_flags() {
        let migrate = migrate_of(parse_owned(&migrate_args(&[])).unwrap());
        assert_eq!(migrate.from, Source::Fs);
        assert_eq!(migrate.to, Target::S3);
        assert_eq!(
            migrate.fs_root,
            std::path::Path::new("/var/lib/orchestrator/artifacts")
        );
        assert!(!migrate.dry_run);
        let migrate = migrate_of(parse_owned(&migrate_args(&["--dry-run"])).unwrap());
        assert!(migrate.dry_run);
    }

    #[test]
    fn the_configuration_file_may_come_before_the_command_or_after_it() {
        let before = parse(&[
            "orchestrator",
            "--config",
            "/etc/a.yaml",
            "artifacts",
            "migrate",
            "--from",
            "fs",
            "--to",
            "s3",
            "--fs-root",
            "/x",
        ])
        .unwrap();
        assert_eq!(before.config.as_deref(), Some("/etc/a.yaml"));
        let mut after = migrate_args(&[]);
        after.extend(["--config".to_owned(), "/etc/b.yaml".to_owned()]);
        after.extend(["--log-format".to_owned(), "text".to_owned()]);
        let after = parse_owned(&after).unwrap();
        assert_eq!(after.config.as_deref(), Some("/etc/b.yaml"));
        assert_eq!(after.log_format.as_deref(), Some("text"));
        // a flag of the service is not shared: after the command it is not a flag at all
        assert!(parse_owned(&migrate_args(&["--listen-addr", "127.0.0.1:1"])).is_err());
    }

    #[test]
    fn anything_else_is_a_usage_error_that_exits_2() {
        let refused = |args: &[&str]| {
            let err = parse(args).expect_err(&format!("{args:?}"));
            assert_eq!(err.exit_code(), 2, "{args:?}: {err}");
            err.to_string()
        };
        // each of the three required flags
        refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--to",
            "s3",
            "--fs-root",
            "/x",
        ]);
        refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--from",
            "fs",
            "--fs-root",
            "/x",
        ]);
        refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--from",
            "fs",
            "--to",
            "s3",
        ]);
        // the only values are fs and s3, and the error says so
        let said = refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--from",
            "s3",
            "--to",
            "s3",
            "--fs-root",
            "/x",
        ]);
        assert!(said.contains("possible values: fs"), "{said}");
        let said = refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--from",
            "fs",
            "--to",
            "fs",
            "--fs-root",
            "/x",
        ]);
        assert!(said.contains("possible values: s3"), "{said}");
        refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--from",
            "gcs",
            "--to",
            "s3",
            "--fs-root",
            "/x",
        ]);
        // a command that is not one, and one that is half of one
        refused(&["orchestrator", "artifacts"]);
        refused(&["orchestrator", "migrate"]);
        refused(&["orchestrator", "artifacts", "frobnicate"]);
        // an unknown flag of the command
        refused(&[
            "orchestrator",
            "artifacts",
            "migrate",
            "--from",
            "fs",
            "--to",
            "s3",
            "--fs-root",
            "/x",
            "--delete-source",
        ]);
    }

    #[test]
    fn help_for_the_command_says_what_it_does() {
        let err = parse(&["orchestrator", "artifacts", "migrate", "--help"]).unwrap_err();
        assert_eq!(err.exit_code(), 0);
        let help = err.to_string();
        for needle in [
            "--from",
            "--to",
            "--fs-root",
            "--dry-run",
            "read-only",
            "65",
            "69",
            "78",
        ] {
            assert!(help.contains(needle), "{needle}: {help}");
        }
        let top = Args::command().render_help().to_string();
        assert!(
            top.contains("artifacts"),
            "the service's help lists the command: {top}"
        );
    }

    #[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
    mod the_report {
        use orch_artifacts_fs::{Failure, FailureKind, Ignored, IgnoredWhy, Report};
        use orch_core::ThreadId;
        use orch_ports::ArtifactKey;

        use super::super::command::{exit_code, summary};

        fn key(n: u128) -> ArtifactKey {
            ArtifactKey::new(ThreadId(uuid::Uuid::from_u128(n)), [n as u8; 32])
        }

        fn failure(kind: FailureKind) -> Failure {
            Failure {
                key: key(1),
                kind,
                reason: "a reason".to_owned(),
            }
        }

        #[test]
        fn exit_zero_only_when_every_file_is_in_the_bucket() {
            let done = Report {
                files: 3,
                uploaded: 2,
                skipped: 1,
                ..Report::default()
            };
            assert_eq!(exit_code(&done), 0);
            let nothing = Report::default();
            assert_eq!(
                exit_code(&nothing),
                0,
                "a directory with no file is nothing to do"
            );
            // a dry run that could do it all
            let dry = Report {
                dry_run: true,
                files: 2,
                would_upload: 2,
                ..Report::default()
            };
            assert_eq!(exit_code(&dry), 0);
        }

        #[test]
        fn a_damaged_file_is_65_whatever_else_failed_and_a_bucket_alone_is_69() {
            for kind in [
                FailureKind::Corrupt,
                FailureKind::Invalid,
                FailureKind::TooLarge,
                FailureKind::Unreadable,
            ] {
                let report = Report {
                    files: 1,
                    failed: vec![failure(kind)],
                    ..Report::default()
                };
                assert_eq!(exit_code(&report), 65, "{kind}");
            }
            let bucket = Report {
                files: 9,
                failed: vec![failure(FailureKind::Target); 5],
                not_attempted: 4,
                ..Report::default()
            };
            assert_eq!(exit_code(&bucket), 69);
            let both = Report {
                files: 2,
                failed: vec![failure(FailureKind::Target), failure(FailureKind::Corrupt)],
                ..Report::default()
            };
            assert_eq!(exit_code(&both), 65);
            // files left untried are not in the bucket: not a success
            let stopped = Report {
                files: 2,
                not_attempted: 1,
                uploaded: 1,
                ..Report::default()
            };
            assert_eq!(exit_code(&stopped), 69);
        }

        #[test]
        fn the_summary_is_one_line_of_json_that_names_each_failure_by_key_and_reason() {
            let report = Report {
                files: 3,
                uploaded: 1,
                skipped: 1,
                bytes_uploaded: 10,
                bytes_skipped: 4,
                failed: vec![failure(FailureKind::Corrupt)],
                ignored: vec![Ignored {
                    path: "threads/x/.tmp-1".to_owned(),
                    why: IgnoredWhy::Temporary,
                }],
                ..Report::default()
            };
            let line = summary(&report);
            assert!(!line.contains('\n'), "one line");
            let json: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(json["files"], 3);
            assert_eq!(json["uploaded"], 1);
            assert_eq!(json["skipped"], 1);
            assert_eq!(json["failed"], 1);
            assert_eq!(json["bytesUploaded"], 10);
            assert_eq!(json["complete"], false);
            assert_eq!(json["dryRun"], false);
            assert_eq!(json["failures"][0]["key"], key(1).to_string());
            assert_eq!(json["failures"][0]["kind"], "corrupt");
            assert_eq!(json["failures"][0]["reason"], "a reason");
            assert_eq!(json["ignored"], 1);
            assert_eq!(json["ignoredEntries"][0]["path"], "threads/x/.tmp-1");
        }

        #[test]
        fn a_long_list_of_ignored_entries_is_counted_whole_and_listed_in_part() {
            let report = Report {
                ignored: (0..250)
                    .map(|n| Ignored {
                        path: format!("threads/x/{n}"),
                        why: IgnoredWhy::Unrecognized,
                    })
                    .collect(),
                ..Report::default()
            };
            let json: serde_json::Value = serde_json::from_str(&summary(&report)).unwrap();
            assert_eq!(json["ignored"], 250);
            assert_eq!(json["ignoredEntries"].as_array().unwrap().len(), 100);
        }
    }
}
