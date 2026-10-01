//! A scripted model: answers a test says it will give, in order, and remembers what it was asked.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{ChatModel, ChatRequest, ModelError};

/// What the scripted model does with the next question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStep {
    /// Answer with this text.
    Answer(String),
    /// Fail as an unreachable endpoint does (transient).
    Unreachable,
    /// Fail as a rate limited one does.
    RateLimited,
    /// Fail as one that refuses the request for good does.
    Reject,
    /// Fail as one that refuses the credential does.
    Unauthenticated,
    /// Fail as one that answers something unreadable does.
    Nonsense,
    /// Never answer (the caller's timeout is what ends the question).
    Hang,
}

#[derive(Debug)]
struct Inner {
    steps: VecDeque<ModelStep>,
    /// What to do when the script has run out.
    otherwise: ModelStep,
    calls: Vec<ChatRequest>,
}

/// The in-memory [`ChatModel`]. Cheap to clone: clones share the script and the record, so a
/// test keeps one to script and read while the application holds another.
#[derive(Debug, Clone)]
pub struct ScriptedModel {
    inner: Arc<Mutex<Inner>>,
}

impl Default for ScriptedModel {
    /// A model with no script, which says `NONE` (it has no topic to give) to every question.
    fn default() -> Self {
        ScriptedModel::new(ModelStep::Answer("NONE".to_owned()))
    }
}

impl ScriptedModel {
    /// A model that does `otherwise` once its script has run out.
    pub fn new(otherwise: ModelStep) -> Self {
        ScriptedModel {
            inner: Arc::new(Mutex::new(Inner {
                steps: VecDeque::new(),
                otherwise,
                calls: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next question gets `step` (after the ones already scripted).
    pub fn then(&self, step: ModelStep) -> &Self {
        self.lock().steps.push_back(step);
        self
    }

    /// The next question is answered with `text`.
    pub fn then_answer(&self, text: &str) -> &Self {
        self.then(ModelStep::Answer(text.to_owned()))
    }

    /// Every question the model was asked, oldest first.
    pub fn calls(&self) -> Vec<ChatRequest> {
        self.lock().calls.clone()
    }
}

impl ChatModel for ScriptedModel {
    async fn complete(&self, request: &ChatRequest) -> Result<String, ModelError> {
        let step = {
            let mut inner = self.lock();
            inner.calls.push(request.clone());
            inner
                .steps
                .pop_front()
                .unwrap_or_else(|| inner.otherwise.clone())
        };
        match step {
            ModelStep::Answer(text) => Ok(text),
            ModelStep::Unreachable => Err(ModelError::unreachable("scripted outage")),
            ModelStep::RateLimited => Err(ModelError::RateLimited {
                retry_after: Some(Duration::from_secs(1)),
            }),
            ModelStep::Reject => Err(ModelError::Rejected("scripted refusal".to_owned())),
            ModelStep::Unauthenticated => Err(ModelError::Unauthenticated),
            ModelStep::Nonsense => Err(ModelError::protocol("scripted nonsense")),
            ModelStep::Hang => std::future::pending().await,
        }
    }
}
