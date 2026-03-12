use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use tracing::debug;

use crate::config::Config;

struct ProviderConfig {
    api_url: String,
    api_key: String,
    model: String,
    kind: ProviderKind,
}

enum ProviderKind {
    Anthropic,
    OpenAiCompat,
    Google,
}

pub struct AiClient {
    http: reqwest::Client,
    provider: ProviderConfig,
}

/// Resolve provider configuration from name + optional base_url override.
fn resolve_provider(config: &Config) -> Result<ProviderConfig> {
    let provider = config.ai.provider.as_str();
    let model = config.ai.model.clone();
    let base_url = config.ai.base_url.clone();

    match provider {
        // ── Anthropic (custom API format) ──────────────────────────
        "anthropic" => Ok(ProviderConfig {
            api_url: base_url.unwrap_or_else(|| "https://api.anthropic.com/v1/messages".into()),
            api_key: env_key(&["ANTHROPIC_API_KEY"])?,
            model,
            kind: ProviderKind::Anthropic,
        }),

        // ── Google Gemini (custom API format) ──────────────────────
        "google" | "gemini" => Ok(ProviderConfig {
            api_url: base_url.unwrap_or_else(|| {
                format!(
                    "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
                )
            }),
            api_key: env_key(&["GOOGLE_API_KEY", "GEMINI_API_KEY"])?,
            model,
            kind: ProviderKind::Google,
        }),

        // ── OpenAI ─────────────────────────────────────────────────
        "openai" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".into()),
            api_key: env_key(&["OPENAI_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Mistral ────────────────────────────────────────────────
        "mistral" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.mistral.ai/v1/chat/completions".into()),
            api_key: env_key(&["MISTRAL_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Groq ──────────────────────────────────────────────────
        "groq" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.groq.com/openai/v1/chat/completions".into()),
            api_key: env_key(&["GROQ_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── DeepSeek ──────────────────────────────────────────────
        "deepseek" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.deepseek.com/chat/completions".into()),
            api_key: env_key(&["DEEPSEEK_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── xAI (Grok) ───────────────────────────────────────────
        "xai" | "grok" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.x.ai/v1/chat/completions".into()),
            api_key: env_key(&["XAI_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Together AI ───────────────────────────────────────────
        "together" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.together.xyz/v1/chat/completions".into()),
            api_key: env_key(&["TOGETHER_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Fireworks AI ──────────────────────────────────────────
        "fireworks" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.fireworks.ai/inference/v1/chat/completions".into()),
            api_key: env_key(&["FIREWORKS_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Perplexity ───────────────────────────────────────────
        "perplexity" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.perplexity.ai/chat/completions".into()),
            api_key: env_key(&["PERPLEXITY_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Cohere ────────────────────────────────────────────────
        "cohere" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.cohere.com/v2/chat".into()),
            api_key: env_key(&["COHERE_API_KEY", "CO_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── OpenRouter (aggregator) ───────────────────────────────
        "openrouter" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://openrouter.ai/api/v1/chat/completions".into()),
            api_key: env_key(&["OPENROUTER_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Ollama (local, no API key) ────────────────────────────
        "ollama" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "http://localhost:11434/v1/chat/completions".into()),
            api_key: std::env::var("OLLAMA_API_KEY").unwrap_or_default(),
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        // ── Azure OpenAI ─────────────────────────────────────────
        "azure" | "azure-openai" => {
            let endpoint = base_url
                .or_else(|| std::env::var("AZURE_OPENAI_ENDPOINT").ok())
                .context("Set base_url in config or AZURE_OPENAI_ENDPOINT env var")?;
            let api_version = std::env::var("AZURE_OPENAI_API_VERSION")
                .unwrap_or_else(|_| "2024-06-01".into());
            Ok(ProviderConfig {
                api_url: format!(
                    "{endpoint}/openai/deployments/{model}/chat/completions?api-version={api_version}"
                ),
                api_key: env_key(&["AZURE_OPENAI_API_KEY"])?,
                model,
                kind: ProviderKind::OpenAiCompat,
            })
        }

        // ── Custom OpenAI-compatible endpoint ─────────────────────
        "custom" => Ok(ProviderConfig {
            api_url: base_url.context("Set base_url in [ai] config for custom provider")?,
            api_key: env_key(&["WSHM_AI_API_KEY", "AI_API_KEY"]).unwrap_or_default(),
            model,
            kind: ProviderKind::OpenAiCompat,
        }),

        "local" => anyhow::bail!(
            "Provider 'local' uses embedded inference — use LocalClient, not AiClient"
        ),

        other => anyhow::bail!(
            "Unknown AI provider: '{other}'. Supported: anthropic, openai, google, \
             mistral, groq, deepseek, xai, together, fireworks, perplexity, cohere, \
             openrouter, ollama, azure, local, custom"
        ),
    }
}

/// Try multiple env var names, return the first one found.
fn env_key(names: &[&str]) -> Result<String> {
    for name in names {
        if let Ok(val) = std::env::var(name) {
            if !val.is_empty() {
                return Ok(val);
            }
        }
    }
    let vars = names.join(" or ");
    anyhow::bail!("Set {vars} environment variable")
}

impl AiClient {
    pub fn new(config: &Config) -> Result<Self> {
        let provider = resolve_provider(config)?;
        Ok(Self {
            http: reqwest::Client::new(),
            provider,
        })
    }

    pub async fn complete<T: DeserializeOwned>(&self, system: &str, user: &str) -> Result<T> {
        let raw = match self.provider.kind {
            ProviderKind::Anthropic => self.call_anthropic(system, user).await?,
            ProviderKind::OpenAiCompat => self.call_openai_compat(system, user).await?,
            ProviderKind::Google => self.call_google(system, user).await?,
        };

        debug!("AI response: {raw}");

        let json_str = extract_json(&raw);
        serde_json::from_str(json_str)
            .with_context(|| format!("Failed to parse AI response as JSON:\n{raw}"))
    }

    async fn call_anthropic(&self, system: &str, user: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.provider.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {"role": "user", "content": user}
            ]
        });

        let response = self
            .http
            .post(&self.provider.api_url)
            .header("x-api-key", &self.provider.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to call Anthropic API")?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            anyhow::bail!("Anthropic API error ({status}): {text}");
        }

        let resp: serde_json::Value = serde_json::from_str(&text)?;
        let content = resp["content"][0]["text"]
            .as_str()
            .context("Missing text in Anthropic response")?;

        Ok(content.to_string())
    }

    async fn call_openai_compat(&self, system: &str, user: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.provider.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "max_tokens": 4096,
            "temperature": 0.1
        });

        let mut req = self
            .http
            .post(&self.provider.api_url)
            .header("content-type", "application/json");

        // Azure uses api-key header, others use Bearer token
        if self.provider.api_url.contains("openai.azure.com") {
            req = req.header("api-key", &self.provider.api_key);
        } else if !self.provider.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.provider.api_key));
        }

        let response = req
            .json(&body)
            .send()
            .await
            .context("Failed to call AI API")?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            anyhow::bail!("AI API error ({status}): {text}");
        }

        let resp: serde_json::Value = serde_json::from_str(&text)?;
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .context("Missing content in AI response")?;

        Ok(content.to_string())
    }

    async fn call_google(&self, system: &str, user: &str) -> Result<String> {
        let body = serde_json::json!({
            "system_instruction": {
                "parts": [{"text": system}]
            },
            "contents": [{
                "parts": [{"text": user}]
            }],
            "generationConfig": {
                "temperature": 0.1,
                "maxOutputTokens": 4096
            }
        });

        let url = format!("{}?key={}", self.provider.api_url, self.provider.api_key);

        let response = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to call Google Gemini API")?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            anyhow::bail!("Google Gemini API error ({status}): {text}");
        }

        let resp: serde_json::Value = serde_json::from_str(&text)?;
        let content = resp["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .context("Missing text in Google Gemini response")?;

        Ok(content.to_string())
    }
}

pub fn extract_json_from(text: &str) -> &str {
    extract_json(text)
}

fn extract_json(text: &str) -> &str {
    let trimmed = text.trim();
    // Handle ```json ... ``` blocks
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7;
        if let Some(end) = trimmed[json_start..].find("```") {
            return trimmed[json_start..json_start + end].trim();
        }
    }
    // Handle ``` ... ``` blocks
    if let Some(start) = trimmed.find("```") {
        let json_start = start + 3;
        if let Some(end) = trimmed[json_start..].find("```") {
            return trimmed[json_start..json_start + end].trim();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_plain() {
        let input = r#"{"category": "bug"}"#;
        assert_eq!(extract_json(input), input);
    }

    #[test]
    fn test_extract_json_code_block() {
        let input = "```json\n{\"category\": \"bug\"}\n```";
        assert_eq!(extract_json(input), r#"{"category": "bug"}"#);
    }
}
