use anyhow::{Context, Result};
use async_trait::async_trait;

use super::VaultResolver;

/// AWS Secrets Manager client.
pub struct AwsSecretsManager {
    client: aws_sdk_secretsmanager::Client,
}

impl AwsSecretsManager {
    pub async fn new() -> Result<Self> {
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        let client = aws_sdk_secretsmanager::Client::new(&sdk_config);
        Ok(Self { client })
    }
}

#[async_trait]
impl VaultResolver for AwsSecretsManager {
    async fn resolve(&self, path: &str) -> Result<String> {
        let result = self
            .client
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
