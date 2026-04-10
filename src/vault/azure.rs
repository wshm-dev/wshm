use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::config::VaultConfig;

use super::VaultResolver;

/// Azure Key Vault client.
pub struct AzureKeyVault {
    client: azure_security_keyvault::SecretClient,
}

impl AzureKeyVault {
    pub fn new(config: &VaultConfig) -> Result<Self> {
        let vault_url = config
            .address
            .as_deref()
            .context("Azure Key Vault requires 'address' in [vault] config")?;

        let credential = azure_identity::DefaultAzureCredential::new()
            .context("Failed to create Azure credentials")?;

        let client =
            azure_security_keyvault::SecretClient::new(vault_url, std::sync::Arc::new(credential))
                .context("Failed to create Azure Key Vault client")?;

        Ok(Self { client })
    }
}

#[async_trait]
impl VaultResolver for AzureKeyVault {
    async fn resolve(&self, path: &str) -> Result<String> {
        let secret = self
            .client
            .get(path)
            .await
            .with_context(|| format!("Failed to resolve Azure Key Vault secret '{path}'"))?;

        Ok(secret.value)
    }

    fn name(&self) -> &str {
        "azure-keyvault"
    }
}
