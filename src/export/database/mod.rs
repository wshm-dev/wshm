use anyhow::Result;

use crate::config::DatabaseExportConfig;

use super::ExportSink;

/// Validate that a table/index name is safe for SQL identifier use.
/// Only allows alphanumeric characters and underscores.
pub fn validate_identifier(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Table name cannot be empty");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "Invalid table name '{name}': only alphanumeric characters and underscores are allowed"
        );
    }
    Ok(())
}

#[cfg(feature = "export-elastic")]
pub mod elastic;

#[cfg(feature = "export-opensearch")]
pub mod opensearch;

#[cfg(feature = "export-postgres")]
pub mod postgres;

#[cfg(feature = "export-mongodb")]
pub mod mongodb;

#[cfg(feature = "export-mysql")]
pub mod mysql;

/// Build a database sink from config. Returns None if the required feature is not enabled.
pub fn build_sink(config: &DatabaseExportConfig) -> Result<Option<Box<dyn ExportSink>>> {
    match config.provider.as_str() {
        #[cfg(feature = "export-elastic")]
        "elasticsearch" => Ok(Some(Box::new(elastic::ElasticSink::new(config)?))),

        #[cfg(feature = "export-opensearch")]
        "opensearch" => Ok(Some(Box::new(opensearch::OpenSearchSink::new(config)?))),

        #[cfg(feature = "export-postgres")]
        "postgresql" => Ok(Some(Box::new(postgres::PostgresSink::new(config)?))),

        #[cfg(feature = "export-mongodb")]
        "mongodb" => Ok(Some(Box::new(mongodb::MongoSink::new(config)?))),

        #[cfg(feature = "export-mysql")]
        "mysql" | "mariadb" => Ok(Some(Box::new(mysql::MysqlSink::new(config)?))),

        provider => {
            tracing::warn!(
                "Database provider '{provider}' is not available. \
                 Compile with the corresponding feature flag (e.g., --features export-elastic)."
            );
            Ok(None)
        }
    }
}
