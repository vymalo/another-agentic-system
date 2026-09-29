use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::StreamExt;
use orch_core::{AgentId, AgentTaskState, AgentUpdate, Releases};
use tokio::sync::Notify;

use crate::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, IdemKey,
    SendRequest, TaskHandle, TaskSnapshot,
};

/// The URL the scripted agent reports as a produced artifact.
pub const PR_URL: &str = "https://github.com/acme/demo/pull/1";

/// A recorded call on the scripted agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `send_stream`.
    Send {
        /// Agent the request targeted.
        agent: AgentId,
        /// Message id (the outbox row id).
        message_id: String,
        /// A2A context.
        context_id: String,
        /// Task continued, if any.
        task_id: Option<String>,
        /// Text.
        text: String,
        /// Selected release.
        release: Option<String>,
    },
    /// `resubscribe`.
    Resubscribe {
        /// Task.
        task_id: String,
    },
    /// `get_task`.
    GetTask {
        /// Task.
        task_id: String,
    },
    /// `cancel`.
    Cancel {
        /// Task.
        task_id: String,
    },
    /// `find_task_by_message`.
    Find {
        /// Message id searched.
        message_id: String,
    },
    /// `read_card`.
    ReadCard {
        /// Agent.
        agent: AgentId,
    },
}

struct TaskRec {
    context_id: String,
    state: AgentTaskState,
    detail: Option<String>,
    revision: Option<String>,
    log: Vec<AgentEnvelope>,
    message_ids: Vec<String>,
}

#[derive(Default)]
struct State {
    tasks: HashMap<String, TaskRec>,
    next_task: u32,
    calls: Vec<Call>,
    cards: HashMap<AgentId, AgentCardInfo>,
    cards_down: HashSet<AgentId>,
    fail_sends: VecDeque<AgentError>,
    no_resubscribe: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes stream followers and drivers on any change.
    changed: Notify,
    /// Permit-style gate for "gate" and "drop" scripts.
    gate: Notify,
}

/// A fake [`AgentClient`] driven by the message text, for tests. It behaves like a real
/// agent host: task execution continues when the client disconnects, `resubscribe` works only
/// while a turn is live, and follow-ups continue the same task after `input-required`.
///
/// Scripts (first word of the text):
/// - `echo` (and anything else): `working`, an artifact `echo` with a PR URL, `completed`;
/// - `ask`: `working`, `input-required("Which branch?")`; the follow-up continues the task
///   with `working`, an artifact `answer`, `completed`;
/// - `gate`: `working`, then waits for [`ScriptedAgent::release_gate`], then artifact, `completed`;
/// - `drop`: like `gate`, but the initial stream is cut after two envelopes;
/// - `slow`: `working`, then runs until cancelled;
/// - `fail`: `send_stream` fails with `Rejected`; `down`: with `Unreachable`.
#[derive(Clone)]
pub struct ScriptedAgent {
    shared: Arc<Shared>,
}

impl Default for ScriptedAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ScriptedAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScriptedAgent")
    }
}

impl ScriptedAgent {
    /// A scripted agent whose cards have no releases.
    pub fn new() -> Self {
        ScriptedAgent {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                changed: Notify::new(),
                gate: Notify::new(),
            }),
        }
    }

    /// Makes `agent`'s card advertise the release-channels extension.
    pub fn with_releases(self, agent: &str, releases: Releases) -> Self {
        self.state().cards.insert(
            AgentId::new(agent),
            AgentCardInfo {
                description: Some("scripted agent with releases".to_owned()),
                releases: Some(releases),
            },
        );
        self
    }

    /// Makes reading `agent`'s card fail.
    pub fn set_card_down(&self, agent: &str, down: bool) {
        let mut st = self.state();
        if down {
            st.cards_down.insert(AgentId::new(agent));
        } else {
            st.cards_down.remove(&AgentId::new(agent));
        }
    }

    /// The next `n` sends fail with `error`.
    pub fn fail_next_sends(&self, n: usize, error: AgentError) {
        let mut st = self.state();
        for _ in 0..n {
            st.fail_sends.push_back(error.clone());
        }
    }

    /// `false` makes `resubscribe` answer `Unsupported`, forcing the `get_task` polling path.
    pub fn set_resubscribe_supported(&self, supported: bool) {
        self.state().no_resubscribe = !supported;
    }

    /// Lets one waiting `gate`/`drop` task continue (a permit is stored if none waits yet).
    pub fn release_gate(&self) {
        self.shared.gate.notify_one();
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<Call> {
        self.state().calls.clone()
    }

    /// The `send_stream` calls so far.
    pub fn sends(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Send { .. }))
            .collect()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn status_key(task: &str, state: AgentTaskState) -> IdemKey {
    IdemKey::Turn(format!("{task}:status:{state:?}"))
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(
        &self,
        task: &str,
        state: Option<AgentTaskState>,
        detail: Option<String>,
        key: IdemKey,
        update: Option<AgentUpdate>,
    ) {
        {
            let mut st = self.state();
            let Some(rec) = st.tasks.get_mut(task) else {
                return;
            };
            if let Some(s) = state {
                rec.state = s;
                rec.detail = detail;
            }
            let env = AgentEnvelope {
                task_id: task.to_owned(),
                context_id: rec.context_id.clone(),
                task_state: state,
                revision: rec.revision.clone(),
                key,
                update,
            };
            rec.log.push(env);
        }
        self.changed.notify_waiters();
    }

    fn push_status(&self, task: &str, state: AgentTaskState, detail: Option<&str>) {
        self.push(
            task,
            Some(state),
            detail.map(str::to_owned),
            status_key(task, state),
            Some(AgentUpdate::Status {
                state,
                detail: detail.map(str::to_owned),
            }),
        );
    }

    fn push_artifact(&self, task: &str, name: &str, text: String) {
        self.push(
            task,
            None,
            None,
            IdemKey::Task(format!("a2a:{task}:artifact:{name}")),
            Some(AgentUpdate::Artifact {
                name: name.to_owned(),
                mime_type: None,
                uri: Some(PR_URL.to_owned()),
                text: Some(text),
            }),
        );
    }

    /// Waits until `f` holds for the task (checked on every change).
    async fn wait_until(&self, task: &str, f: impl Fn(&TaskRec) -> bool) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let done = self.state().tasks.get(task).is_none_or(&f);
            if done {
                return;
            }
            notified.await;
        }
    }

    /// Follows a task's envelope log from `from`, ending after an envelope that ends the turn
    /// (or after `limit` envelopes, to simulate a dropped connection).
    fn follow(self: &Arc<Self>, task: String, from: usize, limit: Option<usize>) -> AgentStream {
        let shared = Arc::clone(self);
        futures::stream::unfold((from, false, 0usize), move |(idx, done, sent)| {
            let shared = Arc::clone(&shared);
            let task = task.clone();
            async move {
                loop {
                    if done || limit.is_some_and(|l| sent >= l) {
                        return None;
                    }
                    let notified = shared.changed.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    let next = shared
                        .state()
                        .tasks
                        .get(&task)
                        .and_then(|t| t.log.get(idx).cloned());
                    match next {
                        Some(env) => {
                            let ends = env.task_state.is_some_and(AgentTaskState::ends_turn);
                            return Some((Ok(env), (idx + 1, ends, sent + 1)));
                        }
                        None => notified.await,
                    }
                }
            }
        })
        .boxed()
    }
}

async fn drive(shared: Arc<Shared>, task: String, text: String, resumed: bool) {
    use AgentTaskState::{Completed, InputRequired, Working};
    let script = text.split_whitespace().next().unwrap_or("").to_owned();
    shared.push_status(&task, Working, None);
    match script.as_str() {
        "ask" if !resumed => shared.push_status(&task, InputRequired, Some("Which branch?")),
        "ask" => {
            shared.push_artifact(&task, "answer", format!("answered: {text}"));
            shared.push_status(&task, Completed, None);
        }
        "gate" | "drop" => {
            shared.gate.notified().await;
            shared.push_artifact(&task, "echo", format!("echo: {text}"));
            shared.push_status(&task, Completed, None);
        }
        "slow" => {
            shared.wait_until(&task, |t| t.state.is_terminal()).await;
        }
        _ => {
            shared.push_artifact(&task, "echo", format!("echo: {text}"));
            shared.push_status(&task, Completed, None);
        }
    }
}

impl AgentClient for ScriptedAgent {
    async fn read_card(&self, ep: &AgentEndpoint) -> Result<AgentCardInfo, AgentError> {
        let mut st = self.state();
        st.calls.push(Call::ReadCard {
            agent: ep.id.clone(),
        });
        if st.cards_down.contains(&ep.id) {
            return Err(AgentError::Unreachable("card unreachable".to_owned()));
        }
        Ok(st.cards.get(&ep.id).cloned().unwrap_or(AgentCardInfo {
            description: Some("scripted agent".to_owned()),
            releases: None,
        }))
    }

    async fn send_stream(&self, req: SendRequest) -> Result<AgentStream, AgentError> {
        let script = req.text.split_whitespace().next().unwrap_or("").to_owned();
        let (task, resumed, from) = {
            let mut st = self.state();
            st.calls.push(Call::Send {
                agent: req.endpoint.id.clone(),
                message_id: req.message_id.clone(),
                context_id: req.context_id.clone(),
                task_id: req.task_id.clone(),
                text: req.text.clone(),
                release: req.release.clone(),
            });
            if let Some(err) = st.fail_sends.pop_front() {
                return Err(err);
            }
            match script.as_str() {
                "fail" => return Err(AgentError::Rejected("scripted failure".to_owned())),
                "down" => return Err(AgentError::Unreachable("scripted outage".to_owned())),
                _ => {}
            }
            let existing = req
                .task_id
                .as_ref()
                .filter(|id| st.tasks.get(*id).is_some_and(|t| !t.state.is_terminal()))
                .cloned();
            if let Some(id) = existing {
                let rec = st
                    .tasks
                    .get_mut(&id)
                    .ok_or_else(|| AgentError::TaskNotFound(id.clone()))?;
                rec.message_ids.push(req.message_id.clone());
                let from = rec.log.len();
                (id, true, from)
            } else {
                st.next_task += 1;
                let id = format!("task-{}", st.next_task);
                let revision = st.cards.get(&req.endpoint.id).and_then(|c| {
                    c.releases.as_ref().and_then(|r| {
                        let selector = req.release.as_deref().unwrap_or(&r.default_channel);
                        r.resolve(selector)
                    })
                });
                st.tasks.insert(
                    id.clone(),
                    TaskRec {
                        context_id: req.context_id.clone(),
                        state: AgentTaskState::Submitted,
                        detail: None,
                        revision,
                        log: Vec::new(),
                        message_ids: vec![req.message_id.clone()],
                    },
                );
                (id, false, 0)
            }
        };
        if !resumed {
            self.shared
                .push_status(&task, AgentTaskState::Submitted, None);
        }
        // Execution continues even if the client goes away (like a real agent host).
        tokio::spawn(drive(
            Arc::clone(&self.shared),
            task.clone(),
            req.text,
            resumed,
        ));
        let limit = (script == "drop").then_some(2);
        Ok(self.shared.follow(task, from, limit))
    }

    async fn resubscribe(&self, task: &TaskHandle) -> Result<AgentStream, AgentError> {
        let from = {
            let mut st = self.state();
            st.calls.push(Call::Resubscribe {
                task_id: task.task_id.clone(),
            });
            if st.no_resubscribe {
                return Err(AgentError::Unsupported("resubscribe".to_owned()));
            }
            let rec = st
                .tasks
                .get(&task.task_id)
                .ok_or_else(|| AgentError::TaskNotFound(task.task_id.clone()))?;
            if rec.state.ends_turn() {
                return Err(AgentError::TaskNotFound(task.task_id.clone()));
            }
            rec.log.len()
        };
        // Like the real protocol: a snapshot of the current state first, then live updates.
        let snapshot = {
            let st = self.state();
            let rec = st
                .tasks
                .get(&task.task_id)
                .ok_or_else(|| AgentError::TaskNotFound(task.task_id.clone()))?;
            AgentEnvelope {
                task_id: task.task_id.clone(),
                context_id: rec.context_id.clone(),
                task_state: Some(rec.state),
                revision: rec.revision.clone(),
                key: status_key(&task.task_id, rec.state),
                update: Some(AgentUpdate::Status {
                    state: rec.state,
                    detail: rec.detail.clone(),
                }),
            }
        };
        let live = self.shared.follow(task.task_id.clone(), from, None);
        Ok(futures::stream::once(async move { Ok(snapshot) })
            .chain(live)
            .boxed())
    }

    async fn get_task(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let mut st = self.state();
        st.calls.push(Call::GetTask {
            task_id: task.task_id.clone(),
        });
        snapshot(&st, &task.task_id)
    }

    async fn cancel(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        {
            let mut st = self.state();
            st.calls.push(Call::Cancel {
                task_id: task.task_id.clone(),
            });
            let rec = st
                .tasks
                .get(&task.task_id)
                .ok_or_else(|| AgentError::TaskNotFound(task.task_id.clone()))?;
            if rec.state.is_terminal() {
                return Err(AgentError::NotCancelable(format!(
                    "task is {:?}",
                    rec.state
                )));
            }
        }
        self.shared
            .push_status(&task.task_id, AgentTaskState::Canceled, None);
        let st = self.state();
        snapshot(&st, &task.task_id)
    }

    async fn find_task_by_message(
        &self,
        ep: &AgentEndpoint,
        context_id: &str,
        message_id: &str,
    ) -> Result<Option<String>, AgentError> {
        let _ = ep;
        let mut st = self.state();
        st.calls.push(Call::Find {
            message_id: message_id.to_owned(),
        });
        Ok(st
            .tasks
            .iter()
            .find(|(_, t)| {
                t.context_id == context_id && t.message_ids.iter().any(|m| m == message_id)
            })
            .map(|(id, _)| id.clone()))
    }
}

fn snapshot(st: &State, task: &str) -> Result<TaskSnapshot, AgentError> {
    let rec = st
        .tasks
        .get(task)
        .ok_or_else(|| AgentError::TaskNotFound(task.to_owned()))?;
    let mut envelopes: Vec<AgentEnvelope> = rec
        .log
        .iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .cloned()
        .collect();
    envelopes.push(AgentEnvelope {
        task_id: task.to_owned(),
        context_id: rec.context_id.clone(),
        task_state: Some(rec.state),
        revision: rec.revision.clone(),
        key: status_key(task, rec.state),
        update: Some(AgentUpdate::Status {
            state: rec.state,
            detail: rec.detail.clone(),
        }),
    });
    Ok(TaskSnapshot {
        task_id: task.to_owned(),
        context_id: rec.context_id.clone(),
        state: rec.state,
        revision: rec.revision.clone(),
        envelopes,
    })
}
