use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use tracing::debug;
use zeroize::Zeroizing;

use crate::config::Config;

struct ProviderConfig {
    api_url: String,
    api_key: Zeroizing<String>,
    model: String,
    kind: ProviderKind,
    is_oauth: bool,
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
fn resolve_provider(config: &Config, model_override: Option<&str>) -> Result<ProviderConfig> {
    let provider = config.ai.provider.as_str();
    let model = model_override
        .map(String::from)
        .unwrap_or_else(|| config.ai.model.clone());
    let base_url = config.ai.base_url.clone();

    match provider {
        // ── Anthropic (custom API format) ──────────────────────────
        "anthropic" => {
            let (key, is_oauth) = crate::login::resolve_anthropic_auth()
                .ok_or_else(|| anyhow::anyhow!(
                    "No Anthropic credentials found. Run `wshm login --claude` (Max/Pro) or set ANTHROPIC_API_KEY"
                ))?;
            Ok(ProviderConfig {
                api_url: base_url.unwrap_or_else(|| "https://api.anthropic.com/v1/messages".into()),
                api_key: Zeroizing::new(key),
                model,
                kind: ProviderKind::Anthropic,
                is_oauth,
            })
        }

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
            is_oauth: false,
        }),

        // ── OpenAI ─────────────────────────────────────────────────
        "openai" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".into()),
            api_key: env_key(&["OPENAI_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Mistral ────────────────────────────────────────────────
        "mistral" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.mistral.ai/v1/chat/completions".into()),
            api_key: env_key(&["MISTRAL_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Groq ──────────────────────────────────────────────────
        "groq" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.groq.com/openai/v1/chat/completions".into()),
            api_key: env_key(&["GROQ_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── DeepSeek ──────────────────────────────────────────────
        "deepseek" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.deepseek.com/chat/completions".into()),
            api_key: env_key(&["DEEPSEEK_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── xAI (Grok) ───────────────────────────────────────────
        "xai" | "grok" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.x.ai/v1/chat/completions".into()),
            api_key: env_key(&["XAI_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Together AI ───────────────────────────────────────────
        "together" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.together.xyz/v1/chat/completions".into()),
            api_key: env_key(&["TOGETHER_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Fireworks AI ──────────────────────────────────────────
        "fireworks" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.fireworks.ai/inference/v1/chat/completions".into()),
            api_key: env_key(&["FIREWORKS_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Perplexity ───────────────────────────────────────────
        "perplexity" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.perplexity.ai/chat/completions".into()),
            api_key: env_key(&["PERPLEXITY_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Cohere ────────────────────────────────────────────────
        "cohere" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://api.cohere.com/v2/chat".into()),
            api_key: env_key(&["COHERE_API_KEY", "CO_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── OpenRouter (aggregator) ───────────────────────────────
        "openrouter" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "https://openrouter.ai/api/v1/chat/completions".into()),
            api_key: env_key(&["OPENROUTER_API_KEY"])?,
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
        }),

        // ── Ollama (local, no API key) ────────────────────────────
        "ollama" => Ok(ProviderConfig {
            api_url: base_url
                .unwrap_or_else(|| "http://localhost:11434/v1/chat/completions".into()),
            // Optional auth header — most local Ollama deployments leave
            // this blank; route through the secret store on the off
            // chance someone fronts Ollama with a reverse-proxy auth.
            api_key: env_key(&["OLLAMA_API_KEY"])
                .unwrap_or_else(|_| Zeroizing::new(String::new())),
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
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
                is_oauth: false,
            })
        }

        // ── Custom OpenAI-compatible endpoint ─────────────────────
        "custom" => Ok(ProviderConfig {
            api_url: base_url.context("Set base_url in [ai] config for custom provider")?,
            api_key: env_key(&["WSHM_AI_API_KEY", "AI_API_KEY"]).unwrap_or_else(|_| Zeroizing::new(String::new())),
            model,
            kind: ProviderKind::OpenAiCompat,
            is_oauth: false,
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

/// Resolve an AI provider key, checking the encrypted secret store first.
///
/// Order: `secrets::resolve_blocking(global, <store_key>)` → env vars in
/// order. The store key uses lowercase form (`anthropic_api_key`,
/// `openai_api_key`, …) so admins setting a key via Settings → Secrets
/// don't have to deal with provider-specific env-var naming.
///
/// Each `name` in `env_names` is also tried on the secret store as
/// lowercase (`OPENAI_API_KEY` → `openai_api_key`) before falling back
/// to the env var. Fixes the audit finding that 13/14 providers
/// bypassed the encrypted vault entirely.
fn env_key(names: &[&str]) -> Result<Zeroizing<String>> {
    if let Some(s) = crate::secrets::global() {
        for name in names {
            let store_key = name.to_ascii_lowercase();
            if let Ok(Some(v)) = s.get_blocking(crate::secrets::Scope::Global, None, &store_key) {
                if !v.is_empty() {
                    return Ok(Zeroizing::new(v));
                }
            }
        }
    }
    for name in names {
        if let Ok(val) = std::env::var(name) {
            if !val.is_empty() {
                return Ok(Zeroizing::new(val));
            }
        }
    }
    let vars = names.join(" or ");
    anyhow::bail!("Set {vars} environment variable")
}

/// AI API request timeout (LLM calls can be slow for large prompts).
const AI_REQUEST_TIMEOUT_SECS: u64 = 120;
/// TCP connection timeout for AI API calls.
const AI_CONNECT_TIMEOUT_SECS: u64 = 10;

impl AiClient {
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(AI_REQUEST_TIMEOUT_SECS))
            .connect_timeout(std::time::Duration::from_secs(AI_CONNECT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// Create an AiClient with a model override (for per-pipeline model config).
    pub fn with_model(config: &Config, model: &str) -> Result<Self> {
        let provider = resolve_provider(config, Some(model))?;
        Ok(Self {
            http: Self::build_http_client(),
            provider,
        })
    }

    pub async fn complete<T: DeserializeOwned>(&self, system: &str, user: &str) -> Result<T> {
        let raw = match (&self.provider.kind, self.provider.is_oauth) {
            (ProviderKind::Anthropic, true) => self.call_claude_cli(system, user).await?,
            (ProviderKind::Anthropic, false) => self.call_anthropic(system, user).await?,
            (ProviderKind::OpenAiCompat, _) => self.call_openai_compat(system, user).await?,
            (ProviderKind::Google, _) => self.call_google(system, user).await?,
        };

        debug!("AI response: {raw}");

        let json_str = extract_json(&raw);
        serde_json::from_str(json_str)
            .with_context(|| format!("Failed to parse AI response as JSON:\n{raw}"))
    }

    /// Call Claude via the `claude` CLI (`claude -p`).
    /// Used for Max/Pro/Team subscriptions (OAuth) — no API key needed.
    async fn call_claude_cli(&self, system: &str, user: &str) -> Result<String> {
        debug!("Calling claude CLI (OAuth/Max/Pro mode)");

        let prompt = format!("{system}\n\n{user}");

        let output = tokio::process::Command::new("claude")
            .args(["-p", &prompt, "--output-format", "text"])
            .env("CLAUDE_MODEL", &self.provider.model)
            .output()
            .await
            .context("Failed to run `claude -p`. Is claude CLI installed? (npm install -g @anthropic-ai/claude-code)")?;

        if !output.status.success() {
            // The claude CLI writes operational failures (usage limit, auth,
            // low credit) to stdout, not stderr — so capture both plus the
            // exit code, otherwise the logged error is empty and useless.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail: String = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" | ");
            let detail = if detail.is_empty() {
                "no output".to_string()
            } else {
                detail
            };
            // Surface usage/rate limits distinctly so operators know to wait
            // for the reset or switch credentials, not chase a phantom error.
            if detail.to_ascii_lowercase().contains("limit") {
                anyhow::bail!("claude CLI usage limit reached: {detail}");
            }
            anyhow::bail!(
                "claude CLI failed (exit {}): {detail}",
                output.status.code().unwrap_or(-1)
            );
        }

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.trim().is_empty() {
            anyhow::bail!("claude CLI returned empty response");
        }

        Ok(text)
    }

    /// Call Anthropic API directly with an API key.
    async fn call_anthropic(&self, system: &str, user: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.provider.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {"role": "user", "content": user}
            ]
        });

        // Connect-only retry: a completion is non-idempotent (each accepted
        // request is billed), so don't re-issue on a post-send body EOF.
        let text = crate::retry::with_retry_connect_only("AI: Anthropic", || async {
            let response = self
                .http
                .post(&self.provider.api_url)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("x-api-key", self.provider.api_key.as_str())
                .json(&body)
                .send()
                .await
                .context("Failed to call Anthropic API")?;

            let status = response.status();
            let text = response.text().await?;

            if !status.is_success() {
                let safe_text = truncate_error_body(&text);
                anyhow::bail!("Anthropic API error ({status}): {safe_text}");
            }
            Ok::<_, anyhow::Error>(text)
        })
        .await?;

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

        // Connect-only retry: see Anthropic path above.
        let text = crate::retry::with_retry_connect_only("AI: OpenAI-compatible", || async {
            let mut req = self
                .http
                .post(&self.provider.api_url)
                .header("content-type", "application/json");

            // Azure uses api-key header, others use Bearer token
            if self.provider.api_url.contains("openai.azure.com") {
                req = req.header("api-key", self.provider.api_key.as_str());
            } else if !self.provider.api_key.is_empty() {
                req = req.header(
                    "Authorization",
                    format!("Bearer {}", self.provider.api_key.as_str()),
                );
            }

            let response = req
                .json(&body)
                .send()
                .await
                .context("Failed to call AI API")?;

            let status = response.status();
            let text = response.text().await?;

            if !status.is_success() {
                let safe_text = truncate_error_body(&text);
                anyhow::bail!("AI API error ({status}): {safe_text}");
            }
            Ok::<_, anyhow::Error>(text)
        })
        .await?;

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

        // Connect-only retry: see Anthropic path above.
        let text = crate::retry::with_retry_connect_only("AI: Google Gemini", || async {
            let response = self
                .http
                .post(&self.provider.api_url)
                .header("content-type", "application/json")
                .header("x-goog-api-key", self.provider.api_key.as_str())
                .json(&body)
                .send()
                .await
                .context("Failed to call Google Gemini API")?;

            let status = response.status();
            let text = response.text().await?;

            if !status.is_success() {
                let safe_text = truncate_error_body(&text);
                anyhow::bail!("Google Gemini API error ({status}): {safe_text}");
            }
            Ok::<_, anyhow::Error>(text)
        })
        .await?;

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
    // Handle ```json ... ``` blocks (safe slicing via .get() to avoid panics on UTF-8 boundaries)
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7;
        if let Some(rest) = trimmed.get(json_start..) {
            if let Some(end) = rest.find("```") {
                if let Some(block) = trimmed.get(json_start..json_start + end) {
                    return block.trim();
                }
            }
        }
    }
    // Handle ``` ... ``` blocks
    if let Some(start) = trimmed.find("```") {
        let json_start = start + 3;
        if let Some(rest) = trimmed.get(json_start..) {
            if let Some(end) = rest.find("```") {
                if let Some(block) = trimmed.get(json_start..json_start + end) {
                    return block.trim();
                }
            }
        }
    }
    trimmed
}

/// Truncate API error body to avoid leaking sensitive data in logs.
/// Keeps first 200 chars which is enough for debugging without exposing tokens.
fn truncate_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() <= 200 {
        trimmed.to_string()
    } else {
        // Find a safe UTF-8 boundary near 200 chars
        let end = trimmed
            .char_indices()
            .nth(200)
            .map(|(i, _)| i)
            .unwrap_or(trimmed.len());
        format!("{}… (truncated)", &trimmed[..end])
    }
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
