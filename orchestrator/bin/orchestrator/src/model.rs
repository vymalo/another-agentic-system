//! The model this process asks (ADR 0005, ADR 0009): the OpenAI-compatible endpoint
//! `ORCH_MODEL_BASE_URL` names when `ORCH_TITLE_MODEL` is set, or none. A composition detail of
//! the binary: the application sees only the `ChatModel` port.

use orch_model_openai::{OpenAiChat, OpenAiConfig};
use orch_ports::{ChatModel, ChatRequest, ModelError, NoModel};

use crate::config::ModelSettings;

/// The one model type of the binary's `PortSet`, so that "titles on" and "titles off" are the same
/// build (static dispatch over two variants, not a runtime plugin).
#[derive(Debug, Clone)]
pub enum ConfiguredModel {
    /// No model: every question is `ModelError::NotConfigured`, and the application asks none.
    Off(NoModel),
    /// The configured endpoint.
    OpenAi(OpenAiChat),
}

impl ConfiguredModel {
    /// The model `settings` name, or none.
    pub fn build(settings: Option<&ModelSettings>) -> anyhow::Result<Self> {
        let Some(settings) = settings else {
            return Ok(ConfiguredModel::Off(NoModel));
        };
        let mut cfg = OpenAiConfig::new(settings.base_url.clone()).with_timeout(settings.timeout);
        if let Some(key) = &settings.api_key {
            cfg = cfg.with_api_key(key.clone());
        }
        let chat = OpenAiChat::new(cfg)?;
        tracing::info!(
            model = %settings.title_model,
            endpoint = %settings.base_url,
            "thread titles are written by a model"
        );
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
