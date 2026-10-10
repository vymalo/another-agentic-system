//! The settings of the orchestrator's own model calls, the utility tasks (ADR 0035).
//!
//! `AppConfig::tasks` maps each [`TaskKind`] to the [`TaskSettings`] it runs with. A task that is
//! absent is off: the application writes no outbox row for it and no model is ever asked. The
//! settings are what the configuration file says, validated before they get here (an endpoint that
//! exists, a guidance that is not empty), plus the bound of one try, which is the endpoint's
//! timeout. The core's prompt parts that no configuration can remove are not settings.

use std::time::Duration;

use orch_core::{DEFAULT_DESCRIPTION_CHARS, DEFAULT_MIN_NEW_MESSAGES, LanguageRule, TaskKind};

/// How one task asks its model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSettings {
    /// The endpoint to ask: a name of `models.endpoints`, which the model port resolves.
    pub endpoint: String,
    /// The model's name at that endpoint.
    pub model: String,
    /// The guidance that replaces the core's own (what to write and in what style), read from the
    /// configuration once; `None` is the core's guidance.
    pub guidance: Option<String>,
    /// Most tokens of answer.
    pub max_tokens: u32,
    /// The language rule.
    pub language: LanguageRule,
    /// The longest one try may take, whatever the adapter does: the endpoint's timeout.
    pub timeout: Duration,
    /// Description only: where a description is cut, at a word.
    pub max_chars: usize,
    /// Description only: how many messages the conversation has to have grown by since the last
    /// description before a new one is asked for.
    pub min_new_messages: u32,
}

impl TaskSettings {
    /// The settings of `kind` at `endpoint` with `model`, and the defaults of ADR 0035 for the
    /// rest: the core's guidance, 32 tokens for a title and 160 for a description, the person's
    /// language, a 20 second timeout, 300 characters and 4 new messages.
    pub fn new(kind: TaskKind, endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        TaskSettings {
            endpoint: endpoint.into(),
            model: model.into(),
            guidance: None,
            max_tokens: match kind {
                TaskKind::Title => 32,
                TaskKind::Description => 160,
            },
            language: LanguageRule::Conversation,
            timeout: Duration::from_secs(20),
            max_chars: DEFAULT_DESCRIPTION_CHARS,
            min_new_messages: DEFAULT_MIN_NEW_MESSAGES,
        }
    }

    /// The same with another bound for one try.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The public subset of the configuration, which the web reads from `GET /api/config` (ADR 0034).
/// It is the `ui` section of the file with its effective values; nothing else of the file is ever
/// in it, and no field of it is a secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PublicConfig {
    /// The `ui` section.
    pub ui: UiSettings,
}

/// The `ui` section: what the web is told about how to show things. A key is added here, in the
/// configuration's `ui` type and in the contract together, and never renamed or retyped in
/// `version: 1`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    /// `ui.showDescriptions`: whether the web shows a thread's description (ADR 0035). The API
    /// returns the description either way.
    pub show_descriptions: bool,
    /// `ui.history` (ADR 0059): how the web opens a long thread. **Present only when the process
    /// serves the history route**, which is its presence being the capability: an older
    /// orchestrator, or one without the AG-UI surface, leaves it out and the web replays the log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<UiHistory>,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            show_descriptions: true,
            history: None,
        }
    }
}

/// `ui.history`: what the web asks for when it opens a thread at its end, and the version of the
/// projection that writes the frames it gets (ADR 0059, `docs/api/history.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiHistory {
    /// The turns the web asks for when it opens a thread.
    pub initial_turns: u32,
    /// The turns of the first older page; each later one asks for more, up to `max_turns`.
    pub page_turns: u32,
    /// The most turns a page holds (`server.history.maxTurns`): the largest `limit` the server accepts,
    /// which the pages that grow stop at.
    pub max_turns: u32,
    /// `PROJECTION_VERSION` of this build: the version of the frames the routes write.
    pub projection: u32,
    /// Whether the web opens a thread from its history.
    pub windowed: bool,
}

impl Default for UiHistory {
    fn default() -> Self {
        UiHistory {
            initial_turns: 12,
            page_turns: 20,
            max_turns: 100,
            projection: 0,
            windowed: true,
        }
    }
}
