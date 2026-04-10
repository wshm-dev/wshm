use anyhow::{Context, Result};
use async_trait::async_trait;

use super::VaultResolver;

/// GCP Secret Manager client.
pub struct GcpSecretManager {
    client: google_cloud_secretmanager_v1::client::SecretManagerServiceClient,
}

impl GcpSecretManager {
    pub async fn new() -> Result<Self> {
        let config = google_cloud_secretmanager_v1::client::SecretManagerServiceConfig::default()
            .with_auth()
            .await?;
        let client =
            google_cloud_secretmanager_v1::client::SecretManagerServiceClient::new(config).await?;
        Ok(Self { client })
    }
}

#[async_trait]
impl VaultResolver for GcpSecretManager {
    async fn resolve(&self, path: &str) -> Result<String> {
        // path format: "projects/PROJECT/secrets/NAME/versions/latest"
        use google_cloud_secretmanager_v1::google::cloud::secretmanager::v1::AccessSecretVersionRequest;

        let result = self
            .client
            .access_secret_version(
                AccessSecretVersionRequest {
                    name: path.to_string(),
                },
                None,
            )
            .await
            .with_context(|| format!("Failed to resolve GCP secret '{path}'"))?;

        let payload = result
            .into_inner()
            .payload
            .context("GCP secret has no payload")?;

        String::from_utf8(payload.data).context("GCP secret is not valid UTF-8")
    }

    fn name(&self) -> &str {
        "gcp-secret-manager"
    }
}
