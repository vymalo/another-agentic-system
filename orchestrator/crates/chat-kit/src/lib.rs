//! The chat contract of an application client (`docs/api/agui.md`, "Chat contract of an
//! application client") as an executable conformance kit: one trait over a chat home, four cases,
//! and a macro that writes one test per case.
//!
//! A *home* is an implementation of [`ChatHome`]: the orchestrator reached over HTTP
//! (`orch-surface-agui` over `orch-api`) or embedded in an application (`orch-embed`). This crate
//! names no transport and no adapter; a consuming crate implements [`ChatHome`] and runs
//! [`chat_conformance!`].
//!
//! # The four cases
//!
//! 1. [`cases::replay_from_a_seq_says_only_what_follows`]: a replay from a sequence number says
//!    only what follows it, with no gap and no repeat.
//! 2. [`cases::a_client_message_id_makes_a_retry_one_message`]: the client message id is the
//!    idempotency key, so a retry is one message.
//! 3. [`cases::a_limit_is_one_custom_event`]: a 402 is one `CUSTOM` event named
//!    `vymalo.limit_reached`, carrying `{status: 402, code, message}`.
//! 4. [`cases::an_old_or_unstated_client_is_refused`]: 426 for a client below the home's minimum,
//!    an unreadable one or none.
//!
//! # Using it
//!
//! ```ignore
//! use orch_chat_kit::{Setup, chat_conformance};
//!
//! async fn make(setup: Setup) -> Option<MyHome> {
//!     // a fresh, isolated home configured as `setup` says, or `None` when it is unavailable
//!     # todo!()
//! }
//!
//! chat_conformance!(make);
//! ```
//!
//! The consuming crate needs `tokio` (features `macros` and `rt`) as a dev-dependency: the macro
//! writes `#[tokio::test]`.

#![forbid(unsafe_code)]
// The trait is the chat contract's shape: native `async fn`, generic and never `dyn`. A `Send`
// bound would be the consumer's business, and the kit's own tests run on the current-thread
// runtime.
#![allow(async_fn_in_trait)]
// `HomeError::Limit` carries an `agui::Event`, which is larger than clippy's default 128-byte
// threshold for a `Result` error. The error is a case's outcome, never a hot path, and boxing it
// would hide the protocol type the contract names.
#![allow(clippy::result_large_err)]

use std::fmt;

use orch_agui_proto as agui;

pub mod cases;
pub mod double;

/// What a home is configured with: the application's limit, and the version gate.
///
/// `Default` is a home with no limit, no minimum and no stated client version: the ordinary
/// deployment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Setup {
    /// Whether the home must refuse new work with the 402 limit event.
    pub limit: bool,
    /// `server.minClientVersion`: the lowest client version served, when the home has one.
    pub min_client_version: Option<&'static str>,
    /// The version the home states for itself (`X-Client-Version`, `Embedded::with_client_version`).
    pub client_version: Option<&'static str>,
}

/// What a home can say about the 402 rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitRule {
    /// The home enforces an application-level limit and answers with the 402 event.
    Enforced,
    /// The home has no application-level limit, for the reason given.
    NotApplicable(&'static str),
}

/// One AG-UI frame and the sequence a client may resume from after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The `id:` of the frame, when it is a resume point.
    pub resume_id: Option<i64>,
    /// The AG-UI event.
    pub event: agui::Event,
}

/// A message a home accepted, and the frames of the run it started.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    /// The thread's id, as the home spells it.
    pub thread: String,
    /// The run's frames, read to the run's end.
    pub frames: Vec<Frame>,
}

/// One user message as the home's log holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessage {
    /// The id the client gave it, or the home's own id when it has none.
    pub id: String,
    /// Its text.
    pub text: String,
}

/// What a home refused, or failed, to do.
#[derive(Debug, thiserror::Error)]
#[allow(clippy::large_enum_variant)] // the 402 variant carries the protocol event, as the contract names it
pub enum HomeError {
    /// The application's limit is reached (402): the one event that says so.
    #[error("the application's limit is reached")]
    Limit {
        /// The `CUSTOM` event the home would send.
        event: agui::Event,
    },
    /// The client is below the home's minimum, or stated no readable version (426).
    #[error("update the application: version {min} or later is needed")]
    UpdateRequired {
        /// The lowest version served.
        min: String,
    },
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

/// Why a case failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(String);

impl Failure {
    /// A failure with a message for the person reading the test output.
    pub fn new(message: impl Into<String>) -> Self {
        Failure(message.into())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

/// A chat home: the orchestrator reached over HTTP, or embedded in an application.
///
/// The trait is generic and never used as `dyn`: the cases take a factory that returns the
/// concrete home. Implementations are asynchronous with native `async fn` in the trait.
pub trait ChatHome {
    /// What the home can say about the 402 rule.
    fn limit_rule(&self) -> LimitRule;

    /// Sends `text` as a user message named `message_id`: `None` mints a fresh thread, `Some`
    /// continues it. The frames are the run's stream read to its end.
    ///
    /// # Errors
    /// [`HomeError::Limit`], [`HomeError::UpdateRequired`], or [`HomeError::Other`].
    async fn send(
        &self,
        thread: Option<&str>,
        message_id: &str,
        text: &str,
    ) -> Result<Sent, HomeError>;

    /// Replays the frames after the log sequence `after` (0 is the start).
    ///
    /// # Errors
    /// [`HomeError::UpdateRequired`] or [`HomeError::Other`].
    async fn replay(&self, thread: &str, after: i64) -> Result<Vec<Frame>, HomeError>;

    /// The user messages of `thread`, oldest first.
    ///
    /// # Errors
    /// [`HomeError::UpdateRequired`] or [`HomeError::Other`].
    async fn user_messages(&self, thread: &str) -> Result<Vec<UserMessage>, HomeError>;
}

/// Generates one `#[tokio::test]` per conformance case, named after it.
///
/// `$make` is a path to a function `fn(Setup) -> impl Future<Output = Option<H>>` (an `async fn`
/// is one): it builds a fresh, isolated home for the `Setup` a case asks for, or returns `None`
/// when the home is unavailable (the case then prints `skipped: …` and passes).
///
/// The consuming crate needs `tokio` (features `macros` and `rt`) as a dev-dependency.
#[macro_export]
macro_rules! chat_conformance {
    ($make:path) => {
        $crate::chat_conformance!(@case $make, replay_from_a_seq_says_only_what_follows);
        $crate::chat_conformance!(@case $make, a_client_message_id_makes_a_retry_one_message);
        $crate::chat_conformance!(@case $make, a_limit_is_one_custom_event);
        $crate::chat_conformance!(@case $make, an_old_or_unstated_client_is_refused);
    };
    (@case $make:path, $case:ident) => {
        #[tokio::test]
        async fn $case() {
            match $crate::cases::$case($make).await {
                Ok(()) => {}
                Err(failure) => panic!("{failure}"),
            }
        }
    };
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A home over a fixed script: a run of `RUN_STARTED`, five frames with resume ids 1 to 5,
    /// and `RUN_FINISHED` at 6.
    struct Scripted {
        frames: Vec<Frame>,
    }

    impl Scripted {
        fn new() -> Self {
            let event = |value: serde_json::Value| -> agui::Event {
                serde_json::from_value(value).expect("a well-formed AG-UI event")
            };
            let mut frames = vec![Frame {
                resume_id: None,
                event: event(serde_json::json!({
                    "type": "RUN_STARTED", "threadId": "thread-1", "runId": "run-1"
                })),
            }];
            for seq in 1..=5 {
                frames.push(Frame {
                    resume_id: Some(seq),
                    event: event(serde_json::json!({
                        "type": "CUSTOM", "name": "vymalo.test", "value": {"seq": seq}
                    })),
                });
            }
            frames.push(Frame {
                resume_id: Some(6),
                event: event(serde_json::json!({
                    "type": "RUN_FINISHED", "threadId": "thread-1", "runId": "run-1"
                })),
            });
            Scripted { frames }
        }
    }

    impl ChatHome for Scripted {
        fn limit_rule(&self) -> LimitRule {
            LimitRule::NotApplicable("the fake has no limit")
        }

        async fn send(
            &self,
            _thread: Option<&str>,
            _message_id: &str,
            _text: &str,
        ) -> Result<Sent, HomeError> {
            Ok(Sent {
                thread: "thread-1".to_owned(),
                frames: self.frames.clone(),
            })
        }

        async fn replay(&self, _thread: &str, after: i64) -> Result<Vec<Frame>, HomeError> {
            let mut frames = vec![self.frames[0].clone()];
            frames.extend(
                self.frames
                    .iter()
                    .filter(|frame| frame.resume_id.is_some_and(|seq| seq > after))
                    .cloned(),
            );
            Ok(frames)
        }

        async fn user_messages(&self, _thread: &str) -> Result<Vec<UserMessage>, HomeError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn a_repeated_replay_frame_fails_the_replay_case() {
        let outcome = crate::cases::replay_from_a_seq_says_only_what_follows(|_| async {
            Some(crate::double::RepeatedReplay(Scripted::new()))
        })
        .await;
        let failure = outcome.expect_err("a repeated frame must fail the case");
        let message = failure.to_string();
        assert!(message.contains("repeated"), "{message}");
        assert!(
            message.contains('3'),
            "the repeated seq is named: {message}"
        );
    }

    #[tokio::test]
    async fn a_gapped_replay_fails_the_replay_case() {
        let outcome = crate::cases::replay_from_a_seq_says_only_what_follows(|_| async {
            Some(crate::double::GappedReplay(Scripted::new()))
        })
        .await;
        assert!(outcome.is_err(), "a gap in the replay must fail the case");
    }
}
