//! The `vymalo.*` vocabulary of `docs/api/agui.md`: activity types, metadata keys, error codes.

use orch_agui_proto::Metadata;
use orch_core::{Actor, AgentStatus, AnswerVia, MessagePurpose, UserMessageData};
use serde_json::{Value, json};

/// The member of every `vymalo.*` activity's content that says when its event happened (the
/// event's `at`, RFC 3339).
pub const AT_KEY: &str = "at";

/// Activity type of a status line (`{status, detail?, at}`). The words of a `completed`,
/// `input_required` or `auth_required` status are an assistant message (`st-<seq>`) instead of
/// its `detail`.
pub const ACTIVITY_STATUS: &str = "vymalo.status";
/// Activity type of an artifact (`{kind, name, mimeType?, uri?, text?, at}` plus the fields of its
/// kind: `branch` `{repository, branch, sha, shortSha}`, `checks` `{passed, sha, shortSha}`,
/// `pull_request` `{url, number?, repository?, branch?}`, `file` nothing more).
pub const ACTIVITY_ARTIFACT: &str = "vymalo.artifact";
/// Activity type of an error (`{message, retryable, at}`).
pub const ACTIVITY_ERROR: &str = "vymalo.error";

/// Activity type of an A2UI surface: the ecosystem's, not ours (`@ag-ui/a2ui-middleware`,
/// *verified 2026-09-29*). Its content is `{a2ui_operations: [A2UI message]}`.
pub const ACTIVITY_A2UI_SURFACE: &str = "a2ui-surface";
/// The member of an `a2ui-surface` activity's content that holds the operations.
pub const A2UI_OPERATIONS_KEY: &str = "a2ui_operations";
/// Activity type of a verification source's answer (`vymalo.check`, ADR 0018): the content is the
/// `check_result` event's data, `{source, attempt, status, name?, commit?, summary?, stale?,
/// findings?}`, plus `at`.
pub const ACTIVITY_CHECK: &str = "vymalo.check";
/// Activity type of a CI report (`vymalo.ci`, ADR 0017): the content is
/// `{name, conclusion, passed, sha, shortSha, provider, repository, branch?, url?, summary?, at}`; `summary`
/// (and `name`, `branch`) come from outside and are untrusted text.
pub const ACTIVITY_CI: &str = "vymalo.ci";
/// Activity type of a rework (`vymalo.rework`, ADR 0018): the content is the `rework` event's
/// data, `{attempt, maxAttempts, findings: [{source, findings}]}`, plus `at`.
pub const ACTIVITY_REWORK: &str = "vymalo.rework";
/// Activity type of the start of a thread's next job (`vymalo.job`, ADR 0020): the content is
/// `{job, at}`, the number of the job that started (from 2), and its id is `job-<job>`.
pub const ACTIVITY_JOB: &str = "vymalo.job";
/// Activity type of the start of a fork (`vymalo.fork`, ADR 0029): the content is `{from: {threadId,
/// seq}, kind, title, target: {agentId, release?}, at}`, and its id is `fork-<seq>` of the
/// `thread_forked` event. A thread that began as a copy of another says so once, where the copy
/// ends. `title` is the parent's, as it was when the fork was made.
pub const ACTIVITY_FORK: &str = "vymalo.fork";
/// Activity type of a change of the MCP servers attached to the thread (`vymalo.tools`, ADR 0024):
/// the content is `{attached?: [id], detached?: [id], at}`, the server ids that came or went, and
/// its id is `evt-<seq>` of the `tools_attached` or `tools_detached` event. Ids only: no URL, no
/// credential.
pub const ACTIVITY_TOOLS: &str = "vymalo.tools";
/// Activity type of a user's action on a surface (`{surfaceId, name, sourceComponentId, context, at}`).
pub const ACTIVITY_ACTION: &str = "vymalo.action";

/// Activity type of a step of the agent's work (`vymalo.step`, ADR 0025): the content is `{id,
/// path, kind, label, state, icon?, detail?, startedAt, at}`; its id is `step-<seq of the step's
/// first event>`, and every event of the step says it again with `replace`. `label` and `detail`
/// come from an agent and are untrusted text.
pub const ACTIVITY_STEP: &str = "vymalo.step";

/// Activity type of an agent the thread's agent asked (`vymalo.ask`, ADR 0026): its id is
/// `ask-<n>`, the ask's step, and it says the ask again with `replace` when it ends. The content is
/// `{ask, agent, by, depth, text, stepId, parentStepId?, state, startedAt, at}` and, once the ask
/// ended, what it ended with: `answer?` (the asked agent's last words), `question?` (what it asks
/// back), `artifacts?` (`[{name, uri?, mimeType?}]`) and `error?`. `state` is `running` or the
/// outcome (`completed`, `input_required`, `auth_required`, `failed`, `rejected`, `canceled`,
/// `timed_out`). `text`, `answer`, `question` and `error` come from agents and are untrusted text.
pub const ACTIVITY_ASK: &str = "vymalo.ask";

/// Metadata key naming who produced an event (`{type, name, revision?}`).
pub const ACTOR_KEY: &str = "vymalo.actor";
/// Metadata key of a `RUN_ERROR` carrying the problem (`{type, title, detail?}`).
pub const PROBLEM_KEY: &str = "vymalo.problem";
/// Metadata key of live text (ADR 0027): on the `TEXT_MESSAGE_START` that opens a live message
/// (`{}`), on its `CONTENT` (`{offset}`: the UTF-16 code units already sent before the delta), on
/// the `CONTENT` and `END` of the log's final message that completes it (`{offset, final: true}`
/// and `{final: true}`), and on the `END` of a live message that was given up (`{abandoned: true}`).
pub const LIVE_KEY: &str = "vymalo.live";

/// Metadata key of what an agent's words are for (ADR 0031): `"working"` or `"answer"`, on the
/// `TEXT_MESSAGE_START` of an agent message whose `agent_message` says. Absent when it does not
/// (a plain A2A `Message`, and every log written before the field existed).
pub const PURPOSE_KEY: &str = "vymalo.purpose";
/// Metadata key of how an answer was announced (ADR 0031): `"turn_output"`, on the same `START`,
/// beside `vymalo.purpose` `"answer"`. Reserved: nothing writes it yet.
pub const VIA_KEY: &str = "vymalo.via";

/// The `forwardedProps` key that says how a message sent while a run is open is delivered (ADR
/// 0036): `"steer"` or `"interrupt"`. Without it such a run is refused (409).
pub const SEND_KEY: &str = "vymalo.send";

/// Metadata key of how a message reached an agent that was working (ADR 0036): `"steer"` or
/// `"interrupt"`, on the `TEXT_MESSAGE_START` of a user message the core logged with that
/// `delivery`. Absent on every other message (and on every log written before the field existed).
pub const DELIVERY_KEY: &str = "vymalo.delivery";

/// Metadata key of the agents a person mentioned in a message (ADR 0026, `mentions/v1`): the
/// references as the `user_message` stores them, `[{agentId, label, start, end, cardUrl?}]`
/// (offsets are UTF-16 code units into the message text), on the `TEXT_MESSAGE_START` of a user
/// message that has any. Absent when it has none, and in every log written before the field
/// existed.
pub const MENTIONS_KEY: &str = "vymalo.mentions";

/// `SUBAGENT_ERROR.code` of a sub-agent step that ended `failed`.
pub const CODE_STEP_FAILED: &str = "step_failed";
/// `SUBAGENT_ERROR.code` of an asked agent that ended `failed` or `rejected`.
pub const CODE_ASK_FAILED: &str = "ask_failed";
/// `SUBAGENT_ERROR.code` of an asked agent that did not answer by its deadline.
pub const CODE_ASK_TIMED_OUT: &str = "ask_timed_out";

/// `RUN_ERROR.code` for an agent that reported `failed` (or `rejected`).
pub const CODE_AGENT_FAILED: &str = "agent_failed";
/// `RUN_ERROR.code` for a delegation that could not be delivered.
pub const CODE_DELIVERY_FAILED: &str = "delivery_failed";
/// `SUBAGENT_ERROR.code` of the verifier's invocation when the verification is held because the
/// verifier could not be used or did not answer in time.
pub const CODE_VERIFIER_FAILED: &str = "verifier_failed";
/// `RUN_ERROR.code` for a job whose work did not pass the verification gate in its last attempt.
pub const CODE_CHECKS_FAILED: &str = "checks_failed";

/// The release-channels extension URI (ADR 0008): the key of `forwardedProps` that selects a
/// release when a thread is created.
pub const RELEASE_CHANNELS_URI: &str =
    "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

/// `{"vymalo.actor": {type, name, revision?}}`.
pub(crate) fn actor_metadata(actor: &Actor) -> Metadata {
    let mut metadata = Metadata::new();
    // `Actor` is plain strings and an enum: it always serialises.
    metadata.insert(
        ACTOR_KEY.to_owned(),
        serde_json::to_value(actor).unwrap_or(Value::Null),
    );
    metadata
}

/// The metadata of a user message's `TEXT_MESSAGE_START`: who sent it, how it reached the agent
/// when the log says (`vymalo.delivery`), and the agents it mentions (`vymalo.mentions`) when it
/// does: no member when it does not.
pub(crate) fn user_message_metadata(actor: &Actor, message: &UserMessageData) -> Metadata {
    let mut metadata = actor_metadata(actor);
    if let Some(delivery) = message.delivery {
        metadata.insert(DELIVERY_KEY.to_owned(), Value::from(delivery.as_str()));
    }
    if !message.mentions.is_empty() {
        metadata.insert(
            MENTIONS_KEY.to_owned(),
            // plain strings and numbers: it always serialises
            serde_json::to_value(&message.mentions).unwrap_or(Value::Null),
        );
    }
    metadata
}

/// The metadata of an agent message's `TEXT_MESSAGE_START`: who said it, and what it is for when
/// the log says (`vymalo.purpose`, `vymalo.via`: no member when it does not).
pub(crate) fn message_metadata(
    actor: &Actor,
    purpose: Option<MessagePurpose>,
    via: Option<AnswerVia>,
) -> Metadata {
    let mut metadata = actor_metadata(actor);
    if let Some(purpose) = purpose {
        metadata.insert(PURPOSE_KEY.to_owned(), Value::from(purpose.as_str()));
    }
    if let Some(via) = via {
        metadata.insert(VIA_KEY.to_owned(), Value::from(via.as_str()));
    }
    metadata
}

/// The `vymalo.status` content.
pub(crate) fn status_content(status: AgentStatus, detail: Option<&str>) -> Metadata {
    let mut content = Metadata::new();
    content.insert(
        "status".to_owned(),
        serde_json::to_value(status).unwrap_or(Value::Null),
    );
    if let Some(detail) = detail {
        content.insert("detail".to_owned(), Value::from(detail));
    }
    content
}

/// `{"vymalo.problem": {type, title, detail?}}`, in the shape of the API's RFC 9457 problems.
pub(crate) fn problem_metadata(title: &str, detail: &str) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert(
        PROBLEM_KEY.to_owned(),
        json!({"type": "about:blank", "title": title, "detail": detail}),
    );
    metadata
}

/// The schema of an interrupt's answer: `{text}`.
pub(crate) fn response_schema() -> Metadata {
    let mut schema = Metadata::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("required".to_owned(), json!(["text"]));
    schema.insert("properties".to_owned(), json!({"text": {"type": "string"}}));
    schema
}
