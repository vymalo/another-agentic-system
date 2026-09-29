use serde::{Deserialize, Serialize};

/// Protocol-neutral task state reported by an agent (mirrors A2A `TaskState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskState {
    /// The agent accepted the task.
    Submitted,
    /// The agent is working.
    Working,
    /// The agent needs more input from the user.
    InputRequired,
    /// The agent needs the user to authenticate somewhere.
    AuthRequired,
    /// The task finished successfully.
    Completed,
    /// The task failed.
    Failed,
    /// The task was cancelled.
    Canceled,
    /// The agent refused the task.
    Rejected,
}

impl AgentTaskState {
    /// No further updates will follow for this task.
    pub fn is_terminal(self) -> bool {
        match self {
            AgentTaskState::Completed
            | AgentTaskState::Failed
            | AgentTaskState::Canceled
            | AgentTaskState::Rejected => true,
            AgentTaskState::Submitted
            | AgentTaskState::Working
            | AgentTaskState::InputRequired
            | AgentTaskState::AuthRequired => false,
        }
    }

    /// The task waits for the user.
    pub fn is_interrupted(self) -> bool {
        match self {
            AgentTaskState::InputRequired | AgentTaskState::AuthRequired => true,
            AgentTaskState::Submitted
            | AgentTaskState::Working
            | AgentTaskState::Completed
            | AgentTaskState::Failed
            | AgentTaskState::Canceled
            | AgentTaskState::Rejected => false,
        }
    }

    /// The agent has nothing more to say for this turn (terminal or waiting for the user).
    pub fn ends_turn(self) -> bool {
        self.is_terminal() || self.is_interrupted()
    }
}

/// One thing an agent told us, already stripped of protocol framing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentUpdate {
    /// A task state change.
    Status {
        /// The new state.
        state: AgentTaskState,
        /// Human readable detail (the agent's status message).
        detail: Option<String>,
    },
    /// A produced artifact (a branch, a PR URL, a patch, …).
    Artifact {
        /// Artifact name.
        name: String,
        /// Media type of the payload.
        mime_type: Option<String>,
        /// Where the artifact lives.
        uri: Option<String>,
        /// Inline text content.
        text: Option<String>,
    },
    /// A message from the agent.
    Message {
        /// Stable id; a later message with the same id replaces a partial one.
        message_id: String,
        /// Message text.
        text: String,
        /// `false` for a streamed partial.
        is_final: bool,
    },
}
