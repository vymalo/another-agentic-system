//! ANR-394: the `chat` folder's first model request, measured the way adam-rs measures its own
//! shipped folders, and held to the owner's **5,000-token** budget for a lean agent (adam-rs
//! ADR 0066).
//!
//! The reference is adam-rs at the pinned revision (`5883708`),
//! `bin/adam-agent/tests/agent.rs`, `a_folder_agents_first_request_stays_under_the_lean_budget`:
//! build the agent from its folder, push one "Hi." into a scripted model, and read the **first
//! request's `system` + `tools`** bytes, each tool schema serialised as the OpenAI wire wraps it.
//! The token estimate is `chars / 4`, the repository's own (`cl100k` is about 6 % smaller and CI has
//! no tokenizer), so the pin is in bytes. The message is not counted: it is the same "Hi." in every
//! measurement.
//!
//! **What this measures.** The `chat` folder as `dev/agents/chat/agent/` is served: the root's
//! instructions with its vars rendered, the subagent catalog, the skills catalog (the chat has
//! none), the words of the default mode (`modes/direct.md`, in force because the message names no
//! mode), the sentence the loop appends about `after_previous`, and the tools the root is offered.
//! The root has no `tools:` of its own, so `bind` gives it every registered tool: the six of the
//! person's screen (`ask_user`, `show`, `ui_catalog`, `plan_work`, `plan_check`, `plan_add`), one
//! per helper (`planner`, `writer`, `researcher`) and the five session tools of a root that has a
//! helper.
//!
//! **What it does not measure**, and why the number is a floor:
//!
//! * the deployed `adam-agent` also registers the thread-tools **source**
//!   (`Ui::new(policy).source()`), which can describe `show` with the components of the
//!   conversation's screen and offer a person's attached MCP tools at run time (ADR 0046). Neither
//!   is the folder's own text, and both are per conversation; here the six screen tools are the
//!   ones `Ui::embedded()` makes, with the same specs;
//! * the researcher's `mcp.json` is stood in for by [`Search`], one tool of a trivial schema. It
//!   is the researcher's tool, not the root's, so it does not move this number; the stand-in is
//!   only there because `bind` refuses an agent whose `mcp.json` lists servers that were given no
//!   tools (`McpNotConnected`, fail closed) and this test reaches no network and no database.
//!
//! **Tool groups (ANR-394, item 2).** Nothing to declare. `tools_loading` defaults to `lazy` and
//! the chat sets nothing, and a group is made from an `mcp.json` server with a `when`: the chat's
//! root has no `mcp.json`, so it has no group. The tools of the first request are not group
//! members, so `tools_loading: lazy` does not shrink them — adding the key would be a no-op.
//!
//! [`BASELINE_BYTES`] is not recorded yet: this host cannot compile, so the growth pin says nothing
//! until a run prints the total and the constant is set in the same commit. The budget assert is
//! live.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests assert by unwrapping

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a2a::{Message, Part, Role, Task, TaskState};
use adam_a2a::{Caller, TaskBackend as _};
use adam_a2a_runtime::RuntimeTaskBackend;
use adam_core::{DynStore, MemoryStore};
use adam_engine::adam_assembly::AgentFolder;
use adam_engine::adam_llm_agent::{Tool, ToolCtx, ToolError, ToolOutput, ToolSet};
use adam_engine::adam_model::{DynModel, MockModel, ModelRequest, ToolSpec};
use adam_engine::adam_ui::Ui;
use adam_runtime::{BroadcastSink, Runtime};
use async_trait::async_trait;
use serde_json::{Value, json};

/// The measured total of the first request when this test was added, in bytes: the growth pin.
///
/// `None` until a run records it (this host cannot compile). A deliberate growth updates it in the
/// same commit; anything else is the regression this test exists for.
const BASELINE_BYTES: Option<usize> = None;

/// How much the first request may grow over [`BASELINE_BYTES`] before a reviewer is asked, in bytes
/// (adam's own allowance): a paragraph, a description, one small tool.
const GROWTH_ALLOWED: usize = 1024;

/// The owner's budget for a lean agent, in tokens (adam-rs ADR 0066), counted at `chars / 4`.
const LEAN_BUDGET_TOKENS: usize = 5000;

/// The repository root, from this crate's manifest directory.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate is three levels below the repository root")
        .to_path_buf()
}

/// The folder as `dev/` mounts it, and the chart's copy of the same files.
fn folders() -> (PathBuf, PathBuf) {
    let repo = repo();
    (
        repo.join("dev/agents/chat"),
        repo.join("deploy/chart/files/chat"),
    )
}

/// The researcher's one MCP tool (`search__web_search`, the server `search` of its `mcp.json`), as
/// a stand-in for the search pod the deployment renders: `bind` refuses a folder that names MCP
/// servers and was given no tools, and this test talks to no network.
struct Search;

#[async_trait]
impl Tool for Search {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search__web_search".to_owned(),
            description: "Search the web and answer with sources.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            }),
        }
    }

    async fn call(&self, _ctx: &ToolCtx, _args: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(""))
    }
}

/// The first request's bytes: the system prompt plus each tool schema as the OpenAI wire wraps it.
/// Copied from adam-rs's S0 test so the two numbers are comparable.
fn first_request_bytes(req: &ModelRequest) -> (usize, usize) {
    let system = req.system.as_deref().map_or(0, str::len);
    let tools = req
        .tools
        .iter()
        .map(|t| {
            serde_json::to_vec(&json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            }))
            .expect("a tool schema serialises")
            .len()
        })
        .sum();
    (system, tools)
}

/// Poll the durable task until it ends.
async fn wait_for(backend: &RuntimeTaskBackend, caller: &Caller, id: &str) -> Task {
    for _ in 0..1000 {
        let task = backend
            .get(caller, id)
            .await
            .expect("get")
            .expect("the task exists");
        if task.status.state.is_terminal() {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task {id} never ended");
}

/// ANR-394: the chat folder's first model request is under the lean budget, and the print says by
/// how much so a change that eats the margin is visible before it fails.
#[tokio::test]
async fn the_chats_first_request_stays_under_the_lean_budget() {
    let (folder, _) = folders();
    let def = AgentFolder::load(folder.as_path())
        .expect("the chat folder loads")
        .def;
    let name = def.name().to_owned();
    let researcher = format!("{name}/researcher");

    let mock = Arc::new(MockModel::new());
    mock.push_text("Hi.");
    let model: DynModel = mock.clone();

    // What `adam_agent::agents::build` does for a worker, minus the MCP connection (see the module
    // doc) and minus the A2A card: bind the screen's tools, give every agent the model.
    let ui = Ui::embedded();
    let assembly = def
        .mcp_tools(researcher, ToolSet::new().tool(Search))
        .bind(ui.tools())
        .expect("the chat binds: its helpers, its modes and the screen's tools")
        .max_parallel(10)
        .model(model, "test-model")
        .expect("the chat takes its model");

    let store: DynStore = Arc::new(MemoryStore::new());
    let runtime = assembly
        .register(Runtime::builder(store).poll_interval(Duration::from_millis(10)))
        .build();
    let backend = RuntimeTaskBackend::new(runtime.clone(), BroadcastSink::default(), name);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let worker = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = runtime
                .run_worker(async {
                    let _ = stopped.await;
                })
                .await;
        })
    };

    let caller = Caller::new("token-0");
    let task = backend
        .submit(
            caller.clone(),
            Message::new(Role::User, vec![Part::text("Hi.")]),
            None,
            None,
        )
        .await
        .expect("the message is accepted");
    let task = wait_for(&backend, &caller, &task.id).await;
    assert_eq!(task.status.state, TaskState::Completed, "{task:?}");

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("the worker stops")
        .expect("the worker task does not panic");

    let requests = mock.requests();
    assert!(
        !requests.is_empty(),
        "the chat asked the model nothing: {task:?}"
    );
    let (system, tools) = first_request_bytes(&requests[0]);
    let total = system + tools;
    let tokens = total / 4;
    println!(
        "chat folder first request: system {system} B + tools {tools} B = {total} B \
         (~{tokens} tokens at chars/4); lean budget {LEAN_BUDGET_TOKENS} tokens (~{} B), \
         margin {} tokens; baseline {}, allowed +{GROWTH_ALLOWED} B",
        LEAN_BUDGET_TOKENS * 4,
        LEAN_BUDGET_TOKENS.saturating_sub(tokens),
        BASELINE_BYTES.map_or_else(
            || "not recorded yet: set BASELINE_BYTES to this total".to_owned(),
            |baseline| format!("{baseline} B"),
        ),
    );
    assert!(
        tokens <= LEAN_BUDGET_TOKENS,
        "the chat folder's first request is ~{tokens} tokens (chars/4), over the \
         {LEAN_BUDGET_TOKENS}-token budget for a lean agent (adam-rs ADR 0066): \
         system {system} B + tools {tools} B = {total} B"
    );
    if let Some(baseline) = BASELINE_BYTES {
        assert!(
            total <= baseline + GROWTH_ALLOWED,
            "the chat folder's first request grew to {total} B (baseline {baseline} B, allowed \
             +{GROWTH_ALLOWED} B). A deliberate growth updates the baseline in the same commit; \
             anything else is the regression this test exists for."
        );
    }
}

/// Every file under `root`, as paths relative to it, sorted.
fn files(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .expect("a file under the root")
                        .to_path_buf(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// The chart serves a copy of this folder, flattened into ConfigMap keys by
/// `deploy/chart/templates/chat.yaml`. `deploy/chart/tests/render-check.sh` compares the two too
/// (`deploy.yml`); this test says the same where a `cargo test` sees it, so a change to the folder
/// that forgets the chart fails here first.
///
/// The one file the chart renders instead of copying is the researcher's `mcp.json`: its server,
/// its URL and its bearer are the deployment's (`agentic.chat.researcherMcp`). So that path is
/// compared by presence, and it must be absent from the chart's copy.
#[test]
fn the_charts_copy_of_the_chat_folder_is_equal() {
    let (dev, chart) = folders();
    let dev = dev.join("agent");
    let rendered = Path::new("subagents/researcher/mcp.json");

    let dev_files: Vec<PathBuf> = files(&dev)
        .into_iter()
        .filter(|path| path.as_path() != rendered)
        .collect();
    assert_eq!(
        dev_files,
        files(&chart),
        "the chart copies the chat folder file for file"
    );
    for path in &dev_files {
        let wanted = std::fs::read(dev.join(path)).expect("the folder's file is readable");
        let copied = std::fs::read(chart.join(path)).expect("the chart's copy is readable");
        assert_eq!(
            wanted,
            copied,
            "`{}` differs between `dev/agents/chat/agent/` and `deploy/chart/files/chat/`",
            path.display()
        );
    }

    assert!(
        dev.join(rendered).exists(),
        "the chat's researcher names its MCP server"
    );
    assert!(
        !chart.join(rendered).exists(),
        "the chart renders the researcher's `mcp.json`, it does not copy it"
    );
}
