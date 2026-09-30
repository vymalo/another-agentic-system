//! The tools, as a closed enum (ADR 0004, ADR 0019): the names, the arguments and the schemas
//! the server advertises. What each tool does is in [`crate::server`].

use std::sync::Arc;

use rmcp::handler::server::common::schema_for_input;
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Every tool the server offers. A new tool is a new variant, so the compiler finds every place
/// that must know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    /// The configured agents.
    ListAgents,
    /// Starts a job.
    StartJob,
    /// A summary of a job.
    GetJob,
    /// Follows a job until it is over or blocked, with progress notifications.
    WaitForJob,
    /// Sends a message to a job (an answer to a question, a follow-up, or the next request on a
    /// finished job, which starts the thread's next job).
    Answer,
    /// Cancels a job.
    CancelJob,
}

impl ToolName {
    /// Every tool, in the order `tools/list` gives them.
    pub const ALL: &'static [ToolName] = &[
        ToolName::ListAgents,
        ToolName::StartJob,
        ToolName::GetJob,
        ToolName::WaitForJob,
        ToolName::Answer,
        ToolName::CancelJob,
    ];

    /// The name on the wire.
    pub const fn name(self) -> &'static str {
        match self {
            ToolName::ListAgents => "list_agents",
            ToolName::StartJob => "start_job",
            ToolName::GetJob => "get_job",
            ToolName::WaitForJob => "wait_for_job",
            ToolName::Answer => "answer",
            ToolName::CancelJob => "cancel_job",
        }
    }

    /// The tool a wire name stands for.
    pub fn parse(name: &str) -> Option<ToolName> {
        Self::ALL.iter().copied().find(|t| t.name() == name)
    }

    const fn description(self) -> &'static str {
        match self {
            ToolName::ListAgents => {
                "List the agents that can be given a job: id, name and description. The first one \
                 is the default of start_job."
            }
            ToolName::StartJob => {
                "Start a job: give an agent a task. Returns at once with the job_id and its state; \
                 use get_job or wait_for_job to look at it. Set client_request_id to make a retry \
                 safe: the same client_request_id with the same request returns the same job \
                 instead of starting another, and a different request is refused."
            }
            ToolName::GetJob => {
                "Summarise a job: its state, which job of the thread it is (job), the attempt, the \
                 branch and commit the agent pushed, the pull request, the last CI result and any \
                 findings that failed a check. All of it is about the thread's current job."
            }
            ToolName::WaitForJob => {
                "Wait for a job: returns when it is finished or blocked (waiting for your answer), \
                 or after timeout_secs (a finished job is not the end of the thread: answer starts \
                 the next job, and you wait again), with the same summary as get_job plus outcome \
                 (finished, blocked, timed_out, interrupted) and resume_after_seq. Progress \
                 notifications report each event as it happens when the request has a \
                 progressToken; without one the wait is cut to the server's heartbeat interval \
                 (60 s by default), because nothing could keep the connection alive. To keep \
                 waiting, call again with after_seq = resume_after_seq: nothing is lost. An \
                 interrupted outcome (the server is restarting) carries retry_after_secs."
            }
            ToolName::Answer => {
                "Send a message to a job: the answer to a question the agent asked (state \
                 blocked), or a follow-up while it works. A message to a finished job (done, \
                 failed or cancelled) starts the thread's next job with the same agent and the \
                 same job_id; the answer says which job (job). Wait for it again."
            }
            ToolName::CancelJob => {
                "Cancel the job that is running. Cancelling a job that is already finished \
                 changes nothing; a later answer still starts the next job."
            }
        }
    }

    fn annotations(self) -> ToolAnnotations {
        match self {
            ToolName::ListAgents | ToolName::GetJob | ToolName::WaitForJob => {
                ToolAnnotations::new().read_only(true).open_world(false)
            }
            ToolName::StartJob => ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(true),
            ToolName::Answer => ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(true),
            ToolName::CancelJob => ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(true)
                .open_world(true),
        }
    }

    fn input_schema(self) -> Arc<JsonObject> {
        match self {
            ToolName::ListAgents => schema_for_input::<NoArgs>(),
            ToolName::StartJob => schema_for_input::<StartJobArgs>(),
            ToolName::GetJob => schema_for_input::<GetJobArgs>(),
            ToolName::WaitForJob => schema_for_input::<WaitForJobArgs>(),
            ToolName::Answer => schema_for_input::<AnswerArgs>(),
            ToolName::CancelJob => schema_for_input::<CancelJobArgs>(),
        }
        // The argument types are ours and are objects; a failure here is a bug that the first
        // test of `tools/list` finds.
        .unwrap_or_else(|reason| {
            panic!("the input schema of {} is unusable: {reason}", self.name())
        })
    }

    /// The definition `tools/list` gives.
    pub fn definition(self) -> Tool {
        Tool::new(self.name(), self.description(), self.input_schema()).annotate(self.annotations())
    }
}

/// `list_agents` takes nothing.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

/// The verification gate a job asks for (ADR 0018): the same object as AG-UI's
/// `forwardedProps["vymalo.gate"]`, checked by the same rules. It may add sources and raise or
/// lower the attempts within the deployment's cap; it never removes a source the deployment
/// requires. Fixed when the job is created.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateArgs {
    /// Sources that must pass before the job is done: `agent-checks` (the agent's own checks).
    /// `ci` and `verifier` are named here too, but a deployment that cannot honour them refuses
    /// the job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require: Option<Vec<String>>,
    /// Attempts the agent gets, the first included, within the deployment's cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

impl GateArgs {
    /// The layer the application checks. `Err` is a reason to show the caller.
    pub fn to_layer(&self) -> Result<orch_app::GateLayer, String> {
        let value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        match orch_app::GateLayer::from_json(&value) {
            Ok(Some(layer)) => Ok(layer),
            Ok(None) => Ok(orch_app::GateLayer::default()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The arguments of `start_job`.
///
/// Unknown arguments are refused, not ignored: a client must hear that an argument did not
/// apply, not run a job believing it had been given.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartJobArgs {
    /// What the agent should do. Plain text.
    pub text: String,
    /// The agent's id from list_agents. Default: the first agent.
    #[serde(default)]
    pub agent: Option<String>,
    /// A short title for the job. Default: the first line of the text.
    #[serde(default)]
    pub title: Option<String>,
    /// Your own id for this request, at most 256 bytes. A retry with the same id returns the job
    /// the first call started. Without it every call starts a new job.
    #[serde(default)]
    pub client_request_id: Option<String>,
    /// The verification gate for this job. Omitted: the deployment's gate. Refused, naming the
    /// reason, when the deployment cannot honour it.
    #[serde(default)]
    pub gate: Option<GateArgs>,
}

/// The arguments of `get_job`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetJobArgs {
    /// The job_id from start_job.
    pub job_id: String,
}

/// The arguments of `wait_for_job`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitForJobArgs {
    /// The job_id from start_job.
    pub job_id: String,
    /// Report the events after this sequence number. Omitted: only what happens from now on. 0:
    /// the whole log. Use the resume_after_seq of an earlier call to continue it.
    #[serde(default)]
    pub after_seq: Option<i64>,
    /// How long to wait, in seconds; at most the server's limit (larger values are cut to it).
    /// 0 reports what the log holds now and returns.
    pub timeout_secs: u64,
}

/// The arguments of `answer`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerArgs {
    /// The job_id from start_job.
    pub job_id: String,
    /// The message: the answer to the agent's question, or a follow-up. Plain text.
    pub text: String,
}

/// The arguments of `cancel_job`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelJobArgs {
    /// The job_id from start_job.
    pub job_id: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_unique_name_that_parses_back() {
        let mut names: Vec<&str> = ToolName::ALL.iter().map(|t| t.name()).collect();
        for tool in ToolName::ALL {
            assert_eq!(ToolName::parse(tool.name()), Some(*tool));
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ToolName::ALL.len());
        assert_eq!(ToolName::parse("wait_for_job"), Some(ToolName::WaitForJob));
        assert_eq!(ToolName::parse("START_JOB"), None, "names are exact");
    }

    #[test]
    fn every_definition_is_an_object_schema_that_names_its_required_arguments() {
        for tool in ToolName::ALL {
            let def = tool.definition();
            assert_eq!(def.input_schema["type"], "object", "{}", tool.name());
            assert!(def.description.is_some());
        }
        let start = ToolName::StartJob.definition();
        assert_eq!(start.input_schema["required"], serde_json::json!(["text"]));
        assert!(start.input_schema["properties"]["client_request_id"].is_object());
        assert_eq!(
            start.input_schema["$defs"]["GateArgs"]["properties"]
                .as_object()
                .map(|p| p.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default(),
            ["maxAttempts", "require"],
            "the gate object is {{require?, maxAttempts?}}"
        );
        let get = ToolName::GetJob.definition();
        assert_eq!(get.input_schema["required"], serde_json::json!(["job_id"]));
        let wait = ToolName::WaitForJob.definition();
        assert_eq!(
            wait.input_schema["required"],
            serde_json::json!(["job_id", "timeout_secs"])
        );
        assert!(wait.input_schema["properties"]["after_seq"].is_object());
    }

    #[test]
    fn a_gate_becomes_the_layer_the_application_checks() {
        let args: StartJobArgs = serde_json::from_value(serde_json::json!(
            {"text": "go", "gate": {"require": ["agent-checks"], "maxAttempts": 2}}
        ))
        .unwrap();
        let layer = args.gate.unwrap().to_layer().unwrap();
        assert_eq!(layer.max_attempts, Some(2));
        assert_eq!(layer.require.as_ref().map(Vec::len), Some(1));
        let bad: GateArgs =
            serde_json::from_value(serde_json::json!({"require": ["magic"]})).unwrap();
        let reason = bad.to_layer().unwrap_err();
        assert!(reason.contains("magic"), "{reason}");
    }

    #[test]
    fn unknown_arguments_are_refused() {
        let err = serde_json::from_value::<StartJobArgs>(
            serde_json::json!({"text": "go", "gates": {"require": ["ci"]}}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("gates"), "{err}");
        // Inside the gate too: a verifier or CI setting is not a thread's to make.
        assert!(
            serde_json::from_value::<StartJobArgs>(
                serde_json::json!({"text": "go", "gate": {"verifier": "v"}})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<GetJobArgs>(serde_json::json!({})).is_err());
    }
}
