use anyhow::{Context, Result};
use async_trait::async_trait;
use google_cloud_secretmanager_v1::client::SecretManagerService;
use tokio::sync::OnceCell;

use super::VaultResolver;

/// GCP Secret Manager client (application default credentials). The client
/// is built lazily on first use.
pub struct GcpSecretManager {
    client: OnceCell<SecretManagerService>,
}

impl GcpSecretManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: OnceCell::new(),
        })
    }

    async fn client(&self) -> Result<&SecretManagerService> {
        self.client
            .get_or_try_init(|| async {
                SecretManagerService::builder()
                    .build()
                    .await
                    .context("Failed to create GCP Secret Manager client")
            })
            .await
    }
}

#[async_trait]
impl VaultResolver for GcpSecretManager {
    /// `path` is the full version name,
    /// e.g. `projects/p/secrets/s/versions/latest`.
    async fn resolve(&self, path: &str) -> Result<String> {
        let result = self
            .client()
            .await?
            .access_secret_version()
            .set_name(path)
            .send()
            .await
            .with_context(|| format!("Failed to resolve GCP secret '{path}'"))?;

        let payload = result.payload.context("GCP secret has no payload")?;

        String::from_utf8(payload.data.to_vec()).context("GCP secret is not valid UTF-8")
    }

    fn name(&self) -> &str {
        "gcp-secret-manager"
    }
}
