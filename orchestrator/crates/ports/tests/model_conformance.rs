//! Runs the `ChatModel` conformance testkit against the scripted in-memory model.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_ports::memory::{ModelStep, ScriptedModel};
use orch_ports::testkit::chat_model::ModelFixture;
use orch_ports::{ChatRequest, NoModel};

struct Scripted(ScriptedModel);

impl ModelFixture for Scripted {
    type Model = ScriptedModel;

    fn model(&self) -> &ScriptedModel {
        &self.0
    }

    fn secret(&self) -> &str {
        // the scripted model has no credential to leak; the case still runs
        "sk-scripted-secret"
    }

    fn endpoint(&self) -> &str {
        "default"
    }

    fn will_answer(&self, text: &str) {
        self.0.then_answer(text);
    }

    fn will_be_unreachable(&self) {
        self.0.then(ModelStep::Unreachable);
    }

    fn will_rate_limit(&self) {
        self.0.then(ModelStep::RateLimited);
    }

    fn will_reject(&self) {
        self.0.then(ModelStep::Reject);
    }

    fn will_refuse_the_credential(&self) {
        self.0.then(ModelStep::Unauthenticated);
    }

    fn will_answer_nonsense(&self) {
        self.0.then(ModelStep::Nonsense);
    }

    fn last_request(&self) -> Option<ChatRequest> {
        self.0.calls().last().cloned()
    }
}

async fn make() -> Option<Scripted> {
    Some(Scripted(
        ScriptedModel::default().with_endpoints(["default"]),
    ))
}

orch_ports::chat_model_conformance!(make);

#[tokio::test]
async fn a_scripted_model_says_none_when_it_has_no_script_and_remembers_its_questions() {
    use orch_ports::ChatModel;
    let model = ScriptedModel::default();
    model.then_answer("A title");
    let ask = |user: &str| ChatRequest {
        endpoint: "default".into(),
        model: "m".into(),
        system: "s".into(),
        user: user.into(),
        max_tokens: 8,
    };
    assert_eq!(model.complete(&ask("one")).await.unwrap(), "A title");
    assert_eq!(model.complete(&ask("two")).await.unwrap(), "NONE");
    let calls = model.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].user, "two");
    // a clone shares the script and the record
    let clone = model.clone();
    clone.then_answer("Another");
    assert_eq!(model.complete(&ask("three")).await.unwrap(), "Another");
    assert_eq!(clone.calls().len(), 3);
}

#[tokio::test]
async fn no_model_has_nothing_to_answer() {
    use orch_ports::ChatModel;
    let err = NoModel
        .complete(&ChatRequest {
            endpoint: "default".into(),
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            max_tokens: 8,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, orch_ports::ModelError::NotConfigured));
}
