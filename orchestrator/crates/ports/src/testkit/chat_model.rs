//! `ChatModel` conformance cases: what the dispatcher relies on from any model adapter, stated once
//! and run against the OpenAI-compatible adapter (over real HTTP, against an in-process stub of
//! the endpoint) and against the scripted in-memory model.
//!
//! An implementation supplies a [`ModelFixture`]: a model, and the means to make what stands
//! behind it answer, fail transiently, ask to slow down, refuse, or say nothing in time. Each case
//! is `async fn(fixture)` and gives up after 10 seconds.
//!
//! What is deliberately not asserted: the wording of any error, the wire format, and how many
//! times the adapter tries before it gives up (it may try more than once, never forever).

use std::future::Future;
use std::time::Duration;

use orch_core::{Classify, ErrorClass};

use crate::{ChatModel, ChatRequest, ModelError};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(10);

/// A model adapter under test, with an endpoint behind it that does what a case tells it to.
pub trait ModelFixture: Send + Sync + 'static {
    /// The adapter under test.
    type Model: ChatModel;

    /// The adapter, built with [`ModelFixture::secret`] as its credential.
    fn model(&self) -> &Self::Model;

    /// The credential the adapter was built with. No error and no debug text may show it.
    fn secret(&self) -> &str;

    /// The next question is answered with `text`.
    fn will_answer(&self, text: &str);

    /// The next questions find the endpoint failing (a 5xx, a dropped connection).
    fn will_be_unreachable(&self);

    /// The next questions are told to slow down.
    fn will_rate_limit(&self);

    /// The next questions are refused for good (an unknown model, a request it cannot read).
    fn will_reject(&self);

    /// The next questions are refused for the credential.
    fn will_refuse_the_credential(&self);

    /// The next questions are answered with something that is not a chat completion.
    fn will_answer_nonsense(&self);

    /// What the endpoint was last asked, when it was asked anything.
    fn last_request(&self) -> Option<ChatRequest>;
}

/// The question every case asks.
fn question() -> ChatRequest {
    ChatRequest {
        model: "mock-title".to_owned(),
        system: "Reply with a short title.".to_owned(),
        user: "Title this: how do I fix the build?".to_owned(),
        max_tokens: 24,
    }
}

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, case)
        .await
        .expect("the case timed out")
}

async fn failure<F: ModelFixture>(f: &F) -> ModelError {
    within(f.model().complete(&question()))
        .await
        .expect_err("the question should fail")
}

/// The answer is the model's text, and the endpoint was asked the question as it was put.
pub async fn the_answer_is_the_models_text<F: ModelFixture>(f: F) {
    f.will_answer("Fix the build");
    let text = within(f.model().complete(&question()))
        .await
        .expect("an answer");
    assert_eq!(text.trim(), "Fix the build");
    let asked = f.last_request().expect("the endpoint was asked");
    assert_eq!(asked.model, "mock-title");
    assert_eq!(asked.system, "Reply with a short title.");
    assert!(asked.user.contains("how do I fix the build?"), "{asked:?}");
}

/// A model that is down is a transient failure: asking again can help.
pub async fn an_endpoint_that_fails_is_transient<F: ModelFixture>(f: F) {
    f.will_be_unreachable();
    let err = failure(&f).await;
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
    assert!(err.is_retryable());
}

/// An endpoint that says nonsense is not an answer, and asking again can help.
pub async fn nonsense_is_not_an_answer<F: ModelFixture>(f: F) {
    f.will_answer_nonsense();
    let err = failure(&f).await;
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
}

/// A model that asks to slow down is rate limited, and the wait it names is passed on.
pub async fn a_rate_limit_is_rate_limited<F: ModelFixture>(f: F) {
    f.will_rate_limit();
    let err = failure(&f).await;
    assert_eq!(err.class(), ErrorClass::RateLimited, "{err}");
    assert!(err.is_retryable());
}

/// A request the model refuses for good is invalid: asking the same again never helps.
pub async fn a_refusal_is_permanent<F: ModelFixture>(f: F) {
    f.will_reject();
    let err = failure(&f).await;
    assert_eq!(err.class(), ErrorClass::Invalid, "{err}");
    assert!(!err.is_retryable());
}

/// A credential the endpoint refuses is unauthenticated, permanently.
pub async fn a_refused_credential_is_unauthenticated<F: ModelFixture>(f: F) {
    f.will_refuse_the_credential();
    let err = failure(&f).await;
    assert_eq!(err.class(), ErrorClass::Unauthenticated, "{err}");
    assert!(!err.is_retryable());
}

/// Whatever went wrong, the credential is in no error text and no debug text.
pub async fn the_credential_is_never_in_an_error<F: ModelFixture>(f: F) {
    let secret = f.secret().to_owned();
    assert!(
        !secret.is_empty(),
        "the fixture must have a credential to hide"
    );
    for arm in 0..5 {
        match arm {
            0 => f.will_be_unreachable(),
            1 => f.will_rate_limit(),
            2 => f.will_reject(),
            3 => f.will_refuse_the_credential(),
            _ => f.will_answer_nonsense(),
        }
        let err = failure(&f).await;
        let shown = format!("{err} | {err:?} | {}", orch_core::report(&err));
        assert!(!shown.contains(&secret), "arm {arm}: {shown}");
    }
}
