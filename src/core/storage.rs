use crate::error::DeltaLensError;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[cfg(feature = "cloud")]
use futures::stream::{self, StreamExt, TryStreamExt};
#[cfg(feature = "cloud")]
use object_store::path::Path as ObjectStorePath;
#[cfg(feature = "cloud")]
use object_store::ObjectStore;
#[cfg(feature = "cloud")]
use std::sync::{Arc, OnceLock};
#[cfg(feature = "cloud")]
use tokio::runtime::Runtime;

#[cfg(feature = "cloud")]
const DEFAULT_CLOUD_CONCURRENCY: usize = 32;

#[cfg(feature = "cloud")]
fn tokio_runtime() -> Result<&'static Runtime, DeltaLensError> {
    static RT: OnceLock<Result<Runtime, String>> = OnceLock::new();
    let res = RT.get_or_init(|| {
        Runtime::new().map_err(|e| format!("Failed to create Tokio runtime for object store: {e}"))
    });
    match res {
        Ok(rt) => Ok(rt),
        Err(msg) => Err(DeltaLensError::Storage(msg.clone())),
    }
}

pub trait StorageBackend: Send + Sync {
    /// List immediate child entries in a directory/subpath (non-recursive).
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError>;

    /// Read entire file contents as bytes.
    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError>;

    /// Read multiple files concurrently using backend-appropriate parallelism
    /// (bounded Tokio async stream for cloud stores, Rayon for local disk).
    fn read_many(&self, file_paths: &[String]) -> Result<Vec<(String, Vec<u8>)>, DeltaLensError> {
        file_paths
            .iter()
            .map(|p| Ok((p.clone(), self.read(p)?)))
            .collect()
    }

    /// Check whether a file or directory path exists.
    fn exists(&self, path: &str) -> Result<bool, DeltaLensError>;

    /// Check whether a path exists and is a directory (or non-empty object prefix).
    fn is_dir(&self, path: &str) -> Result<bool, DeltaLensError> {
        self.exists(path)
    }

    /// Return a human-readable root string for display.
    fn root_display(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Local filesystem
// ---------------------------------------------------------------------------

pub struct LocalStorage {
    root: PathBuf,
    root_display: String,
}

impl LocalStorage {
    pub fn new(root: String) -> Self {
        let root_path = PathBuf::from(&root);
        Self {
            root: root_path,
            root_display: root,
        }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        if path.is_empty() {
            return self.root.clone();
        }
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }
}

impl StorageBackend for LocalStorage {
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError> {
        let full_path = self.resolve_path(dir_path);
        let prefix = dir_path.trim_matches('/');

        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&full_path)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            // Always return paths relative to `self.root` so subsequent `read()`
            // never double-joins relative roots (e.g., `rel/dir` + `rel/dir/_delta_log/...`).
            let rel = if prefix.is_empty() {
                file_name
            } else {
                format!("{}/{}", prefix, file_name)
            };
            entries.push(rel);
        }
        Ok(entries)
    }

    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError> {
        let path = self.resolve_path(file_path);
        Ok(std::fs::read(&path)?)
    }

    fn read_many(&self, file_paths: &[String]) -> Result<Vec<(String, Vec<u8>)>, DeltaLensError> {
        file_paths
            .par_iter()
            .map(|p| {
                let bytes = self.read(p)?;
                Ok((p.clone(), bytes))
            })
            .collect()
    }

    fn exists(&self, path: &str) -> Result<bool, DeltaLensError> {
        Ok(self.resolve_path(path).exists())
    }

    fn is_dir(&self, path: &str) -> Result<bool, DeltaLensError> {
        Ok(self.resolve_path(path).is_dir())
    }

    fn root_display(&self) -> &str {
        &self.root_display
    }
}

// ---------------------------------------------------------------------------
// Cloud Object Storage (S3, GCS, Azure) via `object_store`
// ---------------------------------------------------------------------------

#[cfg(feature = "cloud")]
pub struct ObjectStoreBackend {
    client: Arc<dyn ObjectStore>,
    scheme: String,
    bucket: String,
    /// Object key prefix (everything after bucket/container name), e.g. "data/transactions"
    prefix: String,
    /// Full URI for display, e.g. "s3://my-bucket/data/transactions"
    display_uri: String,
    concurrency: usize,
}

/// Public alias preserving S3Storage naming while sharing the hardened cloud backend.
#[cfg(feature = "cloud")]
pub type S3Storage = ObjectStoreBackend;

#[cfg(feature = "cloud")]
impl ObjectStoreBackend {
    /// Construct from an arbitrary `ObjectStore` implementation (also used in tests with `InMemory`).
    pub fn from_object_store(
        client: Arc<dyn ObjectStore>,
        scheme: &str,
        bucket: &str,
        prefix: &str,
        display_uri: String,
    ) -> Self {
        Self {
            client,
            scheme: scheme.to_string(),
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
            display_uri,
            concurrency: DEFAULT_CLOUD_CONCURRENCY,
        }
    }

    #[cfg(feature = "aws")]
    pub fn new_s3(
        scheme: &str,
        bucket: &str,
        prefix: &str,
        display_uri: String,
    ) -> Result<Self, DeltaLensError> {
        use object_store::aws::AmazonS3Builder;
        let client = AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .build()
            .map_err(|e| {
                DeltaLensError::Auth(format!(
                    "Failed to create S3 client for bucket '{bucket}': {e}\n\
                     \n\
                     Ensure AWS credentials are available via one of:\n\
                       - Environment variables:  AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY [+ AWS_SESSION_TOKEN]\n\
                       - IAM instance/pod role:  Works automatically on EC2, ECS, EKS\n\
                       - WebIdentity / OIDC:     AWS_WEB_IDENTITY_TOKEN_FILE + AWS_ROLE_ARN\n\
                       (Note: object_store reads env/IMDS directly; export credentials from AWS CLI profiles if needed.)\n\
                     \n\
                     Other optional settings (as env vars):\n\
                       - AWS_REGION              e.g. us-east-1, eu-west-1\n\
                       - AWS_ENDPOINT            Custom S3-compatible endpoint (MinIO, R2, LocalStack)\n\
                       - AWS_ALLOW_HTTP          Set to 'true' for local HTTP endpoints (e.g. MinIO)"
                ))
            })?;
        Ok(Self::from_object_store(
            Arc::new(client),
            scheme,
            bucket,
            prefix,
            display_uri,
        ))
    }

    #[cfg(feature = "gcp")]
    pub fn new_gcs(
        bucket: &str,
        prefix: &str,
        display_uri: String,
    ) -> Result<Self, DeltaLensError> {
        use object_store::gcp::GoogleCloudStorageBuilder;
        let client = GoogleCloudStorageBuilder::from_env()
            .with_bucket_name(bucket)
            .build()
            .map_err(|e| {
                DeltaLensError::Auth(format!(
                    "Failed to create GCS client for bucket '{bucket}': {e}\n\
                     \n\
                     Ensure GCP credentials are available via:\n\
                       - GOOGLE_SERVICE_ACCOUNT or GOOGLE_APPLICATION_CREDENTIALS\n\
                       - GCE/GKE metadata service (automatic on GCP compute)"
                ))
            })?;
        Ok(Self::from_object_store(
            Arc::new(client),
            "gs",
            bucket,
            prefix,
            display_uri,
        ))
    }

    #[cfg(feature = "azure")]
    pub fn new_azure(
        scheme: &str,
        container: &str,
        account: Option<&str>,
        prefix: &str,
        display_uri: String,
    ) -> Result<Self, DeltaLensError> {
        use object_store::azure::MicrosoftAzureBuilder;
        let mut builder = MicrosoftAzureBuilder::from_env().with_container_name(container);
        if let Some(acct) = account {
            builder = builder.with_account(acct);
        }
        let client = builder.build().map_err(|e| {
            DeltaLensError::Auth(format!(
                "Failed to create Azure client for container '{container}': {e}\n\
                 \n\
                 Ensure Azure credentials are available via:\n\
                   - AZURE_STORAGE_ACCOUNT_NAME + AZURE_STORAGE_ACCOUNT_KEY (or AZURE_STORAGE_SAS_KEY)\n\
                   - Service principal: AZURE_CLIENT_ID + AZURE_CLIENT_SECRET + AZURE_TENANT_ID\n\
                   - Managed Identity / Azure CLI token"
            ))
        })?;
        Ok(Self::from_object_store(
            Arc::new(client),
            scheme,
            container,
            prefix,
            display_uri,
        ))
    }

    /// Build the full object key for a relative subpath beneath `self.prefix`.
    pub fn object_key(&self, subpath: &str) -> String {
        let clean = subpath.trim_start_matches('/');
        if self.prefix.is_empty() {
            clean.to_string()
        } else if clean.is_empty() {
            self.prefix.clone()
        } else {
            format!("{}/{}", self.prefix, clean)
        }
    }

    /// Resolve either a full URI (`scheme://bucket/key`) or a relative subpath (`_delta_log/...`)
    /// into an object store key. Relative paths ALWAYS prepend `self.prefix`.
    pub fn resolve_key(&self, path: &str) -> String {
        if let Some((_scheme, rest)) = split_uri_scheme(path) {
            if let Some((_authority, key)) = rest.split_once('/') {
                return key.trim_start_matches('/').to_string();
            }
            return String::new();
        }
        self.object_key(path)
    }

    fn map_store_error(action: &str, target: &str, err: object_store::Error) -> DeltaLensError {
        let msg = format!("Failed to {action} object '{target}': {err}");
        match err {
            object_store::Error::Unauthenticated { .. }
            | object_store::Error::PermissionDenied { .. } => DeltaLensError::Auth(msg),
            _ => {
                let lower = msg.to_lowercase();
                if lower.contains("401")
                    || lower.contains("403")
                    || lower.contains("access denied")
                    || lower.contains("invalidaccesskeyid")
                    || lower.contains("signaturedoesnotmatch")
                    || lower.contains("expiredtoken")
                    || lower.contains("credentials")
                {
                    DeltaLensError::Auth(msg)
                } else {
                    DeltaLensError::Storage(msg)
                }
            }
        }
    }
}

#[cfg(feature = "cloud")]
impl StorageBackend for ObjectStoreBackend {
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError> {
        let key = self.resolve_key(dir_path);
        let prefix_opt = if key.is_empty() {
            None
        } else {
            Some(ObjectStorePath::from(format!(
                "{}/",
                key.trim_end_matches('/')
            )))
        };

        let rt = tokio_runtime()?;
        let list_result = rt
            .block_on(async { self.client.list_with_delimiter(prefix_opt.as_ref()).await })
            .map_err(|e| Self::map_store_error("list", dir_path, e))?;

        Ok(list_result
            .objects
            .into_iter()
            .map(|meta| format!("{}://{}/{}", self.scheme, self.bucket, meta.location))
            .collect())
    }

    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError> {
        let key = self.resolve_key(file_path);
        let location = ObjectStorePath::from(key);
        let rt = tokio_runtime()?;
        let bytes = rt
            .block_on(async { self.client.get(&location).await?.bytes().await })
            .map_err(|e| Self::map_store_error("read", file_path, e))?;
        Ok(bytes.to_vec())
    }

    fn read_many(&self, file_paths: &[String]) -> Result<Vec<(String, Vec<u8>)>, DeltaLensError> {
        if file_paths.is_empty() {
            return Ok(Vec::new());
        }
        let rt = tokio_runtime()?;
        let client = Arc::clone(&self.client);
        let concurrency = self.concurrency.max(1);
        let resolved: Vec<(String, ObjectStorePath)> = file_paths
            .iter()
            .map(|p| (p.clone(), ObjectStorePath::from(self.resolve_key(p))))
            .collect();

        rt.block_on(async move {
            stream::iter(resolved)
                .map(|(orig_path, location)| {
                    let client = Arc::clone(&client);
                    async move {
                        let bytes = client
                            .get(&location)
                            .await
                            .map_err(|e| Self::map_store_error("read", &orig_path, e))?
                            .bytes()
                            .await
                            .map_err(|e| Self::map_store_error("read", &orig_path, e))?;
                        Ok::<(String, Vec<u8>), DeltaLensError>((orig_path, bytes.to_vec()))
                    }
                })
                .buffer_unordered(concurrency)
                .try_collect()
                .await
        })
    }

    fn exists(&self, path: &str) -> Result<bool, DeltaLensError> {
        let key = self.resolve_key(path);
        if key.is_empty() {
            return Ok(true);
        }
        let file_loc = ObjectStorePath::from(key.clone());
        let dir_loc = ObjectStorePath::from(format!("{}/", key.trim_end_matches('/')));
        let rt = tokio_runtime()?;

        rt.block_on(async {
            match self.client.head(&file_loc).await {
                Ok(_) => Ok(true),
                Err(object_store::Error::NotFound { .. }) => {
                    let list = self
                        .client
                        .list_with_delimiter(Some(&dir_loc))
                        .await
                        .map_err(|e| Self::map_store_error("check", path, e))?;
                    Ok(!list.objects.is_empty() || !list.common_prefixes.is_empty())
                }
                Err(e) => Err(Self::map_store_error("check", path, e)),
            }
        })
    }

    fn is_dir(&self, path: &str) -> Result<bool, DeltaLensError> {
        let key = self.resolve_key(path);
        let prefix_opt = if key.is_empty() {
            None
        } else {
            Some(ObjectStorePath::from(format!(
                "{}/",
                key.trim_end_matches('/')
            )))
        };
        let rt = tokio_runtime()?;
        let list = rt
            .block_on(async { self.client.list_with_delimiter(prefix_opt.as_ref()).await })
            .map_err(|e| Self::map_store_error("check directory", path, e))?;
        Ok(!list.objects.is_empty() || !list.common_prefixes.is_empty())
    }

    fn root_display(&self) -> &str {
        &self.display_uri
    }
}

// ---------------------------------------------------------------------------
// URI parsing & Factory
// ---------------------------------------------------------------------------

/// Split `scheme://rest` if `uri` begins with a valid RFC 3986 scheme.
fn split_uri_scheme(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = uri.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let valid = scheme.bytes().enumerate().all(|(i, b)| {
        b.is_ascii_alphabetic()
            || (i > 0 && (b.is_ascii_digit() || b == b'+' || b == b'-' || b == b'.'))
    });
    if valid {
        Some((scheme, rest))
    } else {
        None
    }
}

/// Parse an Azure authority (`container@account.dfs.core.windows.net` or `container`)
/// and prefix.
#[cfg(feature = "azure")]
fn parse_azure_uri(rest: &str) -> Result<(String, Option<String>, String), DeltaLensError> {
    let (authority, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return Err(DeltaLensError::InvalidInput(
            "Azure URI is missing container name".to_string(),
        ));
    }
    if let Some((container, host)) = authority.split_once('@') {
        let account = host
            .split('.')
            .next()
            .filter(|s| !s.is_empty())
            .map(String::from);
        if container.is_empty() {
            return Err(DeltaLensError::InvalidInput(
                "Azure URI is missing container name before '@'".to_string(),
            ));
        }
        Ok((container.to_string(), account, prefix.to_string()))
    } else {
        Ok((authority.to_string(), None, prefix.to_string()))
    }
}

fn resolve_local_root(
    local_path: &str,
) -> Result<(Box<dyn StorageBackend>, String), DeltaLensError> {
    let path = Path::new(local_path);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(DeltaLensError::Io)?
            .join(path)
    };
    let root = abs.to_string_lossy().to_string();
    Ok((Box::new(LocalStorage::new(root.clone())), root))
}

/// Parse a URI and create the appropriate storage backend.
/// Returns `(storage_backend, table_root_path)`.
pub fn storage_for(uri: &str) -> Result<(Box<dyn StorageBackend>, String), DeltaLensError> {
    let uri = uri.trim();
    if uri.is_empty() {
        return Err(DeltaLensError::InvalidInput(
            "Table path cannot be empty".to_string(),
        ));
    }

    if let Some((scheme_raw, rest)) = split_uri_scheme(uri) {
        let scheme = scheme_raw.to_ascii_lowercase();
        match scheme.as_str() {
            "file" => resolve_local_root(rest),
            "s3" | "s3a" | "s3n" => {
                #[cfg(feature = "aws")]
                {
                    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
                    if bucket.is_empty() {
                        return Err(DeltaLensError::InvalidInput(format!(
                            "S3 URI '{uri}' is missing bucket name"
                        )));
                    }
                    let root = uri.trim_end_matches('/').to_string();
                    let storage = ObjectStoreBackend::new_s3(&scheme, bucket, prefix, root.clone())?;
                    Ok((Box::new(storage), root))
                }
                #[cfg(not(feature = "aws"))]
                {
                    let _ = rest;
                    Err(DeltaLensError::UnsupportedScheme(format!(
                        "{scheme}:// (rebuild deltalens with the 'aws' feature enabled)"
                    )))
                }
            }
            "gs" => {
                #[cfg(feature = "gcp")]
                {
                    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
                    if bucket.is_empty() {
                        return Err(DeltaLensError::InvalidInput(format!(
                            "GCS URI '{uri}' is missing bucket name"
                        )));
                    }
                    let root = uri.trim_end_matches('/').to_string();
                    let storage = ObjectStoreBackend::new_gcs(bucket, prefix, root.clone())?;
                    Ok((Box::new(storage), root))
                }
                #[cfg(not(feature = "gcp"))]
                {
                    let _ = rest;
                    Err(DeltaLensError::UnsupportedScheme(format!(
                        "gs:// (rebuild deltalens with the 'gcp' feature enabled)"
                    )))
                }
            }
            "az" | "abfs" | "abfss" => {
                #[cfg(feature = "azure")]
                {
                    let (container, account, prefix) = parse_azure_uri(rest)?;
                    let root = uri.trim_end_matches('/').to_string();
                    let storage = ObjectStoreBackend::new_azure(
                        &scheme,
                        &container,
                        account.as_deref(),
                        &prefix,
                        root.clone(),
                    )?;
                    Ok((Box::new(storage), root))
                }
                #[cfg(not(feature = "azure"))]
                {
                    let _ = rest;
                    Err(DeltaLensError::UnsupportedScheme(format!(
                        "{scheme}:// (rebuild deltalens with the 'azure' feature enabled)"
                    )))
                }
            }
            other => Err(DeltaLensError::UnsupportedScheme(format!(
                "{other}:// — supported schemes: file://, s3://, s3a://, s3n://, gs://, az://, abfs://, abfss://"
            ))),
        }
    } else {
        resolve_local_root(uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_local_storage_relative_root_no_double_join() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();
        std::fs::write(log_dir.join("00000000000000000000.json"), b"{}\n").unwrap();

        let rel_storage = LocalStorage::new(tmp.path().to_string_lossy().to_string());
        assert!(rel_storage.is_dir("_delta_log").unwrap());
        let listed = rel_storage.list("_delta_log").unwrap();
        assert_eq!(listed, vec!["_delta_log/00000000000000000000.json"]);
        let bytes = rel_storage.read(&listed[0]).unwrap();
        assert_eq!(bytes, b"{}\n");
    }

    #[test]
    fn test_local_storage_rejects_file_as_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("_delta_log"), b"not a directory").unwrap();
        let storage = LocalStorage::new(tmp.path().to_string_lossy().to_string());
        assert!(storage.exists("_delta_log").unwrap());
        assert!(!storage.is_dir("_delta_log").unwrap());
    }

    #[test]
    fn test_storage_for_rejects_unknown_scheme() {
        let res = storage_for("hdfs://namenode:8020/data/table");
        assert!(matches!(res, Err(DeltaLensError::UnsupportedScheme(_))));

        let res2 = storage_for("http://example.com/table");
        assert!(matches!(res2, Err(DeltaLensError::UnsupportedScheme(_))));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn test_object_store_prefix_resolution_exists_read_list_read_many() {
        use bytes::Bytes;
        use object_store::memory::InMemory;

        let mem = Arc::new(InMemory::new());
        let rt = tokio_runtime().unwrap();
        rt.block_on(async {
            mem.put(
                &ObjectStorePath::from("warehouse/sales/_delta_log/00000000000000000000.json"),
                Bytes::from_static(b"{\"commitInfo\":{\"timestamp\":1,\"operation\":\"WRITE\"}}\n")
                    .into(),
            )
            .await
            .unwrap();
            mem.put(
                &ObjectStorePath::from("warehouse/sales/_delta_log/_sidecars/sidecar1.parquet"),
                Bytes::from_static(b"sidecar-bytes").into(),
            )
            .await
            .unwrap();
        });

        let backend = ObjectStoreBackend::from_object_store(
            mem,
            "s3",
            "my-bucket",
            "warehouse/sales",
            "s3://my-bucket/warehouse/sales".to_string(),
        );

        // Prefix MUST be prepended for relative paths in exists() and read()
        assert_eq!(
            backend.resolve_key("_delta_log"),
            "warehouse/sales/_delta_log"
        );
        assert!(backend.exists("_delta_log").unwrap());
        assert!(backend.is_dir("_delta_log").unwrap());
        assert!(!backend.exists("_nonexistent").unwrap());

        // Non-recursive list_with_delimiter should NOT return _sidecars/sidecar1.parquet
        let listed = backend.list("_delta_log").unwrap();
        assert_eq!(
            listed,
            vec!["s3://my-bucket/warehouse/sales/_delta_log/00000000000000000000.json"]
        );

        // Relative read() prepends prefix
        let rel_bytes = backend
            .read("_delta_log/00000000000000000000.json")
            .unwrap();
        assert!(rel_bytes.starts_with(b"{\"commitInfo\""));

        // Full URI read() works identically
        let full_bytes = backend.read(&listed[0]).unwrap();
        assert_eq!(rel_bytes, full_bytes);

        // Relative nested read (_sidecars) works
        let sidecar = backend
            .read("_delta_log/_sidecars/sidecar1.parquet")
            .unwrap();
        assert_eq!(sidecar, b"sidecar-bytes");

        // read_many reads concurrently via Tokio without panics
        let batch = backend
            .read_many(&[
                "_delta_log/00000000000000000000.json".to_string(),
                "_delta_log/_sidecars/sidecar1.parquet".to_string(),
            ])
            .unwrap();
        assert_eq!(batch.len(), 2);
    }
}
