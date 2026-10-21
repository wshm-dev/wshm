//! Object storage targets: S3 (and S3-compatible: MinIO, OVH, Scaleway, R2...),
//! Google Cloud Storage, Azure Blob Storage and a local directory, all behind
//! one small API built on the [`object_store`] crate.
//!
//! Used by the cloud-storage export sinks and by anything that needs to
//! publish files (e.g. HTML pages) to a bucket.
//!
//! Credentials are never read from the TOML config. Each provider uses its
//! standard environment / ambient credential chain:
//! - `s3`: `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`,
//!   web identity (`AWS_WEB_IDENTITY_TOKEN_FILE`, IRSA), ECS container
//!   credentials, then the EC2 instance metadata service.
//! - `gcs`: `GOOGLE_APPLICATION_CREDENTIALS` / `GOOGLE_SERVICE_ACCOUNT*`,
//!   gcloud application-default credentials, then the GCE metadata server
//!   (workload identity).
//! - `azure`: `AZURE_STORAGE_*` (account key / SAS), `AZURE_CLIENT_*`
//!   (service principal / workload identity), then managed identity (IMDS).
//!
//! Cloud providers are compiled in by the `storage-s3`, `storage-gcs` and
//! `storage-azure` cargo features; `local` is always available.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use object_store::path::Path;
use object_store::{Attribute, Attributes, ObjectStore, PutOptions, PutPayload};
use serde::{Deserialize, Serialize};

/// Configuration of one object storage target (TOML section such as
/// `[export.storage]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct StorageTargetConfig {
    /// `s3`, `gcs`, `azure` or `local`.
    pub provider: String,

    /// Bucket name (`s3`, `gcs`). Accepted as the container name for `azure`.
    #[serde(default)]
    pub bucket: Option<String>,

    /// Key prefix prepended to every object key (e.g. `wshm/exports`).
    #[serde(default)]
    pub prefix: Option<String>,

    /// Region (`s3`). Falls back to `AWS_REGION` / `AWS_DEFAULT_REGION`.
    #[serde(default)]
    pub region: Option<String>,

    /// Custom API endpoint: S3-compatible services (`s3`), emulators
    /// (fake-gcs-server for `gcs`, Azurite for `azure`).
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Storage account (`azure`). Falls back to `AZURE_STORAGE_ACCOUNT_NAME`.
    #[serde(default)]
    pub account: Option<String>,

    /// Blob container (`azure`).
    #[serde(default)]
    pub container: Option<String>,

    /// Target directory (`local`).
    #[serde(default)]
    pub dir: Option<String>,

    /// Public base URL of the bucket / container / directory root (CDN,
    /// static website endpoint, web server). The prefix and key are appended.
    #[serde(default)]
    pub public_url: Option<String>,
}

/// Returns true if `provider` is compiled into this binary.
pub fn provider_available(provider: &str) -> bool {
    provider == "local"
        || (provider == "s3" && cfg!(feature = "storage-s3"))
        || (provider == "gcs" && cfg!(feature = "storage-gcs"))
        || (provider == "azure" && cfg!(feature = "storage-azure"))
}

/// A configured object storage destination.
pub struct ObjectTarget {
    store: Arc<dyn ObjectStore>,
    provider: String,
    /// Normalised prefix: no leading/trailing `/`, may be empty.
    prefix: String,
    /// Base URL (no trailing `/`) under which objects are publicly readable.
    public_base: Option<String>,
    /// Whether the backend stores per-object attributes (content type).
    /// The local filesystem backend does not.
    supports_attributes: bool,
}

impl std::fmt::Debug for ObjectTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectTarget")
            .field("provider", &self.provider)
            .field("store", &self.store.to_string())
            .field("prefix", &self.prefix)
            .field("public_base", &self.public_base)
            .finish()
    }
}

impl ObjectTarget {
    /// Build a target from config. Fails on missing required fields or when
    /// the provider is not compiled in.
    pub fn from_config(config: &StorageTargetConfig) -> Result<ObjectTarget> {
        let provider = config.provider.trim().to_ascii_lowercase();
        let prefix = normalise_prefix(config.prefix.as_deref().unwrap_or_default());
        if !prefix.is_empty() {
            check_portable(&prefix)
                .with_context(|| format!("invalid storage prefix '{prefix}'"))?;
        }
        let configured_url = non_empty(&config.public_url);

        let (store, default_url, supports_attributes): (
            Arc<dyn ObjectStore>,
            Option<String>,
            bool,
        ) = match provider.as_str() {
            "local" => {
                let dir =
                    non_empty(&config.dir).context("storage provider 'local' requires `dir`")?;
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("cannot create storage dir '{dir}'"))?;
                let store = object_store::local::LocalFileSystem::new_with_prefix(dir)
                    .with_context(|| format!("cannot open storage dir '{dir}'"))?;
                (Arc::new(store), None, false)
            }
            "s3" => {
                let (store, url) = build_s3(config)?;
                (store, url, true)
            }
            "gcs" => {
                let (store, url) = build_gcs(config)?;
                (store, url, true)
            }
            "azure" => {
                let (store, url) = build_azure(config)?;
                (store, url, true)
            }
            other => bail!("unknown storage provider '{other}' (expected s3, gcs, azure or local)"),
        };

        Ok(ObjectTarget {
            store,
            provider,
            prefix,
            public_base: configured_url
                .map(|u| u.trim_end_matches('/').to_string())
                .or(default_url),
            supports_attributes,
        })
    }

    /// Wrap an existing [`ObjectStore`] (e.g. `InMemory` in tests). The store
    /// must support object attributes (content type).
    pub fn from_store(
        store: Arc<dyn ObjectStore>,
        provider: &str,
        prefix: &str,
        public_url: Option<&str>,
    ) -> ObjectTarget {
        ObjectTarget {
            store,
            provider: provider.to_string(),
            prefix: normalise_prefix(prefix),
            public_base: public_url
                .filter(|u| !u.trim().is_empty())
                .map(|u| u.trim_end_matches('/').to_string()),
            supports_attributes: true,
        }
    }

    /// Provider name (`s3`, `gcs`, `azure`, `local`...).
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Full object key (prefix included) for `key`.
    pub fn full_key(&self, key: &str) -> Result<String> {
        let key = key.trim_start_matches('/');
        if key.is_empty() || key.ends_with('/') {
            bail!("invalid object key '{key}': must name an object");
        }
        let full = if self.prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}/{}", self.prefix, key)
        };
        check_portable(&full).with_context(|| format!("invalid object key '{key}'"))?;
        Ok(full)
    }

    fn path(&self, key: &str) -> Result<Path> {
        Ok(Path::parse(self.full_key(key)?)?)
    }

    /// Upload `bytes` at `key` (relative to the prefix), overwriting any
    /// existing object. `content_type` is stored as the object's
    /// `Content-Type` so browsers render it (e.g. `text/html; charset=utf-8`).
    /// The local provider has no per-file metadata: serve its directory with
    /// a web server that derives the type from the file extension.
    pub async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        if content_type.trim().is_empty() {
            bail!("content type is required for object '{key}'");
        }
        let path = self.path(key)?;
        let mut attributes = Attributes::new();
        if self.supports_attributes {
            attributes.insert(Attribute::ContentType, content_type.to_string().into());
        }
        let opts = PutOptions {
            attributes,
            ..Default::default()
        };
        self.store
            .put_opts(&path, PutPayload::from(bytes), opts)
            .await
            .with_context(|| format!("{}: failed to write '{path}'", self.provider))?;
        tracing::debug!("{}: wrote {path} ({content_type})", self.provider);
        Ok(())
    }

    /// Delete the object at `key`. Deleting a missing object is not an error.
    pub async fn delete(&self, key: &str) -> Result<()> {
        let path = self.path(key)?;
        let res = object_store::ObjectStoreExt::delete(self.store.as_ref(), &path).await;
        match res {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(anyhow::Error::new(e)
                .context(format!("{}: failed to delete '{path}'", self.provider))),
        }
    }

    /// Public URL of `key`: the configured `public_url`, else the provider's
    /// canonical HTTPS URL (AWS S3, GCS, Azure). `None` for `local` and for
    /// any custom endpoint, whether from config or env (`AWS_ENDPOINT*`,
    /// `GOOGLE_BASE_URL`, `AZURE_STORAGE_ENDPOINT`,
    /// `AZURE_STORAGE_USE_EMULATOR`): set `public_url` for those. The object is only reachable if the bucket /
    /// container grants public read (or sits behind a CDN).
    pub fn public_url(&self, key: &str) -> Option<String> {
        let base = self.public_base.as_ref()?;
        let full = self.full_key(key).ok()?;
        let encoded: Vec<String> = full
            .split('/')
            .map(|seg| urlencoding::encode(seg).into_owned())
            .collect();
        Some(format!("{base}/{}", encoded.join("/")))
    }

    /// Read back an object and its stored content type (tests only).
    #[cfg(test)]
    pub(crate) async fn get_with_content_type(
        &self,
        key: &str,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let path = self.path(key)?;
        let res = object_store::ObjectStoreExt::get(self.store.as_ref(), &path).await?;
        let ct = res
            .attributes
            .get(&Attribute::ContentType)
            .map(|v| v.to_string());
        let bytes = res.bytes().await?;
        Ok((bytes.to_vec(), ct))
    }
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Rejects what could escape the target directory or alias another object:
/// empty, `.` and `..` segments and control characters (via [`Path::parse`]),
/// plus `\` and `:`, which Windows treats as a path separator and a drive /
/// stream marker for the local provider.
fn check_portable(path: &str) -> Result<()> {
    if let Some(c) = path.chars().find(|c| matches!(c, '\\' | ':')) {
        bail!("character {c:?} is not allowed");
    }
    Path::parse(path)?;
    Ok(())
}

fn normalise_prefix(prefix: &str) -> String {
    prefix
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

type Built = (Arc<dyn ObjectStore>, Option<String>);

#[cfg(feature = "storage-s3")]
fn build_s3(config: &StorageTargetConfig) -> Result<Built> {
    use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};

    let bucket = non_empty(&config.bucket).context("storage provider 's3' requires `bucket`")?;
    let mut builder = AmazonS3Builder::from_env().with_bucket_name(bucket);
    let region = non_empty(&config.region)
        .map(str::to_string)
        .or_else(|| std::env::var("AWS_REGION").ok())
        .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
        .unwrap_or_else(|| "us-east-1".to_string());
    builder = builder.with_region(&region);
    let endpoint = non_empty(&config.endpoint);
    if let Some(ep) = endpoint {
        builder = builder
            .with_endpoint(ep)
            .with_allow_http(ep.starts_with("http://"));
    }
    // A custom endpoint may also come from the environment (AWS_ENDPOINT,
    // AWS_ENDPOINT_URL, AWS_ENDPOINT_URL_S3): no amazonaws.com URL then.
    let custom_endpoint = endpoint.is_some()
        || builder
            .get_config_value(&AmazonS3ConfigKey::Endpoint)
            .is_some()
        || builder
            .get_config_value(&AmazonS3ConfigKey::S3Endpoint)
            .is_some();
    let store = builder.build().context("cannot configure S3 storage")?;
    let default_url =
        (!custom_endpoint).then(|| format!("https://{bucket}.s3.{region}.amazonaws.com"));
    Ok((Arc::new(store), default_url))
}

#[cfg(not(feature = "storage-s3"))]
fn build_s3(_: &StorageTargetConfig) -> Result<Built> {
    bail!("storage provider 's3' is not compiled in (build wshm with the `storage-s3` feature)")
}

#[cfg(feature = "storage-gcs")]
fn build_gcs(config: &StorageTargetConfig) -> Result<Built> {
    use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};

    let bucket = non_empty(&config.bucket).context("storage provider 'gcs' requires `bucket`")?;
    let mut builder = GoogleCloudStorageBuilder::from_env().with_bucket_name(bucket);
    let endpoint = non_empty(&config.endpoint);
    if let Some(ep) = endpoint {
        builder = builder.with_base_url(ep);
    }
    // GOOGLE_BASE_URL in the environment also means an emulator / custom host.
    let custom_endpoint = endpoint.is_some()
        || builder
            .get_config_value(&GoogleConfigKey::BaseUrl)
            .is_some();
    let store = builder.build().context("cannot configure GCS storage")?;
    let default_url =
        (!custom_endpoint).then(|| format!("https://storage.googleapis.com/{bucket}"));
    Ok((Arc::new(store), default_url))
}

#[cfg(not(feature = "storage-gcs"))]
fn build_gcs(_: &StorageTargetConfig) -> Result<Built> {
    bail!("storage provider 'gcs' is not compiled in (build wshm with the `storage-gcs` feature)")
}

#[cfg(feature = "storage-azure")]
fn build_azure(config: &StorageTargetConfig) -> Result<Built> {
    use object_store::azure::{AzureConfigKey, MicrosoftAzureBuilder};

    let container = non_empty(&config.container)
        .or_else(|| non_empty(&config.bucket))
        .context("storage provider 'azure' requires `container`")?;
    let mut builder = MicrosoftAzureBuilder::from_env().with_container_name(container);
    let account = non_empty(&config.account)
        .map(str::to_string)
        .or_else(|| std::env::var("AZURE_STORAGE_ACCOUNT_NAME").ok());
    if let Some(ref acc) = account {
        builder = builder.with_account(acc);
    }
    let endpoint = non_empty(&config.endpoint);
    if let Some(ep) = endpoint {
        builder = builder
            .with_endpoint(ep.to_string())
            .with_allow_http(ep.starts_with("http://"));
    }
    // AZURE_STORAGE_ENDPOINT / AZURE_STORAGE_USE_EMULATOR (Azurite) in the
    // environment also mean a non-public host.
    let custom_endpoint = endpoint.is_some()
        || builder
            .get_config_value(&AzureConfigKey::Endpoint)
            .is_some()
        || builder
            .get_config_value(&AzureConfigKey::UseEmulator)
            .as_deref()
            == Some("true");
    let store = builder.build().context("cannot configure Azure storage")?;
    let default_url = match account {
        Some(acc) if !custom_endpoint => {
            Some(format!("https://{acc}.blob.core.windows.net/{container}"))
        }
        _ => None,
    };
    Ok((Arc::new(store), default_url))
}

#[cfg(not(feature = "storage-azure"))]
fn build_azure(_: &StorageTargetConfig) -> Result<Built> {
    bail!(
        "storage provider 'azure' is not compiled in (build wshm with the `storage-azure` feature)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    fn mem_target(prefix: &str, url: Option<&str>) -> ObjectTarget {
        ObjectTarget::from_store(Arc::new(InMemory::new()), "memory", prefix, url)
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let mut bytes = [0u8; 8];
        getrandom::getrandom(&mut bytes).unwrap();
        std::env::temp_dir().join(format!("wshm-storage-{tag}-{}", hex::encode(bytes)))
    }

    #[tokio::test]
    async fn memory_put_sets_content_type_and_prefix() {
        let t = mem_target("/pages/", Some("https://cdn.example.com/"));
        t.put(
            "pr/1/index.html",
            b"<h1>hi</h1>".to_vec(),
            "text/html; charset=utf-8",
        )
        .await
        .unwrap();
        assert_eq!(
            t.full_key("pr/1/index.html").unwrap(),
            "pages/pr/1/index.html"
        );
        let (bytes, ct) = t.get_with_content_type("pr/1/index.html").await.unwrap();
        assert_eq!(bytes, b"<h1>hi</h1>");
        assert_eq!(ct.as_deref(), Some("text/html; charset=utf-8"));
        assert_eq!(
            t.public_url("pr/1/index.html").as_deref(),
            Some("https://cdn.example.com/pages/pr/1/index.html")
        );
    }

    #[tokio::test]
    async fn memory_delete_is_idempotent() {
        let t = mem_target("", None);
        t.put("a.json", b"{}".to_vec(), "application/json")
            .await
            .unwrap();
        t.delete("a.json").await.unwrap();
        assert!(t.get_with_content_type("a.json").await.is_err());
        t.delete("a.json").await.unwrap();
        assert_eq!(t.public_url("a.json"), None);
    }

    #[tokio::test]
    async fn rejects_bad_keys_and_missing_content_type() {
        let t = mem_target("p", None);
        for bad in ["", "/", "a/../b", "a//b", "./a", "dir/"] {
            assert!(t.put(bad, vec![], "text/plain").await.is_err(), "{bad:?}");
        }
        assert!(t.put("ok.txt", vec![], " ").await.is_err());
    }

    #[tokio::test]
    async fn rejects_windows_separators_and_drive_markers() {
        let t = mem_target("", None);
        for bad in ["a\\..\\b", "a\\b.html", "c:/x.html", "a/b:c", "..\\escape"] {
            assert!(t.full_key(bad).is_err(), "{bad:?}");
            assert!(t.put(bad, vec![], "text/plain").await.is_err(), "{bad:?}");
        }
        assert!(t.full_key("a-b_c.d/e f.html").is_ok());
    }

    #[test]
    fn invalid_prefix_fails_config() {
        let dir = temp_dir("prefix");
        for bad in ["..", "a/../b", "./x", "a\\b", "c:"] {
            let cfg = StorageTargetConfig {
                provider: "local".into(),
                dir: Some(dir.to_string_lossy().into_owned()),
                prefix: Some(bad.into()),
                ..Default::default()
            };
            let e = ObjectTarget::from_config(&cfg).unwrap_err().to_string();
            assert!(e.contains("invalid storage prefix"), "{bad:?}: {e}");
        }
        let ok = StorageTargetConfig {
            provider: "local".into(),
            dir: Some(dir.to_string_lossy().into_owned()),
            prefix: Some("/a//b/".into()),
            ..Default::default()
        };
        let t = ObjectTarget::from_config(&ok).unwrap();
        assert_eq!(t.full_key("k").unwrap(), "a/b/k");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn public_url_encodes_segments() {
        let t = mem_target("", Some("https://x.test"));
        assert_eq!(
            t.public_url("a b/é.html").as_deref(),
            Some("https://x.test/a%20b/%C3%A9.html")
        );
    }

    #[tokio::test]
    async fn local_provider_writes_and_deletes_files() {
        let dir = temp_dir("local");
        let cfg = StorageTargetConfig {
            provider: "local".into(),
            dir: Some(dir.to_string_lossy().into_owned()),
            prefix: Some("site".into()),
            public_url: Some("http://localhost:8080".into()),
            ..Default::default()
        };
        let t = ObjectTarget::from_config(&cfg).unwrap();
        assert_eq!(t.provider(), "local");
        t.put("pr/7/index.html", b"<p>x</p>".to_vec(), "text/html")
            .await
            .unwrap();
        let file = dir.join("site/pr/7/index.html");
        assert_eq!(std::fs::read(&file).unwrap(), b"<p>x</p>");
        assert_eq!(
            t.public_url("pr/7/index.html").as_deref(),
            Some("http://localhost:8080/site/pr/7/index.html")
        );
        t.delete("pr/7/index.html").await.unwrap();
        assert!(!file.exists());
        t.delete("pr/7/index.html").await.unwrap();
        assert!(t.put("../escape.html", vec![], "text/html").await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_without_public_url_has_none() {
        let dir = temp_dir("nourl");
        let cfg = StorageTargetConfig {
            provider: "local".into(),
            dir: Some(dir.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let t = ObjectTarget::from_config(&cfg).unwrap();
        assert_eq!(t.public_url("index.html"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_errors() {
        let err = |c: StorageTargetConfig| ObjectTarget::from_config(&c).unwrap_err().to_string();
        assert!(err(StorageTargetConfig {
            provider: "ftp".into(),
            ..Default::default()
        })
        .contains("unknown storage provider"));
        assert!(err(StorageTargetConfig {
            provider: "local".into(),
            ..Default::default()
        })
        .contains("`dir`"));
        for p in ["s3", "gcs", "azure"] {
            // Without a bucket/container it fails whether or not compiled in.
            let e = err(StorageTargetConfig {
                provider: p.into(),
                ..Default::default()
            });
            assert!(
                e.contains("requires") || e.contains("not compiled in"),
                "{p}: {e}"
            );
        }
    }

    #[test]
    fn toml_parses_all_fields() {
        let cfg: StorageTargetConfig = toml::from_str(
            r#"
            provider = "s3"
            bucket = "b"
            prefix = "p"
            region = "eu-west-3"
            endpoint = "https://s3.gra.io.cloud.ovh.net"
            account = "acc"
            container = "c"
            dir = "/tmp/x"
            public_url = "https://pages.example.com"
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.endpoint.as_deref(),
            Some("https://s3.gra.io.cloud.ovh.net")
        );
        assert_eq!(cfg.public_url.as_deref(), Some("https://pages.example.com"));
    }

    #[cfg(feature = "storage-s3")]
    #[test]
    fn s3_default_and_custom_endpoint_urls() {
        let mut cfg = StorageTargetConfig {
            provider: "s3".into(),
            bucket: Some("my-bucket".into()),
            region: Some("eu-west-1".into()),
            prefix: Some("x".into()),
            ..Default::default()
        };
        let t = ObjectTarget::from_config(&cfg).unwrap();
        assert_eq!(
            t.public_url("a.html").as_deref(),
            Some("https://my-bucket.s3.eu-west-1.amazonaws.com/x/a.html")
        );
        cfg.endpoint = Some("http://127.0.0.1:9000".into());
        let t = ObjectTarget::from_config(&cfg).unwrap();
        assert_eq!(t.public_url("a.html"), None);

        // Endpoint from the environment only: still no amazonaws.com URL,
        // an explicit public_url is honoured.
        cfg.endpoint = None;
        std::env::set_var("AWS_ENDPOINT", "http://127.0.0.1:9000");
        let env_only = ObjectTarget::from_config(&cfg);
        cfg.public_url = Some("https://pages.example.com".into());
        let with_url = ObjectTarget::from_config(&cfg);
        std::env::remove_var("AWS_ENDPOINT");
        assert_eq!(env_only.unwrap().public_url("a.html"), None);
        assert_eq!(
            with_url.unwrap().public_url("a.html").as_deref(),
            Some("https://pages.example.com/x/a.html")
        );
    }

    #[cfg(feature = "storage-gcs")]
    #[test]
    fn gcs_default_and_env_base_url() {
        let cfg = StorageTargetConfig {
            provider: "gcs".into(),
            bucket: Some("gb".into()),
            ..Default::default()
        };
        // Building needs no credentials (they are resolved per request).
        let t = ObjectTarget::from_config(&cfg).unwrap();
        assert_eq!(
            t.public_url("i.html").as_deref(),
            Some("https://storage.googleapis.com/gb/i.html")
        );
        std::env::set_var("GOOGLE_BASE_URL", "http://127.0.0.1:4443");
        let emu = ObjectTarget::from_config(&cfg);
        std::env::remove_var("GOOGLE_BASE_URL");
        assert_eq!(emu.unwrap().public_url("i.html"), None);
    }

    #[cfg(feature = "storage-azure")]
    #[test]
    fn azure_default_and_emulator_url() {
        let cfg = StorageTargetConfig {
            provider: "azure".into(),
            account: Some("acct".into()),
            container: Some("pages".into()),
            ..Default::default()
        };
        let t = ObjectTarget::from_config(&cfg).unwrap();
        assert_eq!(
            t.public_url("i.html").as_deref(),
            Some("https://acct.blob.core.windows.net/pages/i.html")
        );
        std::env::set_var("AZURE_STORAGE_USE_EMULATOR", "true");
        let emu = ObjectTarget::from_config(&cfg);
        std::env::remove_var("AZURE_STORAGE_USE_EMULATOR");
        assert_eq!(emu.unwrap().public_url("i.html"), None);
    }

    /// Integration test against a local MinIO (never a real cloud account).
    ///
    /// ```sh
    /// docker run -d --rm --name wshm-minio -p 9000:9000 \
    ///   -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
    ///   minio/minio server /data
    /// docker exec wshm-minio mc alias set local http://127.0.0.1:9000 minioadmin minioadmin
    /// docker exec wshm-minio mc mb local/wshm-test
    /// AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin \
    ///   cargo test --features storage-s3 storage::tests::minio_roundtrip -- --ignored
    /// docker stop wshm-minio
    /// ```
    /// Override with `WSHM_TEST_S3_ENDPOINT` / `WSHM_TEST_S3_BUCKET`.
    #[cfg(feature = "storage-s3")]
    #[tokio::test]
    #[ignore = "needs a local MinIO, see doc comment"]
    async fn minio_roundtrip() {
        let cfg = StorageTargetConfig {
            provider: "s3".into(),
            bucket: Some(
                std::env::var("WSHM_TEST_S3_BUCKET").unwrap_or_else(|_| "wshm-test".into()),
            ),
            endpoint: Some(
                std::env::var("WSHM_TEST_S3_ENDPOINT")
                    .unwrap_or_else(|_| "http://127.0.0.1:9000".into()),
            ),
            region: Some("us-east-1".into()),
            prefix: Some("it".into()),
            ..Default::default()
        };
        let t = ObjectTarget::from_config(&cfg).unwrap();
        t.put(
            "page.html",
            b"<html></html>".to_vec(),
            "text/html; charset=utf-8",
        )
        .await
        .unwrap();
        let (bytes, ct) = t.get_with_content_type("page.html").await.unwrap();
        assert_eq!(bytes, b"<html></html>");
        assert_eq!(ct.as_deref(), Some("text/html; charset=utf-8"));
        t.delete("page.html").await.unwrap();
        assert!(t.get_with_content_type("page.html").await.is_err());
        t.delete("page.html").await.unwrap();
    }
}
