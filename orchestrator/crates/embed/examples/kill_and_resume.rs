//! A thread with a local adam agent on SQLite files is killed mid-run, restarted, and resumes both
//! the thread and the adam run from their journals, with no step run twice (ADR 0076 slice E4b).
//!
//! The example runs itself as a child process. The child opens the thread log and the adam journal
//! (two SQLite files), starts a thread whose agent runs three journaled steps and appends one line
//! to a log file for each, and is killed (SIGKILL) while the last step waits. A second child opens
//! the same files and finishes the thread. The check is on the log file, which only the agent's
//! steps write: each step's line must be there once.
//!
//! ```sh
//! cargo run -p orch-embed --example kill_and_resume
//! cargo run -p orch-embed --example kill_and_resume -- --no-resume   # fails: the work starts over
//! ```
//!
//! `--no-resume` starts a new thread instead of resuming the killed one, which runs the first steps
//! a second time: the example exits non-zero, which is how the check is seen red.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use orch_agent_adam::LocalOptions;
use orch_agent_adam::testkit::{stepper_agents, stepper_endpoint};
use orch_agui_proto::{Event, RunFinishedOutcome};
use orch_app::{AgentDirectory, AgentEntry};
use orch_core::{AgentId, ThreadId, ThreadState, UserId};
use orch_embed::Embedded;
use tokio_util::sync::CancellationToken;

type Fail = Box<dyn std::error::Error>;

const STEPS: u32 = 3;
const AGENT: &str = "stepper";

#[tokio::main]
async fn main() -> Result<(), Fail> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("child") => child(&args[1..]).await,
        _ => parent(args.iter().any(|a| a == "--no-resume")).await,
    }
}

/// The paths every process of one run shares.
struct Files {
    thread: PathBuf,
    journal: PathBuf,
    log: PathBuf,
    hold: PathBuf,
}

impl Files {
    fn in_dir(dir: &Path) -> Self {
        Files {
            thread: dir.join("thread.sqlite"),
            journal: dir.join("adam.sqlite"),
            log: dir.join("steps.log"),
            hold: dir.join("hold"),
        }
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn parent(no_resume: bool) -> Result<(), Fail> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("orch-resume-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let files = Files::in_dir(&dir);
    let me = std::env::current_exe()?;

    // First process: starts the thread, runs two steps and waits at the third.
    let mut first = Command::new(&me)
        .args(["child", "start"])
        .arg(&dir)
        .stdout(Stdio::piped())
        .spawn()?;
    let thread = read_thread_id(&mut first)?;
    println!("started thread {thread}");
    wait_for("the first two steps", Duration::from_secs(60), || {
        lines(&files.log) == ["0", "1"]
    })?;
    first.kill()?; // SIGKILL: nothing is released, the third step never commits
    first.wait()?;
    println!("killed the process with the log at {:?}", lines(&files.log));

    // Second process: the same files. It lets the third step through and finishes.
    std::fs::write(&files.hold, b"go")?;
    let mut second = Command::new(&me);
    if no_resume {
        second.args(["child", "start"]);
    } else {
        second.args(["child", "resume"]);
    }
    second.arg(&dir);
    if !no_resume {
        second.arg(thread.to_string());
    }
    let output = second.stderr(Stdio::inherit()).output()?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.status.success() {
        return Err(format!("the second process failed: {}", output.status).into());
    }

    let log = lines(&files.log);
    println!("steps run, in order: {log:?}");
    let expected: Vec<String> = (0..STEPS).map(|n| n.to_string()).collect();
    if log != expected {
        return Err(
            format!("each step must run once: wanted {expected:?}, the log has {log:?}").into(),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    println!("ok: every journaled step ran once, and the thread finished");
    Ok(())
}

fn read_thread_id(child: &mut Child) -> Result<ThreadId, Fail> {
    let stdout = child.stdout.take().ok_or("the child has no stdout")?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    let id = line
        .trim()
        .strip_prefix("THREAD ")
        .ok_or_else(|| format!("the child said {line:?}, not a thread"))?;
    Ok(id.parse()?)
}

fn wait_for(what: &str, within: Duration, mut done: impl FnMut() -> bool) -> Result<(), Fail> {
    let deadline = Instant::now() + within;
    while !done() {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// `child start <dir>` or `child resume <dir> <thread>`: one process of the application.
async fn child(args: &[String]) -> Result<(), Fail> {
    let (mode, dir) = (args.first().ok_or("mode")?, args.get(1).ok_or("directory")?);
    let files = Files::in_dir(Path::new(dir));

    // The adam journal: a lease of one second, so a restart takes a dead process's run over quickly.
    let mut options = LocalOptions::new(format!("process-{}", std::process::id()));
    options.lease_ttl = Duration::from_secs(1);
    options.poll_interval = Duration::from_millis(50);
    let agents = std::sync::Arc::new(stepper_agents(&files.journal, options)?);
    agents.migrate().await?;
    let client = agents.client();
    let stop = CancellationToken::new();
    let worker = tokio::spawn({
        let (agents, stop) = (std::sync::Arc::clone(&agents), stop.clone());
        async move { agents.run(stop).await }
    });

    let agent = AgentId::new(AGENT);
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: stepper_endpoint(AGENT),
        name: "Stepper".to_owned(),
    }]);
    let embed = Embedded::open(
        &files.thread,
        UserId::new("local@example.invalid"),
        directory,
        client,
    )
    .await?;

    let thread = match mode.as_str() {
        "start" => {
            let text = format!("{STEPS} {} {}", files.log.display(), files.hold.display());
            let record = embed.create_thread(&agent, text).await?;
            println!("THREAD {}", record.id);
            record.id
        }
        "resume" => args.get(2).ok_or("thread")?.parse::<ThreadId>()?,
        other => return Err(format!("unknown mode {other}").into()),
    };

    let events = tokio::time::timeout(Duration::from_secs(90), embed.collect_agui(thread))
        .await
        .map_err(|_| "the thread did not finish within 90 s")??;
    let finished = events
        .iter()
        .filter(|e| matches!(e, Event::RunFinished(f) if matches!(f.outcome, None | Some(RunFinishedOutcome::Success { .. }))))
        .count();
    let errors = events
        .iter()
        .filter(|e| matches!(e, Event::RunError(_)))
        .count();
    let record = embed.read_thread(thread).await?;
    println!(
        "thread {:?}: {finished} RUN_FINISHED, {errors} RUN_ERROR",
        record.state
    );
    if record.state != ThreadState::Done || finished != 1 || errors != 0 {
        return Err("the thread did not finish cleanly".into());
    }

    embed.shutdown().await;
    stop.cancel();
    worker.await??;
    Ok(())
}
