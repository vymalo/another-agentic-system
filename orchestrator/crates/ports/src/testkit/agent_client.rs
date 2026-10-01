//! `AgentClient` conformance cases: what the dispatcher relies on from any agent adapter,
//! stated once and run against the A2A adapter (over real HTTP, against an in-process agent)
//! and against the scripted in-memory agent.
//!
//! An implementation supplies an [`AgentFixture`]: a client, an endpoint of a healthy agent
//! that runs the [`Script`]s, an endpoint nobody listens on, and the gate the `Gate` script
//! waits for. Each case is `async fn(fixture)` and gives up after 10 seconds.
//!
//! What is deliberately not asserted: the wording of any text, the number of envelopes a turn
//! takes, and whether the agent supports `resubscribe` or `find_task_by_message` at all (an
//! adapter may answer `Unsupported` or `Ok(None)`; it may never name a wrong task).

use std::future::Future;
use std::time::Duration;

use futures::StreamExt;
use orch_core::{AgentTaskState, AgentUpdate, Classify, ErrorClass};

use crate::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, IdemKey, SendRequest,
    TaskHandle,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a message asks the fixture's agent to do (chosen by the first word of its text, see
/// [`AgentFixture::text`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// `working`, an artifact, `completed`.
    Echo,
    /// `working`, `input-required`; a follow-up on the same task then completes it with an
    /// artifact.
    Ask,
    /// `working`, then waits for [`AgentFixture::release_gate`], then an artifact and
    /// `completed`.
    Gate,
    /// `working`, then runs until cancelled.
    Slow,
    /// `working`, then `failed` with a message.
    Fail,
}

/// The message [`AgentFixture::text`] returns for a failing task carries this.
pub const FAILURE_MESSAGE: &str = "scripted failure";

/// An agent adapter under test, with an agent behind it that speaks the [`Script`]s.
pub trait AgentFixture: Send + Sync + 'static {
    /// The adapter under test.
    type Client: AgentClient;

    /// The adapter.
    fn client(&self) -> &Self::Client;

    /// An endpoint of a healthy agent that runs the scripts.
    fn endpoint(&self) -> AgentEndpoint;

    /// An endpoint that cannot be reached (nothing listens there).
    fn unreachable(&self) -> AgentEndpoint;

    /// Lets one waiting [`Script::Gate`] task continue (a permit is kept if none waits yet).
    fn release_gate(&self);

    /// The message text that makes the agent run `script`: its first word, then something to
    /// echo. The default fits an agent that names its scripts `echo`, `ask`, `gate`, `slow`
    /// and `fail`; override it for one that does not.
    fn text(&self, script: Script) -> String {
        let word = match script {
            Script::Echo => "echo",
            Script::Ask => "ask",
            Script::Gate => "gate",
            Script::Slow => "slow",
            Script::Fail => "fail",
        };
        format!("{word} conformance")
    }
}

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, case)
        .await
        .expect("the case did not finish within 10 s")
}

fn request(
    ep: &AgentEndpoint,
    text: String,
    message_id: &str,
    context_id: &str,
    task_id: Option<String>,
) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: message_id.to_owned(),
        context_id: context_id.to_owned(),
        task_id,
        reference_task_ids: Vec::new(),
        content: crate::SendContent::Text(text),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
    }
}

fn handle(ep: &AgentEndpoint, task_id: &str) -> TaskHandle {
    TaskHandle {
        endpoint: ep.clone(),
        task_id: task_id.to_owned(),
    }
}

/// The task state an envelope implies.
fn state_of(env: &AgentEnvelope) -> Option<AgentTaskState> {
    env.task_state.or(match &env.update {
        Some(AgentUpdate::Status { state, .. }) => Some(*state),
        Some(
            AgentUpdate::Artifact { .. }
            | AgentUpdate::Message { .. }
            | AgentUpdate::Ui { .. }
            | AgentUpdate::UiRejected { .. }
            | AgentUpdate::Step(_),
        )
        | None => None,
    })
}

async fn next(stream: &mut AgentStream) -> Option<AgentEnvelope> {
    stream
        .next()
        .await
        .map(|item| item.expect("the stream failed"))
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut all = Vec::new();
    while let Some(env) = next(&mut stream).await {
        all.push(env);
    }
    all
}

/// Reads the stream up to and including the first envelope that puts the task in `working`;
/// returns the task id and what was read.
async fn until_working(stream: &mut AgentStream) -> (String, Vec<AgentEnvelope>) {
    let mut seen = Vec::new();
    while let Some(env) = next(stream).await {
        let working = state_of(&env) == Some(AgentTaskState::Working);
        let task = env.task_id.clone();
        seen.push(env);
        if working {
            return (task, seen);
        }
    }
    panic!("the stream ended before the task was working: {seen:?}");
}

/// Sends `text` in `context` as `message_id` and returns the stream.
async fn send<F: AgentFixture>(
    fx: &F,
    script: Script,
    message_id: &str,
    context: &str,
) -> AgentStream {
    let req = request(&fx.endpoint(), fx.text(script), message_id, context, None);
    fx.client()
        .send_stream(req)
        .await
        .unwrap_or_else(|e| panic!("send_stream failed: {e}"))
}

fn keys_of_updates(envs: &[AgentEnvelope]) -> Vec<IdemKey> {
    envs.iter()
        .filter(|e| e.update.is_some())
        .map(|e| e.key.clone())
        .collect()
}

fn artifact_keys(envs: &[AgentEnvelope]) -> Vec<IdemKey> {
    envs.iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .map(|e| e.key.clone())
        .collect()
}

fn last_state(envs: &[AgentEnvelope]) -> Option<AgentTaskState> {
    envs.iter().rev().find_map(state_of)
}

fn assert_class(err: &AgentError, want: &[ErrorClass], what: &str) {
    assert!(want.contains(&err.class()), "{what}: got {err:?}");
}

// ------------------------------------------------------------------ cases

/// The card is read from the agent on every call, and an agent nobody listens on is a
/// transient failure (the dispatcher retries it), not a rejection.
pub async fn read_card_is_live_and_unreachable_is_transient<F: AgentFixture>(fx: F) {
    within(async {
        let client = fx.client();
        client.read_card(&fx.endpoint()).await.unwrap();
        // Read again: a client that cached the card would not notice the next change (ADR 0008).
        client.read_card(&fx.endpoint()).await.unwrap();
        let err = client.read_card(&fx.unreachable()).await.unwrap_err();
        assert_class(&err, &[ErrorClass::Transient], "an unreachable card");
        assert!(err.is_retryable());
    })
    .await;
}

/// The first envelope names the task; every envelope of the turn belongs to that task and to
/// the context the message named; the turn ends `completed` after an artifact.
pub async fn first_envelope_names_the_task<F: AgentFixture>(fx: F) {
    within(async {
        let envs = drain(send(&fx, Script::Echo, "m-first", "ctx-first").await).await;
        let first = envs.first().expect("at least one envelope");
        assert!(!first.task_id.is_empty());
        for env in &envs {
            assert_eq!(env.task_id, first.task_id, "one task per turn");
            assert_eq!(env.context_id, "ctx-first", "the context is echoed");
        }
        assert_eq!(last_state(&envs), Some(AgentTaskState::Completed));
        assert!(
            !artifact_keys(&envs).is_empty(),
            "the turn produced an artifact: {envs:?}"
        );
    })
    .await;
}

/// No two updates of a turn share an idempotency key (a duplicate would be dropped as a
/// replay), and the keys are not empty.
pub async fn keys_are_unique_within_a_turn<F: AgentFixture>(fx: F) {
    within(async {
        let envs = drain(send(&fx, Script::Echo, "m-keys", "ctx-keys").await).await;
        let keys = keys_of_updates(&envs);
        assert!(!keys.is_empty());
        for (i, key) in keys.iter().enumerate() {
            let (IdemKey::Task(text) | IdemKey::Turn(text)) = key;
            assert!(!text.is_empty());
            assert!(!keys[..i].contains(key), "key {key:?} twice in {keys:?}");
        }
    })
    .await;
}

/// After the turn, `get_task` agrees with what the stream said: same task and context, the
/// same final state, and the same artifact keys (the dispatcher deduplicates on them when it
/// applies a snapshot after a broken stream).
pub async fn get_task_matches_the_live_stream<F: AgentFixture>(fx: F) {
    within(async {
        let envs = drain(send(&fx, Script::Echo, "m-get", "ctx-get").await).await;
        let task = envs[0].task_id.clone();
        let snap = fx
            .client()
            .get_task(&handle(&fx.endpoint(), &task))
            .await
            .unwrap();
        assert_eq!(snap.task_id, task);
        assert_eq!(snap.context_id, "ctx-get");
        assert_eq!(Some(snap.state), last_state(&envs));
        assert_eq!(snap.state, AgentTaskState::Completed);
        let from_stream = artifact_keys(&envs);
        let from_snapshot = artifact_keys(&snap.envelopes);
        assert!(!from_stream.is_empty());
        for key in &from_snapshot {
            assert!(from_stream.contains(key), "{key:?} is not in the stream");
        }
        for key in &from_stream {
            assert!(
                from_snapshot.contains(key),
                "{key:?} is not in the snapshot"
            );
        }
        assert!(
            snap.envelopes
                .iter()
                .any(|e| state_of(e) == Some(AgentTaskState::Completed)),
            "the snapshot carries the final status: {:?}",
            snap.envelopes
        );
    })
    .await;
}

/// A follow-up sent to a task that asked for input continues that very task, and finishes it.
pub async fn follow_up_after_input_required_continues_the_task<F: AgentFixture>(fx: F) {
    within(async {
        let asked = drain(send(&fx, Script::Ask, "m-ask", "ctx-ask").await).await;
        assert_eq!(last_state(&asked), Some(AgentTaskState::InputRequired));
        let task = asked[0].task_id.clone();
        let follow = request(
            &fx.endpoint(),
            fx.text(Script::Ask),
            "m-ask-2",
            "ctx-ask",
            Some(task.clone()),
        );
        let answered = drain(fx.client().send_stream(follow).await.unwrap()).await;
        assert!(!answered.is_empty());
        for env in &answered {
            assert_eq!(env.task_id, task, "the follow-up continues the task");
        }
        assert_eq!(last_state(&answered), Some(AgentTaskState::Completed));
        assert!(!artifact_keys(&answered).is_empty());
    })
    .await;
}

/// Re-attaching to a running task yields the rest of the turn under the same keys as the
/// original stream, so replaying both never duplicates an event. An adapter that cannot
/// re-attach answers `Unsupported` (the dispatcher polls instead) and passes.
pub async fn resubscribe_while_running_yields_the_rest_with_the_same_keys<F: AgentFixture>(fx: F) {
    within(async {
        let mut original = send(&fx, Script::Gate, "m-resub", "ctx-resub").await;
        let (task, mut seen) = until_working(&mut original).await;
        let resubscribed = match fx
            .client()
            .resubscribe(&handle(&fx.endpoint(), &task))
            .await
        {
            Ok(stream) => stream,
            Err(AgentError::Unsupported(_)) => {
                fx.release_gate();
                seen.extend(drain(original).await);
                assert_eq!(last_state(&seen), Some(AgentTaskState::Completed));
                return;
            }
            Err(e) => panic!("resubscribe failed: {e}"),
        };
        fx.release_gate();
        let rest = drain(resubscribed).await;
        seen.extend(drain(original).await);
        assert_eq!(last_state(&rest), Some(AgentTaskState::Completed));
        // The first frame of a re-attached stream is a snapshot of the current state; what
        // matters is that the artifacts and the ending carry the keys of the original.
        let original_keys = keys_of_updates(&seen);
        let artifacts = artifact_keys(&rest);
        assert!(!artifacts.is_empty(), "the artifact arrives: {rest:?}");
        for key in artifacts {
            assert!(
                original_keys.contains(&key),
                "{key:?} is new: {original_keys:?}"
            );
        }
        for env in rest
            .iter()
            .filter(|e| state_of(e).is_some_and(AgentTaskState::ends_turn) && e.update.is_some())
        {
            assert!(
                original_keys.contains(&env.key),
                "the ending has a new key {:?}: {original_keys:?}",
                env.key
            );
        }
    })
    .await;
}

/// A finished task cannot be re-attached to, and an unknown one is not found by any
/// operation. (`get_task` of a finished task still works: the dispatcher polls it.)
pub async fn finished_or_unknown_task_is_not_found<F: AgentFixture>(fx: F) {
    within(async {
        let ep = fx.endpoint();
        let client = fx.client();
        let done = drain(send(&fx, Script::Echo, "m-done", "ctx-done").await).await;
        let finished = handle(&ep, &done[0].task_id);
        assert_eq!(
            client.get_task(&finished).await.unwrap().state,
            AgentTaskState::Completed
        );
        // Re-attaching to a finished task fails; an adapter without re-attach says so.
        let err = match client.resubscribe(&finished).await {
            Ok(_) => panic!("re-attached to a finished task"),
            Err(e) => e,
        };
        assert_class(
            &err,
            &[ErrorClass::NotFound, ErrorClass::Unsupported],
            "resubscribe of a finished task",
        );

        let unknown = handle(&ep, "no-such-task");
        let err = client.get_task(&unknown).await.unwrap_err();
        assert_class(&err, &[ErrorClass::NotFound], "get_task of an unknown task");
        let err = client.cancel(&unknown).await.unwrap_err();
        assert_class(&err, &[ErrorClass::NotFound], "cancel of an unknown task");
        let err = match client.resubscribe(&unknown).await {
            Ok(_) => panic!("re-attached to an unknown task"),
            Err(e) => e,
        };
        assert_class(
            &err,
            &[ErrorClass::NotFound, ErrorClass::Unsupported],
            "resubscribe of an unknown task",
        );
    })
    .await;
}

/// The agent runs the turn to its end when the client goes away: a dropped stream does not
/// cancel the task, and `get_task` (the dispatcher's fallback) sees it finish.
pub async fn a_turn_outlives_its_stream<F: AgentFixture>(fx: F) {
    within(async {
        let mut stream = send(&fx, Script::Gate, "m-outlive", "ctx-outlive").await;
        let (task, _) = until_working(&mut stream).await;
        drop(stream);
        fx.release_gate();
        let ep = fx.endpoint();
        loop {
            let snap = fx.client().get_task(&handle(&ep, &task)).await.unwrap();
            match snap.state {
                AgentTaskState::Completed => break,
                AgentTaskState::Working | AgentTaskState::Submitted => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("the task ended {other:?} after its stream was dropped"),
            }
        }
    })
    .await;
}

/// A running task can be cancelled (the snapshot says `canceled` and the stream ends with it);
/// a completed task cannot: that is `NotCancelable`, which the dispatcher treats as final.
pub async fn cancel_running_then_cancel_finished_is_refused<F: AgentFixture>(fx: F) {
    within(async {
        let ep = fx.endpoint();
        let client = fx.client();
        let mut stream = send(&fx, Script::Slow, "m-cancel", "ctx-cancel").await;
        let (task, mut seen) = until_working(&mut stream).await;
        let snap = client.cancel(&handle(&ep, &task)).await.unwrap();
        assert_eq!(snap.task_id, task);
        assert_eq!(snap.state, AgentTaskState::Canceled);
        seen.extend(drain(stream).await);
        assert_eq!(last_state(&seen), Some(AgentTaskState::Canceled));

        // Cancelling again is either refused or idempotent (the A2A SDK answers with the
        // cancelled task); both are safe for the dispatcher, which applies the snapshot.
        match client.cancel(&handle(&ep, &task)).await {
            Ok(again) => assert_eq!(again.state, AgentTaskState::Canceled),
            Err(e) => assert_class(&e, &[ErrorClass::Rejected], "cancel of a cancelled task"),
        }

        let done = drain(send(&fx, Script::Echo, "m-cancel-done", "ctx-cancel").await).await;
        let err = client
            .cancel(&handle(&ep, &done[0].task_id))
            .await
            .unwrap_err();
        assert_class(&err, &[ErrorClass::Rejected], "cancel of a completed task");
    })
    .await;
}

/// A task the agent fails ends the stream in `failed` and carries the agent's message.
pub async fn failed_task_carries_its_message<F: AgentFixture>(fx: F) {
    within(async {
        let envs = drain(send(&fx, Script::Fail, "m-fail", "ctx-fail").await).await;
        assert_eq!(last_state(&envs), Some(AgentTaskState::Failed));
        let detail = envs
            .iter()
            .rev()
            .find_map(|e| match &e.update {
                Some(AgentUpdate::Status {
                    state: AgentTaskState::Failed,
                    detail,
                }) => detail.clone(),
                _ => None,
            })
            .expect("the failed status has a detail");
        assert!(detail.contains(FAILURE_MESSAGE), "{detail:?}");
    })
    .await;
}

/// Finding a task by message id may come back empty (the agent may not support listing
/// tasks) but never names the wrong task: not another message's, not another context's.
pub async fn find_task_by_message_never_names_a_wrong_task<F: AgentFixture>(fx: F) {
    within(async {
        let ep = fx.endpoint();
        let first = drain(send(&fx, Script::Echo, "m-find-1", "ctx-find").await).await;
        let second = drain(send(&fx, Script::Echo, "m-find-2", "ctx-find").await).await;
        let (t1, t2) = (first[0].task_id.clone(), second[0].task_id.clone());
        assert_ne!(t1, t2);
        let find = |context: &'static str, message: &'static str| {
            let (ep, fx) = (ep.clone(), &fx);
            async move {
                fx.client()
                    .find_task_by_message(&ep, context, message)
                    .await
                    .unwrap_or_else(|e| panic!("find_task_by_message failed: {e}"))
            }
        };
        for (message, right) in [("m-find-1", &t1), ("m-find-2", &t2)] {
            let found = find("ctx-find", message).await;
            assert!(
                found.is_none() || found.as_ref() == Some(right),
                "{message}: found {found:?}, expected {right} or nothing"
            );
        }
        assert_eq!(find("ctx-find", "m-never-sent").await, None);
        assert_eq!(find("ctx-elsewhere", "m-find-1").await, None);
    })
    .await;
}

/// Sending to an agent nobody listens on fails as a transient error whose public text (the
/// only text the chat shows) and message name neither the address nor the transport.
pub async fn unreachable_send_has_a_clean_public_detail<F: AgentFixture>(fx: F) {
    within(async {
        let gone = fx.unreachable();
        let req = request(&gone, fx.text(Script::Echo), "m-gone", "ctx-gone", None);
        let err = match fx.client().send_stream(req).await {
            Ok(_) => panic!("sent to an unreachable agent"),
            Err(e) => e,
        };
        assert_class(&err, &[ErrorClass::Transient], "a send to nobody");
        assert!(err.is_retryable());
        for text in [err.public_detail(), err.to_string()] {
            for leak in ["://", "127.0.0.1", "localhost", ".json"] {
                assert!(!text.contains(leak), "{text:?} leaks {leak:?}");
            }
        }
    })
    .await;
}
