//! The models this process asks (ADR 0005, ADR 0009, ADR 0035): the OpenAI-compatible endpoints
//! the configuration names (`models.endpoints`), or none. A composition detail of the binary: the
//! application sees only the `ChatModel` port, and names the endpoint of each request.

use anyhow::Context as _;
use orch_model_openai::{OpenAiChat, OpenAiConfig};
use orch_ports::{ChatModel, ChatRequest, ModelError, NoModel};

use crate::config::ModelsSettings;

/// The one model type of the binary's `PortSet`, so that "tasks on" and "tasks off" are the same
/// build (static dispatch over two variants, not a runtime plugin).
#[derive(Debug, Clone)]
pub enum ConfiguredModel {
    /// No endpoint: every question is `ModelError::NotConfigured`, and the application asks none.
    Off(NoModel),
    /// The configured endpoints.
    OpenAi(OpenAiChat),
}

impl ConfiguredModel {
    /// The model `settings` name: one client per endpoint, or none when no task is on.
    pub fn build(settings: &ModelsSettings) -> anyhow::Result<Self> {
        if settings.tasks.is_empty() {
            return Ok(ConfiguredModel::Off(NoModel));
        }
        let endpoints = settings.endpoints.iter().map(|(name, endpoint)| {
            let mut cfg =
                OpenAiConfig::new(endpoint.base_url.clone()).with_timeout(endpoint.timeout);
            if let Some(key) = &endpoint.api_key {
                cfg = cfg.with_api_key(key.clone());
            }
            (name.clone(), cfg)
        });
        let chat = OpenAiChat::new(endpoints).context("cannot build the model endpoints")?;
        for (kind, task) in &settings.tasks {
            let endpoint = settings.endpoints.get(&task.endpoint);
            tracing::info!(
                task = kind.as_str(),
                endpoint = %task.endpoint,
                url = endpoint.map_or("", |e| e.shown_url()),
                model = %task.model,
                max_tokens = task.max_tokens,
                guidance = if task.guidance.is_some() { "configured" } else { "the core's" },
                "a model is asked for the {}",
                kind.as_str()
            );
        }
        Ok(ConfiguredModel::OpenAi(chat))
    }
}

impl ChatModel for ConfiguredModel {
    async fn complete(&self, request: &ChatRequest) -> Result<String, ModelError> {
        match self {
            ConfiguredModel::Off(off) => off.complete(request).await,
            ConfiguredModel::OpenAi(chat) => chat.complete(request).await,
        }
    }
}
