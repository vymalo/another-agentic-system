//! The chat conformance kit over the embedded home (`orch-embed`): the four cases of the chat
//! contract of `docs/api/agui.md`, run against `Embedded<ScriptedAgent>` on a SQLite file.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::result_large_err)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt as _;
use orch_app::{AgentDirectory, AgentEntry};
use orch_chat_kit::{
    ChatHome, Frame, HomeError, LimitRule, Sent, Setup, UserMessage, chat_conformance,
};
use orch_core::client_version::ClientVersion;
use orch_core::{AgentId, EventBody, EventKind, ThreadId, UserId};
use orch_embed::{Admission, EmbedError, EmbedPorts, Embedded, Limit};
use orch_ports::AgentEndpoint;
use orch_ports::memory::ScriptedAgent;

/// The static directory the embedded application serves: one agent, the scripted one.
fn directory(agent: &AgentId) -> AgentDirectory {
    AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            agent.clone(),
            "https://helper.invalid/.well-known/agent-card.json",
            None,
        ),
        name: "Helper".to_owned(),
    }])
}

/// The application's limit: always reached, for the 402 case.
#[derive(Debug)]
struct Full;

impl Admission for Full {
    fn admit(&self, _: &UserId) -> Result<(), Limit> {
        Err(Limit {
            code: "limit_reached".to_owned(),
            message: "no more for now".to_owned(),
        })
    }
}

/// A fresh SQLite file per home: the cases run side by side in one test binary.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// The embedded home: one `Embedded` over the scripted agent, and the agent it serves.
struct EmbedHome {
    embed: Embedded<ScriptedAgent>,
    agent: AgentId,
    limited: bool,
}

async fn make(setup: Setup) -> Option<EmbedHome> {
    let agent = AgentId::new("helper");
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("orch-chat-kit-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join("orch.sqlite");
    let _ = std::fs::remove_file(&file);
    let admission: Option<Arc<dyn Admission>> = if setup.limit {
        Some(Arc::new(Full))
    } else {
        None
    };
    let parts = EmbedPorts {
        min_client_version: setup
            .min_client_version
            .and_then(|version| version.parse::<ClientVersion>().ok()),
        admission,
        ..EmbedPorts::default()
    };
    let mut embed = Embedded::open_with(
        file,
        UserId::new("kit@example.invalid"),
        directory(&agent),
        ScriptedAgent::new(),
        parts,
    )
    .await
    .ok()?;
    if let Some(version) = setup.client_version {
        embed = embed.with_client_version(version);
    }
    Some(EmbedHome {
        embed,
        agent,
        limited: setup.limit,
    })
}

/// The kit's error for the embed's own.
fn home_error(error: EmbedError) -> HomeError {
    match error {
        EmbedError::LimitReached(limit) => HomeError::Limit {
            event: limit.event(),
        },
        EmbedError::ClientUpdateRequired { min } => HomeError::UpdateRequired {
            min: min.to_string(),
        },
        other => HomeError::Other(other.to_string()),
    }
}

/// The thread id the home spelled, parsed back for the embed's typed calls.
fn thread_id(thread: &str) -> Result<ThreadId, HomeError> {
    thread
        .parse::<ThreadId>()
        .map_err(|error| HomeError::Other(format!("{thread:?} is not a thread id: {error}")))
}

/// The frames of the embed's projected stream, read to its end.
async fn collect(
    mut stream: futures::stream::BoxStream<'static, orch_agui_projection::Frame>,
) -> Vec<Frame> {
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        frames.push(Frame {
            resume_id: frame.resume_id,
            event: frame.event,
        });
    }
    frames
}

impl ChatHome for EmbedHome {
    fn limit_rule(&self) -> LimitRule {
        // The embed home enforces a limit when the application gave it an `Admission` (the 402
        // rule), and has none otherwise.
        if self.limited {
            LimitRule::Enforced
        } else {
            LimitRule::NotApplicable("this home was built without an Admission")
        }
    }

    async fn send(
        &self,
        thread: Option<&str>,
        message_id: &str,
        text: &str,
    ) -> Result<Sent, HomeError> {
        match thread {
            None => {
                let created = self
                    .embed
                    .create_thread(&self.agent, text)
                    .await
                    .map_err(home_error)?;
                let stream = self
                    .embed
                    .stream_agui_from(created.id, 0)
                    .await
                    .map_err(home_error)?;
                Ok(Sent {
                    thread: created.id.to_string(),
                    frames: collect(stream).await,
                })
            }
            Some(thread) => {
                let id = thread_id(thread)?;
                let event = self
                    .embed
                    .post_message_with_id(id, message_id, text)
                    .await
                    .map_err(home_error)?;
                // The run this message opened starts at the message's own log event; folding up
                // to the event before it makes the stream begin with that run's `RUN_STARTED`.
                let stream = self
                    .embed
                    .stream_agui_from(id, event.seq.saturating_sub(1))
                    .await
                    .map_err(home_error)?;
                Ok(Sent {
                    thread: thread.to_owned(),
                    frames: collect(stream).await,
                })
            }
        }
    }

    async fn replay(&self, thread: &str, after: i64) -> Result<Vec<Frame>, HomeError> {
        let id = thread_id(thread)?;
        let stream = self
            .embed
            .stream_agui_from(id, after)
            .await
            .map_err(home_error)?;
        Ok(collect(stream).await)
    }

    async fn user_messages(&self, thread: &str) -> Result<Vec<UserMessage>, HomeError> {
        let id = thread_id(thread)?;
        let events = self.embed.events(id).await.map_err(home_error)?;
        Ok(events
            .into_iter()
            .filter(|event| event.kind() == EventKind::UserMessage)
            .filter_map(|event| {
                let seq = event.seq;
                match event.body {
                    EventBody::UserMessage(message) => Some(UserMessage {
                        id: message.message_id.unwrap_or_else(|| format!("evt-{seq}")),
                        text: message.text,
                    }),
                    _ => None,
                }
            })
            .collect())
    }
}

chat_conformance!(make);
