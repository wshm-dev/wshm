use anyhow::Result;
use serde::de::DeserializeOwned;

use super::client::AiClient;
use super::local::LocalClient;
use crate::config::Config;

/// Unified AI backend: remote (API) or local (embedded llama.cpp).
pub enum AiBackend {
    Remote(AiClient),
    Local(LocalClient),
}

impl AiBackend {
    /// Build an AiBackend from config, with an optional model override.
    pub fn from_config(config: &Config, model: &str) -> Result<Self> {
        if config.ai.provider == "local" {
            Ok(Self::Local(LocalClient::new(model)?))
        } else {
            Ok(Self::Remote(AiClient::with_model(config, model)?))
        }
    }

    /// Call the AI and deserialize the JSON response into T.
    pub async fn complete<T: DeserializeOwned>(&self, system: &str, user: &str) -> Result<T> {
        match self {
            AiBackend::Remote(ai) => ai.complete(system, user).await,
            AiBackend::Local(local) => local.complete(system, user),
        }
    }
}
