use anyhow::{Context, Result};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::config::WebhookConfig;

use super::{ExportEvent, ExportSink};

/// Webhook sink — POST JSON to a URL with optional HMAC signing.
pub struct WebhookSink {
    url: String,
    events: Vec<String>,
    secret: Option<String>,
    client: reqwest::Client,
}

impl WebhookSink {
    pub fn new(config: &WebhookConfig) -> Self {
        Self {
            url: config.url.clone(),
            events: config.events.clone(),
            secret: config.secret.clone(),
            client: reqwest::Client::new(),
        }
    }

    fn should_emit(&self, event: &ExportEvent) -> bool {
        self.events.iter().any(|f| event.kind.matches_filter(f))
    }

    fn sign_payload(&self, payload: &[u8]) -> Option<String> {
        let secret = self.secret.as_ref()?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
        mac.update(payload);
        Some(format!(
            "sha256={}",
            hex::encode(mac.finalize().into_bytes())
        ))
    }
}

#[async_trait]
impl ExportSink for WebhookSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        if !self.should_emit(event) {
            return Ok(());
        }

        let payload = serde_json::to_vec(event)?;

        let mut req = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "wshm-webhook/1.0");

        if let Some(sig) = self.sign_payload(&payload) {
            req = req.header("X-Wshm-Signature", sig);
        }

        let resp = req
            .body(payload)
            .send()
            .await
            .with_context(|| format!("Webhook POST to {} failed", self.url))?;

        if !resp.status().is_success() {
            anyhow::bail!("Webhook {} returned HTTP {}", self.url, resp.status());
        }

        tracing::debug!("Webhook delivered to {}", self.url);
        Ok(())
    }

    fn name(&self) -> &str {
        "webhook"
    }
}
