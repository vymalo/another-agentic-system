//! Pure mapping from A2A 1.0 values to protocol-neutral [`AgentEnvelope`]s.
//!
//! Idempotency keys (the dispatcher stores them so replays never duplicate chat events):
//!
//! | A2A value | key |
//! |---|---|
//! | artifact `X` of task `T` | `Task("a2a:T:artifact:X")` |
//! | agent message `M` | `Task("a2a:msg:M")` |
//! | status carrying message `M` | `Task("a2a:T:status-msg:M")` |
//! | status without a message | `Turn("T:status:<state>")` (the dispatcher prefixes the outbox row) |
//!
//! A snapshot (`Task`, from `GetTask`, `CancelTask` or the first frame of `SubscribeToTask`)
//! maps to the same keys as the live stream, so a poll after a crash and the live events it
//! replaces collapse into one.
//!
//! Artifacts and chunking. On the wire (ProtoJSON) `append: false` and `lastChunk: false` are
//! indistinguishable from "unset", so the first chunk of a chunked artifact looks exactly like a
//! whole artifact. An artifact that is not marked `lastChunk: true` is therefore held back until
//! the next event proves no continuation follows (any other event, or `lastChunk: true`), and
//! is then emitted once, assembled, under its single key. A stream that ends with one still held
//! back drops it: the dispatcher then polls `GetTask`, whose snapshot has it whole, under the
//! same key. The cost is that such an artifact appears when the agent's next event does (a
//! status update normally follows immediately).
//!
//! Known limits, by design: `Task.history` is not replayed (a live stream reports agent
//! messages as `Message` events and status messages as status detail; history mixes both and
//! cannot be mapped back to the same keys), and a snapshot taken while an artifact is still
//! being appended chunk by chunk contains only the chunks so far.

use std::collections::HashMap;

use a2a::{
    Message, Part, PartContent, Role, StreamResponse, Task, TaskArtifactUpdateEvent, TaskState,
    TaskStatus,
};
use orch_core::{AgentTaskState, AgentUpdate};
use orch_ports::{AgentEnvelope, AgentError, IdemKey, TaskSnapshot};
use serde_json::Value;

use crate::releases::RELEASE_CHANNELS_URI;

type Metadata = Option<HashMap<String, Value>>;

/// The neutral state of an A2A state; `None` for `Unspecified`.
pub(crate) fn state_of(state: &TaskState) -> Option<AgentTaskState> {
    match state {
        TaskState::Unspecified => None,
        TaskState::Submitted => Some(AgentTaskState::Submitted),
        TaskState::Working => Some(AgentTaskState::Working),
        TaskState::Completed => Some(AgentTaskState::Completed),
        TaskState::Failed => Some(AgentTaskState::Failed),
        TaskState::Canceled => Some(AgentTaskState::Canceled),
        TaskState::InputRequired => Some(AgentTaskState::InputRequired),
        TaskState::Rejected => Some(AgentTaskState::Rejected),
        TaskState::AuthRequired => Some(AgentTaskState::AuthRequired),
    }
}

fn slug(state: AgentTaskState) -> &'static str {
    match state {
        AgentTaskState::Submitted => "submitted",
        AgentTaskState::Working => "working",
        AgentTaskState::InputRequired => "input_required",
        AgentTaskState::AuthRequired => "auth_required",
        AgentTaskState::Completed => "completed",
        AgentTaskState::Failed => "failed",
        AgentTaskState::Canceled => "canceled",
        AgentTaskState::Rejected => "rejected",
    }
}

/// Text parts joined by newlines; `None` when there is no text.
fn text_of(parts: &[Part]) -> Option<String> {
    let texts: Vec<&str> = parts
        .iter()
        .filter_map(|p| match &p.content {
            PartContent::Text(t) => Some(t.as_str()),
            PartContent::Raw(_) | PartContent::Url(_) | PartContent::Data(_) => None,
        })
        .collect();
    let joined = texts.join("\n");
    (!joined.trim().is_empty()).then_some(joined)
}

/// The revision the agent echoes under the extension's key (`{requested, revision}`).
pub(crate) fn revision_of(metadata: &Metadata) -> Option<String> {
    metadata
        .as_ref()?
        .get(RELEASE_CHANNELS_URI)?
        .get("revision")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn status_envelope(
    task_id: &str,
    context_id: &str,
    status: &TaskStatus,
    revision: Option<String>,
) -> AgentEnvelope {
    let state = state_of(&status.state);
    let (key, update) = match state {
        None | Some(AgentTaskState::Submitted) => {
            (IdemKey::Turn(format!("{task_id}:status:submitted")), None)
        }
        Some(state) => {
            let detail = status.message.as_ref().and_then(|m| text_of(&m.parts));
            let key = match status.message.as_ref().filter(|m| !m.message_id.is_empty()) {
                Some(m) => IdemKey::Task(format!("a2a:{task_id}:status-msg:{}", m.message_id)),
                None => IdemKey::Turn(format!("{task_id}:status:{}", slug(state))),
            };
            (key, Some(AgentUpdate::Status { state, detail }))
        }
    };
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: state,
        revision,
        key,
        update,
    }
}

fn artifact_update(artifact_id: &str, name: Option<&str>, parts: &[Part]) -> AgentUpdate {
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or(artifact_id)
        .to_owned();
    let uri = parts.iter().find_map(|p| match &p.content {
        PartContent::Url(u) => Some(u.clone()),
        PartContent::Text(_) | PartContent::Raw(_) | PartContent::Data(_) => None,
    });
    let mut pieces: Vec<String> = Vec::new();
    let mut only_data = true;
    for p in parts {
        match &p.content {
            PartContent::Text(t) => {
                only_data = false;
                pieces.push(t.clone());
            }
            PartContent::Data(v) => pieces.push(v.to_string()),
            PartContent::Url(_) | PartContent::Raw(_) => only_data = false,
        }
    }
    let mime_type = parts
        .iter()
        .find_map(|p| p.media_type.clone())
        .or_else(|| (only_data && !pieces.is_empty()).then(|| "application/json".to_owned()));
    let joined = pieces.join("\n");
    AgentUpdate::Artifact {
        name,
        mime_type,
        uri,
        text: (!joined.is_empty()).then_some(joined),
    }
}

fn artifact_envelope(
    task_id: &str,
    context_id: &str,
    artifact_id: &str,
    update: AgentUpdate,
    revision: Option<String>,
) -> AgentEnvelope {
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: None,
        revision,
        key: IdemKey::Task(format!("a2a:{task_id}:artifact:{artifact_id}")),
        update: Some(update),
    }
}

/// Every artifact of a task, merging entries that share an id (the server appends chunk by
/// chunk without merging), then the status.
pub(crate) fn task_envelopes(task: &Task) -> Vec<AgentEnvelope> {
    let revision = revision_of(&task.metadata);
    let mut order: Vec<&str> = Vec::new();
    let mut merged: HashMap<&str, (Option<&str>, Vec<Part>)> = HashMap::new();
    for a in task.artifacts.iter().flatten() {
        let entry = merged.entry(a.artifact_id.as_str()).or_insert_with(|| {
            order.push(a.artifact_id.as_str());
            (a.name.as_deref(), Vec::new())
        });
        entry.1.extend(a.parts.iter().cloned());
    }
    let mut out: Vec<AgentEnvelope> = order
        .into_iter()
        .filter_map(|id| {
            let (name, parts) = merged.get(id)?;
            Some(artifact_envelope(
                &task.id,
                &task.context_id,
                id,
                artifact_update(id, *name, parts),
                revision.clone(),
            ))
        })
        .collect();
    out.push(status_envelope(
        &task.id,
        &task.context_id,
        &task.status,
        revision,
    ));
    out
}

/// A polled view of a task. An unspecified state is a protocol violation.
pub(crate) fn snapshot(task: &Task) -> Result<TaskSnapshot, AgentError> {
    let state = state_of(&task.status.state).ok_or_else(|| {
        AgentError::Protocol(format!("task {} reports an unspecified state", task.id))
    })?;
    Ok(TaskSnapshot {
        task_id: task.id.clone(),
        context_id: task.context_id.clone(),
        state,
        revision: revision_of(&task.metadata),
        envelopes: task_envelopes(task),
    })
}

/// An artifact held back because more chunks may follow.
struct Pending {
    task_id: String,
    context_id: String,
    artifact_id: String,
    name: Option<String>,
    parts: Vec<Part>,
    revision: Option<String>,
}

impl Pending {
    fn into_envelope(self) -> AgentEnvelope {
        artifact_envelope(
            &self.task_id,
            &self.context_id,
            &self.artifact_id,
            artifact_update(&self.artifact_id, self.name.as_deref(), &self.parts),
            self.revision,
        )
    }
}

/// Stream-local state: the task the stream belongs to, and the artifact held back (if any).
#[derive(Default)]
pub(crate) struct StreamMapper {
    task_id: Option<String>,
    context_id: Option<String>,
    pending: Option<Pending>,
}

impl StreamMapper {
    fn learn(&mut self, task_id: &str, context_id: &str) {
        if !task_id.is_empty() {
            self.task_id = Some(task_id.to_owned());
        }
        if !context_id.is_empty() {
            self.context_id = Some(context_id.to_owned());
        }
    }

    /// Maps one stream item to zero or more envelopes (or an error).
    pub(crate) fn map(&mut self, item: StreamResponse) -> Vec<Result<AgentEnvelope, AgentError>> {
        if let StreamResponse::ArtifactUpdate(u) = item {
            self.learn(&u.task_id, &u.context_id);
            return self.artifact_update(u).into_iter().map(Ok).collect();
        }
        // Any other event ends a held-back artifact: nothing more is appended to it.
        let mut out: Vec<Result<AgentEnvelope, AgentError>> = self
            .pending
            .take()
            .map(Pending::into_envelope)
            .map(Ok)
            .into_iter()
            .collect();
        match item {
            StreamResponse::Task(task) => {
                self.learn(&task.id, &task.context_id);
                out.extend(task_envelopes(&task).into_iter().map(Ok));
            }
            StreamResponse::StatusUpdate(u) => {
                self.learn(&u.task_id, &u.context_id);
                out.push(Ok(status_envelope(
                    &u.task_id,
                    &u.context_id,
                    &u.status,
                    revision_of(&u.metadata),
                )));
            }
            StreamResponse::Message(m) => out.extend(self.message(&m)),
            StreamResponse::ArtifactUpdate(_) => {}
        }
        out
    }

    fn artifact_update(&mut self, u: TaskArtifactUpdateEvent) -> Vec<AgentEnvelope> {
        let revision = revision_of(&u.metadata);
        let append = u.append.unwrap_or(false);
        let last = u.last_chunk == Some(true);
        let same_artifact = self
            .pending
            .as_ref()
            .is_some_and(|p| p.artifact_id == u.artifact.artifact_id);

        let mut out = Vec::new();
        let held = if same_artifact && append {
            let mut p = self.pending.take();
            if let Some(p) = p.as_mut() {
                p.parts.extend(u.artifact.parts);
                p.revision = revision.or(p.revision.take());
            }
            p
        } else {
            // A different artifact, or the same id sent whole again (which replaces it).
            if !same_artifact {
                out.extend(self.pending.take().map(Pending::into_envelope));
            } else {
                self.pending = None;
            }
            Some(Pending {
                task_id: u.task_id,
                context_id: u.context_id,
                artifact_id: u.artifact.artifact_id,
                name: u.artifact.name,
                parts: u.artifact.parts,
                revision,
            })
        };
        match held {
            Some(p) if last => out.push(p.into_envelope()),
            other => self.pending = other,
        }
        out
    }

    fn message(&mut self, m: &Message) -> Vec<Result<AgentEnvelope, AgentError>> {
        match m.role {
            Role::Agent => {}
            Role::User | Role::Unspecified => return Vec::new(),
        }
        let Some(text) = text_of(&m.parts) else {
            return Vec::new();
        };
        let task_id = m.task_id.clone().filter(|t| !t.is_empty());
        let Some(task_id) = task_id.or_else(|| self.task_id.clone()) else {
            return vec![Err(AgentError::Unsupported(
                "the agent answered with a message but no task; only task-based agents are supported"
                    .to_owned(),
            ))];
        };
        let context_id = m
            .context_id
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| self.context_id.clone())
            .unwrap_or_default();
        self.learn(&task_id, &context_id);
        vec![Ok(AgentEnvelope {
            task_id,
            context_id,
            task_state: None,
            revision: revision_of(&m.metadata),
            key: IdemKey::Task(format!("a2a:msg:{}", m.message_id)),
            update: Some(AgentUpdate::Message {
                message_id: m.message_id.clone(),
                text,
                is_final: true,
            }),
        })]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use a2a::{Artifact, TaskStatusUpdateEvent};
    use serde_json::json;

    use super::*;

    const T: &str = "task-1";
    const C: &str = "ctx-1";

    fn msg(id: &str, role: Role, text: &str) -> Message {
        let mut m = Message::new(role, vec![Part::text(text)]);
        m.message_id = id.to_owned();
        m
    }

    fn status(state: TaskState, message: Option<Message>) -> TaskStatus {
        TaskStatus {
            state,
            message,
            timestamp: None,
        }
    }

    fn status_update(state: TaskState, message: Option<Message>) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            status: status(state, message),
            metadata: None,
        })
    }

    fn art(id: &str, name: Option<&str>, parts: Vec<Part>) -> Artifact {
        Artifact {
            artifact_id: id.into(),
            name: name.map(str::to_owned),
            description: None,
            parts,
            metadata: None,
            extensions: None,
        }
    }

    fn artifact_update(a: Artifact, append: Option<bool>, last: Option<bool>) -> StreamResponse {
        StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            artifact: a,
            append,
            last_chunk: last,
            metadata: None,
        })
    }

    fn task(state: TaskState, artifacts: Vec<Artifact>, message: Option<Message>) -> Task {
        Task {
            id: T.into(),
            context_id: C.into(),
            status: status(state, message),
            artifacts: Some(artifacts),
            history: Some(vec![msg("h1", Role::Agent, "history is not replayed")]),
            metadata: None,
        }
    }

    fn only(mut v: Vec<Result<AgentEnvelope, AgentError>>) -> AgentEnvelope {
        assert_eq!(v.len(), 1, "{v:?}");
        v.remove(0).unwrap()
    }

    #[test]
    fn status_without_message_uses_a_turn_scoped_key() {
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, None)));
        assert_eq!(env.task_id, T);
        assert_eq!(env.context_id, C);
        assert_eq!(env.task_state, Some(AgentTaskState::Working));
        assert_eq!(env.key, IdemKey::Turn("task-1:status:working".into()));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Status {
                state: AgentTaskState::Working,
                detail: None
            })
        );
    }

    #[test]
    fn status_with_message_uses_a_task_scoped_key_and_carries_the_text() {
        let m = msg("m-7", Role::Agent, "Which branch?");
        let env =
            only(StreamMapper::default().map(status_update(TaskState::InputRequired, Some(m))));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:status-msg:m-7".into()));
        assert_eq!(env.task_state, Some(AgentTaskState::InputRequired));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Status {
                state: AgentTaskState::InputRequired,
                detail: Some("Which branch?".into())
            })
        );
    }

    #[test]
    fn every_state_maps() {
        let expect = [
            (TaskState::Submitted, AgentTaskState::Submitted),
            (TaskState::Working, AgentTaskState::Working),
            (TaskState::Completed, AgentTaskState::Completed),
            (TaskState::Failed, AgentTaskState::Failed),
            (TaskState::Canceled, AgentTaskState::Canceled),
            (TaskState::InputRequired, AgentTaskState::InputRequired),
            (TaskState::Rejected, AgentTaskState::Rejected),
            (TaskState::AuthRequired, AgentTaskState::AuthRequired),
        ];
        for (a2a_state, want) in expect {
            let env = only(StreamMapper::default().map(status_update(a2a_state, None)));
            assert_eq!(env.task_state, Some(want));
        }
        assert_eq!(state_of(&TaskState::Unspecified), None);
    }

    #[test]
    fn submitted_and_unspecified_only_carry_the_task_id() {
        let env = only(StreamMapper::default().map(status_update(TaskState::Submitted, None)));
        assert_eq!(env.update, None);
        assert_eq!(env.task_id, T);
        assert_eq!(env.task_state, Some(AgentTaskState::Submitted));
        let env = only(StreamMapper::default().map(status_update(TaskState::Unspecified, None)));
        assert_eq!(env.update, None);
        assert_eq!(env.task_state, None);
    }

    #[test]
    fn whole_artifact_maps_name_uri_text_and_key() {
        let a = art(
            "a-1",
            Some("result"),
            vec![
                Part::text("done"),
                Part::url("https://github.com/acme/demo/pull/1"),
            ],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:artifact:a-1".into()));
        assert_eq!(env.task_state, None);
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "result".into(),
                mime_type: None,
                uri: Some("https://github.com/acme/demo/pull/1".into()),
                text: Some("done".into()),
            })
        );
    }

    #[test]
    fn an_artifact_not_marked_last_is_held_until_the_next_event() {
        let mut m = StreamMapper::default();
        let a = art("a-1", Some("result"), vec![Part::text("x")]);
        assert!(m.map(artifact_update(a, None, None)).is_empty());
        let out = m.map(status_update(TaskState::Completed, None));
        assert_eq!(out.len(), 2, "the artifact comes out before the status");
        let envs: Vec<_> = out.into_iter().map(Result::unwrap).collect();
        assert!(matches!(envs[0].update, Some(AgentUpdate::Artifact { .. })));
        assert_eq!(envs[1].task_state, Some(AgentTaskState::Completed));
    }

    #[test]
    fn a_stream_that_ends_drops_a_held_back_artifact() {
        // The dispatcher then polls GetTask, whose snapshot has the artifact whole, same key.
        let mut m = StreamMapper::default();
        let a = art("a-1", None, vec![Part::text("partial")]);
        assert!(m.map(artifact_update(a, None, None)).is_empty());
        assert!(m.pending.is_some());
    }

    #[test]
    fn artifact_without_name_falls_back_to_its_id_and_data_becomes_json_text() {
        let a = art("a-2", None, vec![Part::data(json!({"ok": true}))]);
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "a-2".into(),
                mime_type: Some("application/json".into()),
                uri: None,
                text: Some(r#"{"ok":true}"#.into()),
            })
        );
    }

    #[test]
    fn media_type_of_the_first_part_wins() {
        let a = art(
            "a-3",
            Some("patch"),
            vec![Part::text("diff").with_media_type("text/x-diff")],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        let Some(AgentUpdate::Artifact { mime_type, .. }) = env.update else {
            panic!("not an artifact");
        };
        assert_eq!(mime_type.as_deref(), Some("text/x-diff"));
    }

    #[test]
    fn appended_chunks_are_emitted_once_on_the_last_chunk() {
        let mut m = StreamMapper::default();
        // On the wire the first chunk's `append: false, lastChunk: false` arrive as unset.
        let first = art("a-4", Some("log"), vec![Part::text("one ")]);
        assert!(m.map(artifact_update(first, None, None)).is_empty());
        let mid = art("a-4", None, vec![Part::text("two ")]);
        assert!(m.map(artifact_update(mid, Some(true), None)).is_empty());
        let last = art("a-4", None, vec![Part::text("three")]);
        let env = only(m.map(artifact_update(last, Some(true), Some(true))));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:artifact:a-4".into()));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "log".into(),
                mime_type: None,
                uri: None,
                text: Some("one \ntwo \nthree".into()),
            })
        );
        assert!(m.pending.is_none(), "nothing is left held back");
    }

    #[test]
    fn another_artifact_ends_the_held_back_one() {
        let mut m = StreamMapper::default();
        let a = art("a-8", Some("first"), vec![Part::text("1")]);
        assert!(m.map(artifact_update(a, None, None)).is_empty());
        let b = art("a-9", Some("second"), vec![Part::text("2")]);
        let out = m.map(artifact_update(b, None, Some(true)));
        let names: Vec<_> = out
            .into_iter()
            .map(|e| match e.unwrap().update {
                Some(AgentUpdate::Artifact { name, .. }) => name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["first", "second"]);
    }

    #[test]
    fn the_same_artifact_sent_whole_again_replaces_the_held_back_one() {
        let mut m = StreamMapper::default();
        let v1 = art("a-8", Some("n"), vec![Part::text("old")]);
        assert!(m.map(artifact_update(v1, None, None)).is_empty());
        let v2 = art("a-8", Some("n"), vec![Part::text("new")]);
        let env = only(m.map(artifact_update(v2, None, Some(true))));
        let Some(AgentUpdate::Artifact { text, .. }) = env.update else {
            panic!("not an artifact");
        };
        assert_eq!(text.as_deref(), Some("new"));
    }

    #[test]
    fn agent_message_maps_with_a_task_scoped_key_and_learns_the_task() {
        let mut m = StreamMapper::default();
        let mut message = msg("m-9", Role::Agent, "hello");
        message.task_id = Some(T.into());
        message.context_id = Some(C.into());
        let env = only(m.map(StreamResponse::Message(message)));
        assert_eq!(env.task_id, T);
        assert_eq!(env.key, IdemKey::Task("a2a:msg:m-9".into()));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Message {
                message_id: "m-9".into(),
                text: "hello".into(),
                is_final: true
            })
        );
    }

    #[test]
    fn message_without_task_uses_the_streams_task_or_is_unsupported() {
        let mut m = StreamMapper::default();
        let bare = msg("m-1", Role::Agent, "hi");
        let out = m.map(StreamResponse::Message(bare.clone()));
        assert!(matches!(out.as_slice(), [Err(AgentError::Unsupported(_))]));

        let _ = m.map(status_update(TaskState::Working, None));
        let env = only(m.map(StreamResponse::Message(bare)));
        assert_eq!(env.task_id, T);
        assert_eq!(env.context_id, C);
    }

    #[test]
    fn user_messages_and_empty_agent_messages_are_ignored() {
        let mut m = StreamMapper::default();
        assert!(
            m.map(StreamResponse::Message(msg("u", Role::User, "x")))
                .is_empty()
        );
        let empty = Message::new(Role::Agent, vec![Part::url("https://x")]);
        assert!(m.map(StreamResponse::Message(empty)).is_empty());
    }

    #[test]
    fn task_snapshot_lists_artifacts_then_the_status_with_stream_keys() {
        let t = task(
            TaskState::Completed,
            vec![art("a-1", Some("r"), vec![Part::text("x")])],
            None,
        );
        let envs = task_envelopes(&t);
        assert_eq!(envs.len(), 2, "history is not replayed");
        assert_eq!(envs[0].key, IdemKey::Task("a2a:task-1:artifact:a-1".into()));
        assert_eq!(envs[1].key, IdemKey::Turn("task-1:status:completed".into()));
        // The same status as a live update has the same key.
        let live = only(StreamMapper::default().map(status_update(TaskState::Completed, None)));
        assert_eq!(live.key, envs[1].key);
    }

    #[test]
    fn snapshot_merges_chunked_artifacts_that_share_an_id() {
        let t = task(
            TaskState::Working,
            vec![
                art("a-6", Some("log"), vec![Part::text("one")]),
                art("a-7", Some("other"), vec![Part::text("z")]),
                art("a-6", None, vec![Part::text("two")]),
            ],
            None,
        );
        let envs = task_envelopes(&t);
        assert_eq!(envs.len(), 3);
        let Some(AgentUpdate::Artifact { text, name, .. }) = &envs[0].update else {
            panic!("not an artifact");
        };
        assert_eq!(name, "log");
        assert_eq!(text.as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn a_task_frame_teaches_the_stream_its_task() {
        let mut m = StreamMapper::default();
        let envs = m.map(StreamResponse::Task(task(
            TaskState::Submitted,
            vec![],
            None,
        )));
        assert_eq!(envs.len(), 1);
        let env = only(m.map(StreamResponse::Message(msg("m", Role::Agent, "yo"))));
        assert_eq!(env.task_id, T);
    }

    #[test]
    fn snapshot_reports_state_revision_and_rejects_unspecified() {
        let mut t = task(
            TaskState::InputRequired,
            vec![],
            Some(msg("q", Role::Agent, "why?")),
        );
        t.metadata = Some(HashMap::from([(
            RELEASE_CHANNELS_URI.to_owned(),
            json!({"requested": "staging", "revision": "coder-r51"}),
        )]));
        let s = snapshot(&t).unwrap();
        assert_eq!(s.state, AgentTaskState::InputRequired);
        assert_eq!(s.revision.as_deref(), Some("coder-r51"));
        assert_eq!(s.task_id, T);
        assert_eq!(
            s.envelopes.last().unwrap().revision.as_deref(),
            Some("coder-r51")
        );

        let bad = task(TaskState::Unspecified, vec![], None);
        assert!(matches!(snapshot(&bad), Err(AgentError::Protocol(_))));
    }

    #[test]
    fn revision_is_read_from_event_metadata() {
        let mut u = TaskStatusUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            status: status(TaskState::Working, None),
            metadata: Some(HashMap::from([(
                RELEASE_CHANNELS_URI.to_owned(),
                json!({"revision": "coder-r47"}),
            )])),
        };
        let env = only(StreamMapper::default().map(StreamResponse::StatusUpdate(u.clone())));
        assert_eq!(env.revision.as_deref(), Some("coder-r47"));
        u.metadata = None;
        let env = only(StreamMapper::default().map(StreamResponse::StatusUpdate(u)));
        assert_eq!(env.revision, None);
    }
}
