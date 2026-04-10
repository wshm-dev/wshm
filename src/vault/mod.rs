use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;

#[cfg(feature = "vault-hashicorp")]
pub mod hashicorp;

#[cfg(feature = "vault-aws")]
pub mod aws;

#[cfg(feature = "vault-azure")]
pub mod azure;

#[cfg(feature = "vault-gcp")]
pub mod gcp;

/// Trait for resolving secrets from a vault backend.
#[async_trait]
pub trait VaultResolver: Send + Sync {
    /// Resolve a secret at the given path. Returns the secret value.
    async fn resolve(&self, path: &str) -> Result<String>;

    /// Human-readable name for logging.
    fn name(&self) -> &str;
}

/// Build a VaultResolver from config. Returns None if vault is not configured,
/// feature not enabled, or Pro license missing.
///
/// Vault integration (HashiCorp, AWS Secrets Manager, Azure Key Vault, GCP
/// Secret Manager) is a wshm Pro feature. Without a license, vault() placeholders
/// in config files are left unresolved and an error is logged.
pub fn build_resolver(
    config: &crate::config::VaultConfig,
) -> Result<Option<Box<dyn VaultResolver>>> {
    if !crate::pro_hooks::has_feature("vault") {
        tracing::warn!(
            "Vault integration ({}) configured but requires wshm Pro. \
             Run `wshm login --license` to activate.",
            config.provider
        );
        return Ok(None);
    }

    match config.provider.as_str() {
        #[cfg(feature = "vault-hashicorp")]
        "hashicorp" => Ok(Some(Box::new(hashicorp::HashiCorpVault::new(config)?))),

        #[cfg(feature = "vault-aws")]
        "aws" => Ok(Some(Box::new(aws::AwsSecretsManager::new()?))),

        #[cfg(feature = "vault-azure")]
        "azure" => Ok(Some(Box::new(azure::AzureKeyVault::new(config)?))),

        #[cfg(feature = "vault-gcp")]
        "gcp" => Ok(Some(Box::new(gcp::GcpSecretManager::new()?))),

        provider => {
            tracing::warn!(
                "Vault provider '{provider}' is not available. \
                 Compile with the corresponding feature flag (e.g., --features vault-hashicorp)."
            );
            Ok(None)
        }
    }
}

/// Resolve all `vault(path/key)` placeholders in a string.
/// Returns the string with placeholders replaced by secret values.
pub async fn resolve_placeholders(input: &str, resolver: &dyn VaultResolver) -> Result<String> {
    static RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r#"vault\(([^)]+)\)"#).unwrap());
    let mut result = input.to_string();

    // Collect all matches first to avoid borrow issues
    let matches: Vec<(String, String)> = RE
        .captures_iter(input)
        .map(|cap| {
            let full = cap[0].to_string();
            let path = cap[1].to_string();
            (full, path)
        })
        .collect();

    for (placeholder, path) in matches {
        let secret = resolver.resolve(&path).await?;
        result = result.replace(&placeholder, &secret);
    }

    Ok(result)
}

/// Check if a string contains any `vault()` placeholders.
pub fn has_vault_placeholders(s: &str) -> bool {
    s.contains("vault(")
}
