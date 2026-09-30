//! The `echo` local agent: repeats the user's message back. For tests and demos; it needs no
//! model, no tool and no network.

use adam_runtime::{Agent, AgentError, AgentStarter, Ctx, Inbound, Transition};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The name of the agent, as stored in `orch_agent_runs.agent`.
pub(crate) const NAME: &str = "echo";

/// One line for humans: the agent card's description and the configuration's help text.
pub(crate) const DESCRIPTION: &str = "repeats the user's message back (tests and demos)";

/// What an echo run remembers between its start and its only step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EchoState {
    text: String,
}

/// The state of a new run, or the reason the start message is unusable. Shared by the agent and
/// its start-only half, which must agree (the control plane starts, a worker steps).
fn initial(input: &Inbound) -> Result<EchoState, AgentError> {
    let text = input
        .payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AgentError::permanent("unusable start message: it has no text"))?;
    Ok(EchoState {
        text: text.to_owned(),
    })
}

/// The agent: one step, which finishes the run with the text as its output.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Echo;

#[async_trait]
impl Agent for Echo {
    type State = EchoState;

    fn name(&self) -> &str {
        NAME
    }

    fn init(&self, input: Inbound) -> Result<EchoState, AgentError> {
        initial(&input)
    }

    async fn step(
        &self,
        _ctx: &mut Ctx,
        state: EchoState,
    ) -> Result<Transition<EchoState>, AgentError> {
        let output = json!({ "text": state.text });
        Ok(Transition::Done { state, output })
    }
}

/// The start-only half of [`Echo`], for a process that accepts tasks but never steps them (the
/// control plane).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EchoStarter;

impl AgentStarter for EchoStarter {
    type State = EchoState;

    fn name(&self) -> &str {
        NAME
    }

    fn init(&self, input: Inbound) -> Result<EchoState, AgentError> {
        initial(&input)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use adam_runtime::Classify as _;
    use serde_json::json;

    use super::*;

    fn message(payload: serde_json::Value) -> Inbound {
        Inbound::new("message", payload)
    }

    #[test]
    fn the_agent_and_its_starter_start_the_same_state() {
        let input = message(json!({ "text": "  hello  " }));
        let a = Agent::init(&Echo, input.clone()).unwrap();
        let s = AgentStarter::init(&EchoStarter, input).unwrap();
        assert_eq!(a, s);
        assert_eq!(a.text, "hello");
        assert_eq!(Agent::name(&Echo), AgentStarter::name(&EchoStarter));
    }

    #[test]
    fn a_message_without_text_is_refused_for_good() {
        for payload in [json!({}), json!({ "text": "   " }), json!({ "text": 3 })] {
            let err = Agent::init(&Echo, message(payload)).unwrap_err();
            assert!(!err.is_retryable(), "{err}");
        }
    }
}
