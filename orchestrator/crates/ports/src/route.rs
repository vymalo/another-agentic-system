//! [`ByTransport`]: one [`AgentClient`] made of two, chosen by the endpoint's transport.

use crate::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentError, AgentStream, AgentTransport,
    SendRequest, TaskHandle, TaskSnapshot,
};

/// Which of the two clients an endpoint belongs to. The only place that reads
/// [`AgentTransport`], and an exhaustive match: a new transport must be given a client here
/// before the workspace compiles (ADR 0004).
enum Route {
    A2a,
    Local,
}

fn route(endpoint: &AgentEndpoint) -> Route {
    match &endpoint.transport {
        AgentTransport::A2a { .. } => Route::A2a,
        AgentTransport::Local { .. } => Route::Local,
    }
}

/// Sends each call to the client that serves the endpoint's transport: `a2a` for
/// [`AgentTransport::A2a`], `local` for [`AgentTransport::Local`] (ADR 0015).
///
/// The composition root builds it when a binary contains both kinds of agent. Neither client
/// ever sees an endpoint of the other transport, so each keeps refusing what is not its own
/// as defence in depth. The type holds no adapter type: `A` and `L` are any two
/// [`AgentClient`]s.
#[derive(Debug, Clone)]
pub struct ByTransport<A, L> {
    /// Serves endpoints that are remote A2A agents.
    pub a2a: A,
    /// Serves endpoints that are hosted in this process.
    pub local: L,
}

impl<A: AgentClient, L: AgentClient> AgentClient for ByTransport<A, L> {
    async fn read_card(&self, ep: &AgentEndpoint) -> Result<AgentCardInfo, AgentError> {
        match route(ep) {
            Route::A2a => self.a2a.read_card(ep).await,
            Route::Local => self.local.read_card(ep).await,
        }
    }

    async fn send_stream(&self, req: SendRequest) -> Result<AgentStream, AgentError> {
        match route(&req.endpoint) {
            Route::A2a => self.a2a.send_stream(req).await,
            Route::Local => self.local.send_stream(req).await,
        }
    }

    async fn resubscribe(&self, task: &TaskHandle) -> Result<AgentStream, AgentError> {
        match route(&task.endpoint) {
            Route::A2a => self.a2a.resubscribe(task).await,
            Route::Local => self.local.resubscribe(task).await,
        }
    }

    async fn get_task(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        match route(&task.endpoint) {
            Route::A2a => self.a2a.get_task(task).await,
            Route::Local => self.local.get_task(task).await,
        }
    }

    async fn cancel(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        match route(&task.endpoint) {
            Route::A2a => self.a2a.cancel(task).await,
            Route::Local => self.local.cancel(task).await,
        }
    }

    async fn find_task_by_message(
        &self,
        ep: &AgentEndpoint,
        context_id: &str,
        message_id: &str,
    ) -> Result<Option<String>, AgentError> {
        match route(ep) {
            Route::A2a => {
                self.a2a
                    .find_task_by_message(ep, context_id, message_id)
                    .await
            }
            Route::Local => {
                self.local
                    .find_task_by_message(ep, context_id, message_id)
                    .await
            }
        }
    }
}

#[cfg(all(test, feature = "testkit"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::AgentId;

    use super::*;
    use crate::memory::{Call, ScriptedAgent};
    use crate::{IdemKey, SendContent};

    fn request(ep: &AgentEndpoint, message: &str) -> SendRequest {
        SendRequest {
            endpoint: ep.clone(),
            message_id: message.to_owned(),
            context_id: "ctx".to_owned(),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::Text("echo hi".to_owned()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
        }
    }

    fn handle(ep: &AgentEndpoint, task: &str) -> TaskHandle {
        TaskHandle {
            endpoint: ep.clone(),
            task_id: task.to_owned(),
        }
    }

    fn agents_of(calls: &[Call]) -> Vec<String> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::Send { agent, .. } | Call::ReadCard { agent } => {
                    Some(agent.as_str().to_owned())
                }
                Call::Resubscribe { .. }
                | Call::GetTask { .. }
                | Call::Cancel { .. }
                | Call::Find { .. } => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn routes_by_transport_and_never_crosses() {
        let both = ByTransport {
            a2a: ScriptedAgent::new(),
            local: ScriptedAgent::new(),
        };
        let remote = AgentEndpoint::a2a(AgentId::new("remote"), "https://r.example/card", None);
        let inside = AgentEndpoint::local(AgentId::new("inside"), "echo");

        // Every operation, once per endpoint.
        for (ep, message) in [(&remote, "m-remote"), (&inside, "m-inside")] {
            both.read_card(ep).await.unwrap();
            let mut stream = both.send_stream(request(ep, message)).await.unwrap();
            let first = futures::StreamExt::next(&mut stream)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(first.key, IdemKey::Turn(_) | IdemKey::Task(_)));
            both.find_task_by_message(ep, "ctx", message).await.unwrap();
            let task = handle(ep, &first.task_id);
            both.get_task(&task).await.unwrap();
            both.cancel(&task).await.ok();
            both.resubscribe(&task).await.ok();
        }

        let remote_calls = both.a2a.calls();
        let local_calls = both.local.calls();
        assert_eq!(
            agents_of(&remote_calls),
            ["remote", "remote"],
            "{remote_calls:?}"
        );
        assert_eq!(
            agents_of(&local_calls),
            ["inside", "inside"],
            "{local_calls:?}"
        );
        // Each side saw the same number of calls (one endpoint each, the same operations), and
        // the task-scoped ones follow the endpoint of the handle too.
        assert_eq!(remote_calls.len(), local_calls.len());
        assert!(
            remote_calls
                .iter()
                .all(|c| !format!("{c:?}").contains("m-inside")),
            "{remote_calls:?}"
        );
        assert!(
            local_calls
                .iter()
                .all(|c| !format!("{c:?}").contains("m-remote")),
            "{local_calls:?}"
        );
    }
}
