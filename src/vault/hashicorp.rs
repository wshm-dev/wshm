use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::config::VaultConfig;

use super::VaultResolver;

/// HashiCorp Vault client using the vaultrs crate.
pub struct HashiCorpVault {
    client: vaultrs::client::VaultClient,
    mount: String,
}

impl HashiCorpVault {
    pub fn new(config: &VaultConfig) -> Result<Self> {
        let address = config.address.as_deref().unwrap_or("http://127.0.0.1:8200");

        let token = std::env::var("VAULT_TOKEN")
            .context("VAULT_TOKEN env var required for HashiCorp Vault")?;

        let mut settings = vaultrs::client::VaultClientSettingsBuilder::default();
        settings.address(address).token(token);
        let client = vaultrs::client::VaultClient::new(settings.build()?)?;

        let mount = config.mount.clone().unwrap_or_else(|| "secret".to_string());

        Ok(Self { client, mount })
    }
}

#[async_trait]
impl VaultResolver for HashiCorpVault {
    async fn resolve(&self, path: &str) -> Result<String> {
        // Path format: "path/to/secret:key" or just "path/to/secret" (uses "value" key)
        let (secret_path, key) = if let Some(idx) = path.rfind(':') {
            (&path[..idx], &path[idx + 1..])
        } else {
            (path, "value")
        };

        let secret: std::collections::HashMap<String, String> =
            vaultrs::kv2::read(&self.client, &self.mount, secret_path).await?;

        secret
            .get(key)
            .cloned()
            .with_context(|| format!("Key '{key}' not found in vault secret '{secret_path}'"))
    }

    fn name(&self) -> &str {
        "hashicorp-vault"
    }
}
