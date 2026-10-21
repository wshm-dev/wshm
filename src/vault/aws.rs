use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::OnceCell;

use super::VaultResolver;

/// AWS Secrets Manager client. The SDK config (standard AWS credential
/// chain) is loaded lazily on first use.
pub struct AwsSecretsManager {
    client: OnceCell<aws_sdk_secretsmanager::Client>,
}

impl AwsSecretsManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: OnceCell::new(),
        })
    }

    async fn client(&self) -> &aws_sdk_secretsmanager::Client {
        self.client
            .get_or_init(|| async {
                let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .load()
                    .await;
                aws_sdk_secretsmanager::Client::new(&sdk_config)
            })
            .await
    }
}

#[async_trait]
impl VaultResolver for AwsSecretsManager {
    async fn resolve(&self, path: &str) -> Result<String> {
        let result = self
            .client()
            .await
            .get_secret_value()
            .secret_id(path)
            .send()
            .await
            .with_context(|| format!("Failed to resolve AWS secret '{path}'"))?;

        result
            .secret_string()
            .map(|s| s.to_string())
            .with_context(|| format!("AWS secret '{path}' has no string value"))
    }

    fn name(&self) -> &str {
        "aws-secrets-manager"
    }
}
