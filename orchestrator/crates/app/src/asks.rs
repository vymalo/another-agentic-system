//! `App::ask`: the application's half of the thread tool `ask_agent` (ADR 0026,
//! [`docs/api/thread-tools-v1.md`](../../../../docs/api/thread-tools-v1.md#ask_agent)).
//!
//! The thread-tools endpoint verified a token and hands the call here. Nothing is written until
//! every check has passed, and the rules that decide whether the job may ask (who may be asked,
//! how deep, how many) are the core's ([`Input::Ask`]); what this adds is what the core cannot
//! know:
//!
//! * **the token against the thread's ledger**: the job it names is the thread's current one and
//!   its task has not ended, and an asked agent that calls is a running ask of this job that is the
//!   agent the token was minted for. The endpoint's guard checked that the caller is on the ledger;
//!   this checks it again at the moment of the ask, because a call can wait on the guard's answer
//!   while its ask ends;
//! * **the agent asked**: listed by the registry now (an agent the platform removed is "no longer
//!   listed", a registry that cannot answer is `RegistryUnavailable`, never a guess) and invocable
//!   under the deployment's roles. A role is a claim of a request's credential and is never stored,
//!   so what can be asked of a thread's owner who is not in the room is whether **some role of the
//!   policy** lets its holders invoke the agent ([`Policy::any_role_may_invoke`]); the agent was
//!   also checked against the person's own roles when the mention was written;
//! * **the call**: the call key `ask:<thread>:<caller>:<callId>` the core deduplicates by, the
//!   step the ask is shown under, and the deadline (the configured one, lowered by the call).
//!
//! Checks that need the registry or the roles are made only for an agent the person mentioned and
//! for a call that is not a repeat: an agent nobody mentioned is refused by the core with the
//! agents that may be asked, and a repeat of a call is answered from the ledger whatever has become
//! of the agent since.

use std::time::Duration;

use orch_core::{
    Actor, AgentId, AskRefusal, Caller, EventBody, Input, ThreadId, ThreadRecord, TransitionError,
    ask_step_id,
};
use orch_ports::{Ports, ThreadStore};

use crate::{App, AppError, ApplyOutcome, Permission};

/// A call of `ask_agent`, as the thread-tools endpoint received it after it verified the token.
#[derive(Debug, Clone)]
pub struct AskCall {
    /// The thread the token opens.
    pub thread: ThreadId,
    /// The token's `job`.
    pub job: u32,
    /// The token's `caller`: the addressed agent, or an ask of this job.
    pub caller: Caller,
    /// The token's `agt`: the agent that asks.
    pub asker: AgentId,
    /// The agent to ask.
    pub agent: AgentId,
    /// What to ask it.
    pub text: String,
    /// The request's `_meta["thread-tools/v1"].callId`: the agent's own id for this call, stable
    /// across a retry. Without one every call is a new ask.
    pub call_id: Option<String>,
    /// The request's `_meta["thread-tools/v1"].parentStepId`: the agent's own id of the step the
    /// call runs under.
    pub parent_step: Option<String>,
    /// What the call asked for as its timeout; it lowers the deployment's, never raises it.
    pub timeout: Option<Duration>,
}

/// An ask that is on the ledger, for the call to wait for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskHandle {
    /// The ask's number in the job.
    pub ask: u32,
    /// The agent asked.
    pub agent: AgentId,
    /// Whether this call found the ask already there (a repeat of a call key): it **re-attaches**
    /// and writes nothing.
    pub reattached: bool,
    /// Where to read the log from to see the ask end: just before the ask's own event for a new
    /// ask, the start of the log for one that was there (it may have ended long ago).
    pub after: i64,
    /// How long this call let the ask run: the deployment's limit, or less when the call asked
    /// for less. A repeat of a call is not told what the first one asked for, and says the
    /// deployment's.
    pub timeout: Duration,
}

/// The call key of an ask: `ask:<thread>:<caller>:<callId>`.
pub fn call_key(thread: ThreadId, caller: Caller, call_id: &str) -> String {
    format!("ask:{thread}:{caller}:{call_id}")
}

fn refused(why: AskRefusal) -> AppError {
    AppError::Transition(TransitionError::AskRefused(why))
}

impl<P: Ports> App<P> {
    /// Asks an agent the person mentioned on behalf of the agent the token was minted for, or finds
    /// the ask a repeat of the same call made.
    ///
    /// # Errors
    ///
    /// * [`AppError::NotFound`]: no such thread, or a caller that the ledger does not hold;
    /// * [`AppError::Transition`] with [`TransitionError::AskRefused`]: the task is over (the job
    ///   is not the token's, the thread is not running, the asking ask ended) or the core refused
    ///   (not mentioned, a cycle, a limit, an empty or long question, a call key used for another
    ///   ask);
    /// * [`AppError::Forbidden`]: no role of the deployment lets anyone invoke the agent (403);
    /// * [`AppError::Unprocessable`]: the registry no longer lists the agent (422);
    /// * [`AppError::RegistryUnavailable`]: the registry cannot say (retry);
    /// * the store's and the commit loop's errors.
    pub async fn ask(&self, mut call: AskCall) -> Result<AskHandle, AppError> {
        let store = self.ports().store();
        let thread = store
            .get_thread(None, call.thread)
            .await?
            .ok_or(AppError::NotFound)?;
        // The asked agent may be named by an alias of the one the thread's log mentions, or the
        // other way round (ADR 0049): it is asked under the name the log holds.
        let wanted = self.canonical_agent(&call.agent);
        if let Some(mentioned) = thread
            .job
            .mentioned
            .iter()
            .find(|m| self.canonical_agent(m) == wanted)
        {
            call.agent = mentioned.clone();
        }
        if thread.job.number != call.job || thread.state.is_terminal() {
            return Err(refused(AskRefusal::TaskOver));
        }
        self.check_caller(&thread, &call)?;

        let key = call
            .call_id
            .as_deref()
            .map(|id| call_key(call.thread, call.caller, id));
        let repeat = key.as_deref().is_some_and(|key| {
            thread
                .job
                .asks
                .iter()
                .any(|a| a.by == call.caller && a.call_key.as_deref() == Some(key))
        });
        if !repeat && thread.job.mentioned.contains(&call.agent) {
            self.check_askable(&call.agent).await?;
        }

        let binding = store.get_binding(call.thread).await?;
        // The step the ask is shown under: the asker's own step when it named one (its ids are the
        // task's, prefixed by the adapter, as for a relayed call), else the ask above it.
        let parent_step = match (call.caller, &call.parent_step) {
            (Caller::Ask(m), _) => Some(ask_step_id(m)),
            (Caller::Main, Some(parent)) => binding
                .as_ref()
                .and_then(|b| b.task_id.as_ref())
                .map(|task| format!("{task}/{parent}")),
            (Caller::Main, None) => None,
        };
        let limits = self.ask_limits();
        // a call may lower the ask's deadline; a repeat re-attaches to the ask the first call made
        // and is told the deployment's (it does not know what the first one asked for)
        let limits = match call.timeout {
            Some(asked)
                if !repeat
                    && asked < Duration::from_secs(limits.timeout.as_secs().unsigned_abs()) =>
            {
                limits.with_timeout(asked)
            }
            _ => limits,
        };
        let revision = match call.caller {
            Caller::Main => binding.and_then(|b| b.revision),
            Caller::Ask(_) => None,
        };
        let input = Input::Ask {
            actor: Actor::agent(&call.asker, revision),
            caller: call.caller,
            agent: call.agent.clone(),
            text: call.text,
            call_key: key.clone(),
            parent_step,
            limits,
        };
        let timeout = Duration::from_secs(limits.timeout.as_secs().unsigned_abs());
        match self.apply(call.thread, input, None, None, None).await? {
            ApplyOutcome::Applied { thread, events } => {
                let started = events.iter().find_map(|e| match &e.body {
                    EventBody::AskStarted(d) => Some((d.ask, e.seq)),
                    _ => None,
                });
                match started {
                    Some((ask, seq)) => Ok(AskHandle {
                        ask,
                        agent: call.agent,
                        reattached: false,
                        after: seq - 1,
                        timeout,
                    }),
                    // nothing was written: the same call again, which the ledger holds
                    None => {
                        let held = key
                            .as_deref()
                            .and_then(|key| held_by(&thread, call.caller, key));
                        held.map(|ask| AskHandle {
                            ask,
                            agent: call.agent,
                            reattached: true,
                            after: 0,
                            timeout,
                        })
                        .ok_or_else(|| {
                            AppError::internal("an ask that wrote nothing is not on the ledger")
                        })
                    }
                }
            }
            ApplyOutcome::Duplicate | ApplyOutcome::Fenced => Err(AppError::internal(
                "an ask is applied with no key and no claim",
            )),
        }
    }

    /// The caller is on the ledger as the token says: the thread's addressed agent, or a running
    /// ask of this job that is the agent the token was minted for.
    fn check_caller(&self, thread: &ThreadRecord, call: &AskCall) -> Result<(), AppError> {
        match call.caller {
            Caller::Main if thread.target.agent_id == call.asker => Ok(()),
            Caller::Main => Err(AppError::NotFound),
            Caller::Ask(n) => match thread.job.asks.iter().find(|a| a.n == n) {
                Some(ask) if ask.agent != call.asker => Err(AppError::NotFound),
                Some(ask) if ask.is_running() => Ok(()),
                Some(_) => Err(refused(AskRefusal::TaskOver)),
                None => Err(refused(AskRefusal::UnknownCaller { n })),
            },
        }
    }

    /// The agent can be asked now: some role of the deployment lets its holders invoke it, and the
    /// registry lists it.
    async fn check_askable(&self, agent: &AgentId) -> Result<(), AppError> {
        if !self
            .policy()
            .any_role_may_invoke(&self.canonical_agent(agent))
        {
            return Err(AppError::agent_not_allowed(Permission::AgentInvoke, agent));
        }
        match self.resolve_agent(agent).await? {
            Some(_) => Ok(()),
            None => Err(AppError::Unprocessable(format!(
                "agent '{agent}' is no longer listed"
            ))),
        }
    }
}

/// The number of the ask the caller's call key names on the ledger.
fn held_by(thread: &ThreadRecord, caller: Caller, key: &str) -> Option<u32> {
    thread
        .job
        .asks
        .iter()
        .find(|a| a.by == caller && a.call_key.as_deref() == Some(key))
        .map(|a| a.n)
}
