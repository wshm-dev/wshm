use anyhow::Result;
use serde::de::DeserializeOwned;

use super::client::AiClient;
use super::local::LocalClient;
use crate::config::Config;

/// Unified AI backend: remote (API) or local (embedded llama.cpp).
pub enum AiBackend {
    Remote(AiClient),
    Local(LocalClient),
    /// Canned JSON response (tests only).
    #[cfg(test)]
    Mock(String),
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

    /// Call the AI and deserialize the JSON response into T. Every system
    /// prompt (built-in or from the config) gets the untrusted-data clause,
    /// see `ai::untrusted`.
    pub async fn complete<T: DeserializeOwned>(&self, system: &str, user: &str) -> Result<T> {
        let system = super::untrusted::harden_system(system);
        match self {
            AiBackend::Remote(ai) => ai.complete(&system, user).await,
            AiBackend::Local(local) => local.complete(&system, user),
            #[cfg(test)]
            AiBackend::Mock(raw) => {
                let _ = (system, user);
                Ok(serde_json::from_str(raw)?)
            }
        }
    }
}
