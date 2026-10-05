use serde::{Deserialize, Serialize};

use crate::event::{FileRef, MessagePurpose};
use crate::step::StepReport;

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

/// Why a file an agent handed over was not kept (ADR 0032).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRefusal {
    /// Over `artifacts.maxFileBytes`.
    TooLarge,
    /// The job has kept as many files, or as many bytes, as it may.
    JobLimit,
    /// Not stored: no store is configured, or the store failed.
    NotKept,
}

impl FileRefusal {
    /// What the people of the thread read in the `error` event.
    pub const fn message(self) -> &'static str {
        match self {
            FileRefusal::TooLarge => "the file is too large to keep",
            FileRefusal::JobLimit => {
                "this job has reached its limit of files, so the file is not kept"
            }
            FileRefusal::NotKept => "the file could not be kept",
        }
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
    /// A file the agent handed over, with its bytes (an A2A `raw` part, or a `url` part the worker
    /// fetched: ADR 0032). It is what an adapter reports; **the worker keeps it and replaces it
    /// with [`AgentUpdate::FileKept`] or [`AgentUpdate::FileRefused`] before the core sees it**, so
    /// the bytes never reach the log. A core that is handed one anyway has no store to put it in:
    /// it logs the file as refused ([`FileRefusal::NotKept`]).
    File {
        /// The artifact's name.
        name: String,
        /// The media type the agent declared, if it did.
        media_type: Option<String>,
        /// The file's name, if the agent gave one.
        filename: Option<String>,
        /// The content.
        bytes: Vec<u8>,
    },
    /// A file the worker put into the artifact store (ADR 0032): logged as an artifact that holds
    /// the reference.
    FileKept {
        /// The artifact's name.
        name: String,
        /// The media type the worker sniffed.
        mime_type: String,
        /// Where the file is.
        file: FileRef,
    },
    /// A file the worker did not keep: logged as an artifact without a file, and an error that says
    /// why. The turn goes on.
    FileRefused {
        /// The artifact's name.
        name: String,
        /// The media type the agent declared, if it did.
        mime_type: Option<String>,
        /// Why.
        reason: FileRefusal,
    },
    /// A message from the agent.
    Message {
        /// Stable id; a later message with the same id replaces a partial one.
        message_id: String,
        /// Message text.
        text: String,
        /// `false` for a streamed partial.
        is_final: bool,
        /// What the words are for in the turn, when the agent's protocol says (ADR 0031): from
        /// the status they were stated on. `None` for a plain A2A `Message`.
        purpose: Option<MessagePurpose>,
    },
    /// What the agent's model thought before one of its turns (ADR 0044, a `text-stream/v1`
    /// stream marked `kind: "reasoning"`), whole: the adapter collected the chunks. The core logs it
    /// as an `agent_reasoning`, bounded again ([`bound_reasoning`](crate::bound_reasoning)). It is not
    /// the agent's words: it never counts as a message, an answer or a summary.
    Reasoning {
        /// The reasoning stream's id: the id of the live reasoning the screen showed.
        message_id: String,
        /// The reasoning.
        text: String,
        /// The text is not the whole of it: a piece was lost, the agent gave up, or the adapter cut it.
        truncated: bool,
    },
    /// An A2UI payload the agent sent, already through the envelope check
    /// ([`check_operations`](crate::check_operations)).
    Ui {
        /// The A2UI messages, as sent.
        operations: Vec<serde_json::Value>,
    },
    /// An A2UI payload the agent sent that failed the envelope check. It is recorded as an
    /// error and never passed on; the rest of the turn goes on.
    UiRejected {
        /// The rule it broke, worded for the people who see the thread.
        reason: String,
    },
    /// A step of the agent's work (`steps/v1`, ADR 0025). The adapter has made its ids unique
    /// within the thread; the core sanitizes it again and coalesces it
    /// ([`record_step`](crate::record_step)).
    Step(StepReport),
}
