use anyhow::{Context, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use tracing::{debug, info};

use super::client::extract_json_from;

/// Known models with their HuggingFace repo and filename.
pub struct ModelSpec {
    pub name: &'static str,
    pub repo: &'static str,
    pub filename: &'static str,
    pub description: &'static str,
    pub size_mb: u32,
}

pub const KNOWN_MODELS: &[ModelSpec] = &[
    // ── Small (2-3B) — fast, ~2 GB ────────────────────────────
    ModelSpec {
        name: "llama3-3b",
        repo: "bartowski/Llama-3.2-3B-Instruct-GGUF",
        filename: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        description: "Meta Llama 3.2 3B (Q4_K_M) — versatile baseline",
        size_mb: 2000,
    },
    ModelSpec {
        name: "smollm3-3b",
        repo: "bartowski/SmolLM3-3B-Instruct-GGUF",
        filename: "SmolLM3-3B-Instruct-Q4_K_M.gguf",
        description: "HuggingFace SmolLM3 3B (Q4_K_M) — tiny, efficient",
        size_mb: 2000,
    },
    // ── Medium (4B) — good balance ────────────────────────────
    ModelSpec {
        name: "phi4-mini",
        repo: "bartowski/phi-4-mini-instruct-GGUF",
        filename: "phi-4-mini-instruct-Q4_K_M.gguf",
        description: "Microsoft Phi-4 Mini 3.8B (Q4_K_M) — excellent classification",
        size_mb: 2400,
    },
    ModelSpec {
        name: "qwen3-4b",
        repo: "bartowski/Qwen3-4B-GGUF",
        filename: "Qwen3-4B-Q4_K_M.gguf",
        description: "Alibaba Qwen3 4B (Q4_K_M) — strong reasoning",
        size_mb: 2600,
    },
    ModelSpec {
        name: "gemma3-4b",
        repo: "bartowski/gemma-3-4b-it-GGUF",
        filename: "gemma-3-4b-it-Q4_K_M.gguf",
        description: "Google Gemma 3 4B (Q4_K_M) — reliable JSON output",
        size_mb: 2800,
    },
    // ── Large (7-8B) — best quality, fits 16GB RAM ───────────
    ModelSpec {
        name: "mistral-7b",
        repo: "bartowski/Mistral-7B-Instruct-v0.3-GGUF",
        filename: "Mistral-7B-Instruct-v0.3-Q4_K_M.gguf",
        description: "Mistral 7B Instruct v0.3 (Q4_K_M) — strong general purpose",
        size_mb: 4400,
    },
    ModelSpec {
        name: "mistral-nemo",
        repo: "bartowski/Mistral-Nemo-Instruct-2407-GGUF",
        filename: "Mistral-Nemo-Instruct-2407-Q4_K_M.gguf",
        description: "Mistral Nemo 12B (Q4_K_M) — best Mistral, great reasoning",
        size_mb: 7400,
    },
    ModelSpec {
        name: "llama3-8b",
        repo: "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
        filename: "Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        description: "Meta Llama 3.1 8B (Q4_K_M) — excellent all-rounder",
        size_mb: 4900,
    },
    ModelSpec {
        name: "qwen3-8b",
        repo: "bartowski/Qwen3-8B-GGUF",
        filename: "Qwen3-8B-Q4_K_M.gguf",
        description: "Alibaba Qwen3 8B (Q4_K_M) — top-tier reasoning & JSON",
        size_mb: 5000,
    },
    ModelSpec {
        name: "gemma3-12b",
        repo: "bartowski/gemma-3-12b-it-GGUF",
        filename: "gemma-3-12b-it-Q4_K_M.gguf",
        description: "Google Gemma 3 12B (Q4_K_M) — best at structured output",
        size_mb: 7800,
    },
];

pub fn models_dir() -> PathBuf {
    let dir = dirs_or_default().join("models");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn dirs_or_default() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wshm")
}

pub fn resolve_model_path(model_name: &str) -> Option<PathBuf> {
    // Check if it's a direct path to a GGUF file
    let as_path = PathBuf::from(model_name);
    if as_path.exists() && model_name.ends_with(".gguf") {
        return Some(as_path);
    }

    // Check known models
    if let Some(spec) = KNOWN_MODELS.iter().find(|m| m.name == model_name) {
        let path = models_dir().join(spec.filename);
        if path.exists() {
            return Some(path);
        }
    }

    // Check models dir for exact filename
    let path = models_dir().join(model_name);
    if path.exists() {
        return Some(path);
    }

    // Check with .gguf extension
    let path = models_dir().join(format!("{model_name}.gguf"));
    if path.exists() {
        return Some(path);
    }

    None
}

pub fn pull_model(name: &str) -> Result<PathBuf> {
    let spec = KNOWN_MODELS
        .iter()
        .find(|m| m.name == name)
        .with_context(|| {
            let available: Vec<&str> = KNOWN_MODELS.iter().map(|m| m.name).collect();
            format!(
                "Unknown model: '{name}'. Available: {}",
                available.join(", ")
            )
        })?;

    let dest = models_dir().join(spec.filename);
    if dest.exists() {
        println!("Model '{}' already downloaded: {}", name, dest.display());
        return Ok(dest);
    }

    println!(
        "Downloading {} (~{} MB) from {}...",
        spec.name, spec.size_mb, spec.repo
    );

    let api = hf_hub::api::sync::Api::new().context("Failed to initialize HuggingFace API")?;
    let repo = api.model(spec.repo.to_string());
    let path = repo
        .get(spec.filename)
        .with_context(|| format!("Failed to download {}", spec.filename))?;

    // Copy from HF cache to our models dir
    std::fs::copy(&path, &dest)
        .with_context(|| format!("Failed to copy model to {}", dest.display()))?;

    println!("Downloaded to: {}", dest.display());
    Ok(dest)
}

pub fn list_models() -> Result<Vec<(String, u64, bool)>> {
    let dir = models_dir();
    let mut models = Vec::new();

    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "gguf") {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let size = entry.metadata()?.len();
                models.push((name, size, true));
            }
        }
    }

    // Add known models that aren't downloaded yet
    for spec in KNOWN_MODELS {
        let path = dir.join(spec.filename);
        if !path.exists() {
            models.push((spec.name.to_string(), spec.size_mb as u64 * 1_000_000, false));
        }
    }

    Ok(models)
}

pub struct LocalClient {
    backend: LlamaBackend,
    model_path: PathBuf,
}

impl LocalClient {
    pub fn new(model_name: &str) -> Result<Self> {
        let model_path = resolve_model_path(model_name).with_context(|| {
            format!("Model '{model_name}' not found. Run: wshm model pull {model_name}")
        })?;

        let backend = LlamaBackend::init().context("Failed to initialize llama backend")?;

        info!("Local model: {}", model_path.display());
        Ok(Self {
            backend,
            model_path,
        })
    }

    pub fn complete<T: DeserializeOwned>(&self, system: &str, user: &str) -> Result<T> {
        let raw = self.generate(system, user)?;
        debug!("Local AI response: {raw}");

        let json_str = extract_json_from(&raw);
        serde_json::from_str(json_str)
            .with_context(|| format!("Failed to parse local AI response as JSON:\n{raw}"))
    }

    fn generate(&self, system: &str, user: &str) -> Result<String> {
        let model_params = LlamaModelParams::default().with_n_gpu_layers(9999);

        let model = LlamaModel::load_from_file(&self.backend, &self.model_path, &model_params)
            .context("Failed to load GGUF model")?;

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(8192))
            .with_n_batch(8192);
        let mut ctx = model
            .new_context(&self.backend, ctx_params)
            .context("Failed to create inference context")?;

        // Build chat prompt using model's template
        let messages = vec![
            LlamaChatMessage::new("system".to_string(), system.to_string())
                .context("Failed to create system message")?,
            LlamaChatMessage::new("user".to_string(), user.to_string())
                .context("Failed to create user message")?,
        ];

        let template = model
            .chat_template(None)
            .unwrap_or_else(|_| {
                LlamaChatTemplate::new("chatml").expect("Failed to create chatml template")
            });

        let prompt = model
            .apply_chat_template(&template, &messages, true)
            .context("Failed to apply chat template")?;

        // Tokenize
        let mut tokens = model
            .str_to_token(&prompt, AddBos::Always)
            .context("Tokenization failed")?;

        // Truncate if prompt exceeds context budget (leave room for 1024 output tokens)
        let max_prompt_tokens = 7000;
        if tokens.len() > max_prompt_tokens {
            info!("Truncating prompt from {} to {} tokens", tokens.len(), max_prompt_tokens);
            tokens.truncate(max_prompt_tokens);
        }

        info!("Prompt tokens: {}", tokens.len());

        // Load tokens into batch
        let mut batch = LlamaBatch::new(tokens.len().max(8192), 1);
        let last_idx = tokens.len() as i32 - 1;
        for (i, token) in (0_i32..).zip(tokens.iter()) {
            batch
                .add(*token, i, &[0], i == last_idx)
                .context("Failed to add token to batch")?;
        }

        // Process prompt
        ctx.decode(&mut batch).context("Prompt decode failed")?;

        // Generate
        let mut n_cur = batch.n_tokens();
        let n_max = n_cur + 4096;
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::temp(0.1),
            LlamaSampler::greedy(),
        ]);
        let mut output = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();

        while n_cur < n_max {
            let token = sampler.sample(&ctx, batch.n_tokens() as i32 - 1);
            sampler.accept(token);

            if model.is_eog_token(token) {
                debug!("EOG token after {} output chars", output.len());
                break;
            }

            let piece = model.token_to_piece(token, &mut decoder, true, None)
                .unwrap_or_default();
            output.push_str(&piece);

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .context("Failed to add generated token")?;
            n_cur += 1;
            ctx.decode(&mut batch).context("Decode failed")?;
        }

        Ok(output)
    }
}
