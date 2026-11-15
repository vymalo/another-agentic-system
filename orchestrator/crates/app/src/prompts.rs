//! A person's prompt library: what the application does with it (ADR 0070,
//! `docs/api/prompts-v1.md`). The HTTP routes call these methods and nothing else; no handler is
//! given the port.
//!
//! # Who may
//!
//! - **Save, read, list, render, unshare and delete one's own prompts need no permission** beyond
//!   the access every route needs (the API answers `no_access` to a person whose roles grant
//!   nothing).
//! - **A write whose result is shared** (a non-empty audience) needs `prompts.share`
//!   ([`Permission::PromptsShare`]), which no role holds by default and `admin` does not imply. So
//!   an edit that leaves an existing share in place, by a person who no longer holds it, is
//!   refused: only `audience: []` and a delete are accepted from them.
//! - **Only the owner writes.** A prompt that is not the caller's, shared with them or not, is
//!   [`PromptRefusal::NotFound`] for an edit and a delete; one outside the audience is the same for
//!   a read and a render, the answer for an id that never existed.
//!
//! # The order of the checks
//!
//! `create`: the permission, the values ([`orch_core::prompt::validate`]), then the store (the name,
//! the limit). `edit`: the prompt (404), the permission, the revision (409), the values of the whole
//! prompt as it would be, then the store (the name). `render`: the prompt (404), the revision (409),
//! the answer (422), the size (422). Nothing here logs or quotes a word of a prompt or an answer.

use std::collections::BTreeSet;

use orch_core::prompt::{
    AnswerRefusal, PromptAudience, PromptDefinition, PromptError, PromptId, PromptInput,
    RenderError, render, validate_answers,
};
use orch_ports::{
    Clock, IdGen, PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError,
    PromptViewer, StoredPrompt,
};
use serde_json::Value;

use crate::app::App;
use crate::authz::{Permission, Requester, Resource};
use crate::error::AppError;

/// How many times a save is tried with a new id when the id turns out to be taken.
const ID_ATTEMPTS: usize = 4;

/// Why the library refused a request. Each is the caller's to act on; none holds a word of a prompt.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum PromptRefusal {
    /// The save breaks a rule of the page: the code says which, `at` where.
    #[error("{0}")]
    Invalid(PromptError),
    /// The person already has a prompt of that name.
    #[error("you already have a prompt of that name")]
    NameTaken,
    /// The person holds the most prompts there are.
    #[error("you hold the most prompts there are ({limit})")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// The prompt changed since the revision the request was based on.
    #[error("the prompt changed since the revision sent")]
    StaleRevision,
    /// No such prompt for this person: it never existed, is gone, is another person's private one or
    /// is shared with roles they do not have (the same answer for all), or the write is the
    /// owner's and the caller is not.
    #[error("no such prompt")]
    NotFound,
    /// The answers do not fit the variables.
    #[error("{0}")]
    InvalidAnswer(AnswerRefusal),
    /// The rendered text would be over the limit.
    #[error(
        "the text would be over {} bytes",
        orch_core::prompt::MAX_RENDERED_BYTES
    )]
    RenderedTooLong,
}

impl PromptRefusal {
    /// The `code` of the problem, where a client acts on one; `None` for a plain 404.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            PromptRefusal::Invalid(e) => Some(e.code),
            PromptRefusal::NameTaken => Some("name_taken"),
            PromptRefusal::TooMany { .. } => Some("too_many_prompts"),
            PromptRefusal::StaleRevision => Some("stale_revision"),
            PromptRefusal::NotFound => None,
            PromptRefusal::InvalidAnswer(_) => Some("invalid_answer"),
            PromptRefusal::RenderedTooLong => Some("rendered_too_long"),
        }
    }
}

impl From<PromptRefusal> for AppError {
    fn from(refusal: PromptRefusal) -> Self {
        AppError::Prompt(refusal)
    }
}

/// The members an edit changes: each present one **replaces** the whole member.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditPrompt {
    /// The revision the caller last read.
    pub revision: Option<PromptRevision>,
    /// A new name.
    pub name: Option<String>,
    /// A new title.
    pub title: Option<String>,
    /// A new description.
    pub description: Option<String>,
    /// A new body.
    pub body: Option<String>,
    /// The whole list of variables.
    pub variables: Option<Vec<Value>>,
    /// The whole audience.
    pub audience: Option<Vec<String>>,
}

/// A render request, as the API read its shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderPrompt {
    /// The revision the form was drawn from: the prompt must still be at it.
    pub revision: Option<PromptRevision>,
    /// The `answers` member, when it was there (any JSON: a list that fits, or not).
    pub answers: Option<Value>,
    /// Whether the request had a `note` (a prompt has no free box).
    pub note: bool,
}

/// The text of a render, and the revision of the prompt it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPrompt {
    /// The text, to put in the message box: nothing was sent.
    pub text: String,
    /// The revision used.
    pub revision: PromptRevision,
}

fn store_error(error: PromptStoreError) -> AppError {
    match error {
        PromptStoreError::TooMany { limit } => PromptRefusal::TooMany { limit }.into(),
        PromptStoreError::NameTaken => PromptRefusal::NameTaken.into(),
        PromptStoreError::NotFound => PromptRefusal::NotFound.into(),
        PromptStoreError::Conflict => PromptRefusal::StaleRevision.into(),
        PromptStoreError::Unavailable { .. } => {
            AppError::Store(orch_ports::StoreError::unavailable(error))
        }
        PromptStoreError::Corrupt(_) => AppError::Store(orch_ports::StoreError::corrupt_with(
            "a person's prompt does not read back",
            error,
        )),
        PromptStoreError::Internal { .. } => {
            AppError::Store(orch_ports::StoreError::internal(error))
        }
        PromptStoreError::Exists => {
            AppError::internal("a prompt id the store reported as taken was not retried")
        }
        // a variant added later is a bug until it is mapped
        other => AppError::Store(orch_ports::StoreError::internal(other)),
    }
}

fn viewer(who: &impl Requester) -> PromptViewer {
    PromptViewer {
        user: who.user().clone(),
        // as given, byte for byte; a role that cannot be in any audience (a NUL, a space, over 64
        // bytes) can match nothing, so it never reaches a store
        roles: who
            .roles()
            .into_iter()
            .map(|r| r.as_str().to_owned())
            .filter(|r| PromptAudience::is_role_value(r))
            .collect::<BTreeSet<_>>(),
    }
}

impl<P: orch_ports::Ports> App<P> {
    /// Whether the person holds `prompts.share`.
    fn may_share_prompts(&self, who: &impl Requester) -> bool {
        self.access(who)
            .check(Permission::PromptsShare, &Resource::Anything)
            .is_ok()
    }

    /// The `PromptsShare` check for a result that is shared.
    fn require_share(&self, who: &impl Requester, audience: &[String]) -> Result<(), AppError> {
        if audience.is_empty() || self.may_share_prompts(who) {
            Ok(())
        } else {
            Err(AppError::missing_permission(Permission::PromptsShare))
        }
    }

    /// Saves a prompt of the person's own.
    ///
    /// # Errors
    /// [`AppError::Forbidden`] for a shared prompt without `prompts.share` (before the values are
    /// read); [`PromptRefusal::Invalid`] for a rule of the page; [`PromptRefusal::NameTaken`];
    /// [`PromptRefusal::TooMany`]; [`AppError::Store`].
    pub async fn create_prompt(
        &self,
        who: &impl Requester,
        input: PromptInput,
    ) -> Result<StoredPrompt, AppError> {
        self.require_share(who, &input.audience)?;
        let store = self.ports().store();
        for _ in 0..ID_ATTEMPTS {
            let id = PromptId::from_entropy(self.ports().ids().new_token_bytes());
            let prompt =
                PromptDefinition::new(id, input.clone()).map_err(PromptRefusal::Invalid)?;
            let now = self.ports().clock().now();
            match store.insert_prompt(who.user(), &prompt, now).await {
                Ok(stored) => return Ok(stored),
                Err(PromptStoreError::Exists) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        Err(AppError::internal(
            "a new prompt id was taken four times in a row",
        ))
    }

    /// A page of what the person sees: their own prompts and the ones shared with their roles.
    ///
    /// # Errors
    /// [`AppError::Store`].
    pub async fn list_prompts(
        &self,
        who: &impl Requester,
        scope: PromptScope,
        after: Option<&PromptId>,
    ) -> Result<PromptPage, AppError> {
        let limit = u32::try_from(orch_core::prompt::PAGE).unwrap_or(100);
        self.ports()
            .store()
            .list_prompts(&viewer(who), scope, after, limit)
            .await
            .map_err(store_error)
    }

    /// One prompt, the person's own or shared with them.
    ///
    /// # Errors
    /// [`PromptRefusal::NotFound`] for anything else; [`AppError::Store`].
    pub async fn get_prompt(
        &self,
        who: &impl Requester,
        id: &PromptId,
    ) -> Result<StoredPrompt, AppError> {
        self.ports()
            .store()
            .get_prompt(&viewer(who), id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| PromptRefusal::NotFound.into())
    }

    /// Changes the person's own prompt: each member given replaces the whole member. A change that
    /// changes nothing writes nothing and keeps the revision (which is still checked).
    ///
    /// # Errors
    /// [`PromptRefusal::NotFound`] (not the person's); [`AppError::Forbidden`] when the result is
    /// shared and the person lacks `prompts.share`; [`PromptRefusal::StaleRevision`];
    /// [`PromptRefusal::Invalid`]; [`PromptRefusal::NameTaken`]; [`AppError::Store`].
    pub async fn edit_prompt(
        &self,
        who: &impl Requester,
        id: &PromptId,
        edit: EditPrompt,
    ) -> Result<StoredPrompt, AppError> {
        let store = self.ports().store();
        let current = store
            .get_prompt(&viewer(who), id)
            .await
            .map_err(store_error)?
            .filter(StoredPrompt::own)
            .ok_or(PromptRefusal::NotFound)?;
        let mut input = current.prompt().to_input();
        let resulting = edit
            .audience
            .clone()
            .unwrap_or_else(|| input.audience.clone());
        let resulting: Vec<String> = resulting
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.require_share(who, &resulting)?;
        if edit.revision != Some(current.revision()) {
            return Err(PromptRefusal::StaleRevision.into());
        }
        if let Some(name) = edit.name {
            input.name = name;
        }
        if let Some(title) = edit.title {
            input.title = title;
        }
        if let Some(description) = edit.description {
            input.description = description;
        }
        if let Some(body) = edit.body {
            input.body = body;
        }
        if let Some(variables) = edit.variables {
            input.variables = variables;
        }
        if let Some(audience) = edit.audience {
            input.audience = audience;
        }
        let next = PromptDefinition::new(id.clone(), input).map_err(PromptRefusal::Invalid)?;
        if &next == current.prompt() {
            return Ok(current);
        }
        let now = self.ports().clock().now();
        store
            .replace_prompt(who.user(), &next, current.revision(), now)
            .await
            .map_err(store_error)
    }

    /// Deletes the person's own prompt; with `expected`, only if it is still at that revision. It
    /// needs no `prompts.share`: a person can always withdraw what they shared.
    ///
    /// # Errors
    /// [`PromptRefusal::NotFound`] (not the person's, or gone); [`PromptRefusal::StaleRevision`];
    /// [`AppError::Store`].
    pub async fn delete_prompt(
        &self,
        who: &impl Requester,
        id: &PromptId,
        expected: Option<PromptRevision>,
    ) -> Result<(), AppError> {
        self.ports()
            .store()
            .delete_prompt(who.user(), id, expected)
            .await
            .map_err(store_error)
    }

    /// Fills a prompt's variables with the person's answers and returns the text. It writes nothing,
    /// and is allowed for the person's own prompts and for the ones shared with them.
    ///
    /// # Errors
    /// [`PromptRefusal::NotFound`]; [`PromptRefusal::StaleRevision`] when the request names a
    /// revision the prompt is no longer at; [`PromptRefusal::InvalidAnswer`] with the first rule the
    /// answers break; [`PromptRefusal::RenderedTooLong`]; [`AppError::Store`].
    pub async fn render_prompt(
        &self,
        who: &impl Requester,
        id: &PromptId,
        request: RenderPrompt,
    ) -> Result<RenderedPrompt, AppError> {
        let stored = self.get_prompt(who, id).await?;
        if request.revision.is_some_and(|r| r != stored.revision()) {
            return Err(PromptRefusal::StaleRevision.into());
        }
        let prompt = stored.prompt();
        let answers = validate_answers(prompt.variables(), request.answers.as_ref(), request.note)
            .map_err(PromptRefusal::InvalidAnswer)?;
        let text = render(prompt.body(), prompt.variables(), &answers).map_err(|e| match e {
            RenderError::TooLong => AppError::from(PromptRefusal::RenderedTooLong),
            RenderError::BadBody => {
                AppError::internal("a stored prompt has a body the rules do not read")
            }
        })?;
        Ok(RenderedPrompt {
            text,
            revision: stored.revision(),
        })
    }
}
