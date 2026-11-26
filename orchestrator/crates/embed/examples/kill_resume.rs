//! Kills an embedded application in the middle of a run and shows that it resumes: the thread from
//! its log and the local adam run from its journal, each journaled step once (ADR 0076, ADR 0015).
//!
//! The example runs itself three times. The parent starts a child that creates a thread with the
//! scripted agent (five journaled steps, 300 ms apart), kills it with SIGKILL after the second
//! step, and starts a second child on the same two SQLite files. The second child waits for the
//! thread to finish and checks the log. The dispatcher's job lease is 30 s, so the resume takes about
//! 30 s.
//!
//! ```sh
//! cargo run -p orch-embed --example kill_resume
//! KILL_RESUME_DISABLED=1 cargo run -p orch-embed --example kill_resume   # fails: nothing resumes
//! ```

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use orch_agent_adam::testkit::scripted_endpoint;
use orch_agent_adam::{LocalAgents, LocalOptions};
use orch_app::{AgentDirectory, AgentEntry};
use orch_core::{AgentId, EventBody, ThreadId, ThreadState, UserId};
use orch_embed::Embedded;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const STEPS: u32 = 5;
const KILL_AFTER: &str = "step-2";
type Failure = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("first") => first(&args[2]).await,
        Some("resume") => resume(&args[2], args[3].parse()?).await,
        _ => parent().await,
    }
}

/// An application on the two files of `dir`: the thread log and the local agents' journal.
async fn open(
    dir: &str,
    name: &str,
    steps: bool,
) -> Result<
    (
        Embedded<orch_agent_adam::LocalAgentClient>,
        CancellationToken,
    ),
    Failure,
> {
    let mut opts = LocalOptions::new(name);
    opts.lease_ttl = Duration::from_secs(2);
    opts.poll_interval = Duration::from_millis(100);
    opts.steps = steps;
    let agents =
        Arc::new(LocalAgents::sqlite_scripted(format!("{dir}/agents.sqlite"), opts).await?);
    let stop = CancellationToken::new();
    tokio::spawn({
        let (agents, stop) = (agents.clone(), stop.clone());
        async move { agents.run(stop).await }
    });
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: scripted_endpoint("steps"),
        name: "Steps".to_owned(),
    }]);
    let embed = Embedded::open(
        format!("{dir}/orch.sqlite"),
        UserId::new("local@example.invalid"),
        directory,
        agents.client(),
    )
    .await?;
    Ok((embed, stop))
}

/// The names of the artifacts the thread's log holds, in order.
async fn artifacts(
    embed: &Embedded<orch_agent_adam::LocalAgentClient>,
    thread: ThreadId,
) -> Result<Vec<String>, Failure> {
    Ok(embed
        .events(thread)
        .await?
        .iter()
        .filter_map(|event| match &event.body {
            EventBody::Artifact(data) => Some(data.name.clone()),
            _ => None,
        })
        .collect())
}

/// The first process: starts the thread and reports each step as it is logged, until it is killed.
async fn first(dir: &str) -> Result<(), Failure> {
    let (embed, _stop) = open(dir, "first", true).await?;
    let thread = embed
        .create_thread(&AgentId::new("steps"), &format!("steps {STEPS}"))
        .await?;
    println!("THREAD {}", thread.id);
    let mut reported = 0;
    loop {
        let names = artifacts(&embed, thread.id).await?;
        for name in &names[reported..] {
            println!("STEP {name}");
        }
        reported = names.len();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The second process: resumes, and checks that every step is in the log once.
async fn resume(dir: &str, thread: ThreadId) -> Result<(), Failure> {
    // With resume disabled the process starts runs but never steps them: the check below must fail.
    let disabled = std::env::var_os("KILL_RESUME_DISABLED").is_some();
    let (embed, stop) = open(dir, "resume", !disabled).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);
    loop {
        if embed.read_thread(thread).await?.state == ThreadState::Done {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the thread did not finish after the restart".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let names = artifacts(&embed, thread).await?;
    let expected: Vec<String> = (1..=STEPS).map(|n| format!("step-{n}")).collect();
    if names != expected {
        return Err(format!(
            "each journaled step once, in order: expected {expected:?}, the log holds {names:?}"
        )
        .into());
    }
    println!("resumed: {} steps, each once, thread done", names.len());
    stop.cancel();
    embed.shutdown().await;
    Ok(())
}

async fn parent() -> Result<(), Failure> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("orch-kill-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let dir_name = dir.to_string_lossy().into_owned();
    let exe = std::env::current_exe()?;

    let mut child = Command::new(&exe)
        .args(["first", &dir_name])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let mut thread = None;
    let mut killed_after = Vec::new();
    let reading = async {
        while let Some(line) = lines.next_line().await? {
            if let Some(id) = line.strip_prefix("THREAD ") {
                thread = Some(id.to_owned());
            } else if let Some(step) = line.strip_prefix("STEP ") {
                killed_after.push(step.to_owned());
                if step == KILL_AFTER {
                    return Ok::<_, Failure>(());
                }
            }
        }
        Err("the first process ended before its second step".into())
    };
    tokio::time::timeout(Duration::from_secs(30), reading)
        .await
        .map_err(|_| "the first process did not reach its second step")??;
    child.kill().await?; // SIGKILL: no lease released, no shutdown
    println!("killed after {killed_after:?}");
    assert!(killed_after.len() < STEPS as usize, "killed mid-run");
    let thread = thread.ok_or("the first process did not name its thread")?;

    let status = Command::new(&exe)
        .args(["resume", &dir_name, &thread])
        .status()
        .await?;
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() {
        return Err("the second process did not resume the run".into());
    }
    println!("ok");
    Ok(())
}
