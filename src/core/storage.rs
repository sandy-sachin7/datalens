use crate::error::DeltaLensError;
use futures::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectStorePath;
use object_store::{ObjectMeta, ObjectStore};
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

fn tokio_runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("Failed to create Tokio runtime for object store"))
}

pub trait StorageBackend: Send + Sync {
    /// List all entries in a directory/subpath.
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError>;
    /// Read entire file contents as bytes.
    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError>;
    /// Check whether a path exists (directory or file).
    fn exists(&self, path: &str) -> Result<bool, DeltaLensError>;
    /// Return a human-readable root string for display.
    fn root_display(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Local filesystem
// ---------------------------------------------------------------------------

pub struct LocalStorage {
    root: String,
}

impl LocalStorage {
    pub fn new(root: String) -> Self {
        Self { root }
    }
}

impl StorageBackend for LocalStorage {
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError> {
        let full_path = if dir_path.is_empty() {
            std::path::PathBuf::from(&self.root)
        } else {
            std::path::Path::new(&self.root).join(dir_path)
        };

        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&full_path)? {
            let entry = entry?;
            let path = entry.path();
            entries.push(path.to_string_lossy().to_string());
        }
        Ok(entries)
    }

    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError> {
        let path = if std::path::Path::new(file_path).is_absolute() {
            std::path::PathBuf::from(file_path)
        } else {
            std::path::Path::new(&self.root).join(file_path)
        };
        Ok(std::fs::read(&path)?)
    }

    fn exists(&self, path: &str) -> Result<bool, DeltaLensError> {
        let full_path = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            std::path::Path::new(&self.root).join(path)
        };
        Ok(full_path.exists())
    }

    fn root_display(&self) -> &str {
        &self.root
    }
}

// ---------------------------------------------------------------------------
// Amazon S3
// ---------------------------------------------------------------------------

pub struct S3Storage {
    client: Arc<dyn ObjectStore>,
    /// Object key prefix (everything after bucket name), e.g. "data/transactions"
    prefix: String,
    /// Full URI for display, e.g. "s3://my-bucket/data/transactions"
    display_uri: String,
}

impl S3Storage {
    pub fn new(bucket: &str, prefix: &str, display_uri: String) -> Result<Self, DeltaLensError> {
        let client = AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .build()
            .map_err(|e| {
                DeltaLensError::Storage(format!(
                    "Failed to create S3 client for bucket '{}': {}\n\
                     \n\
                     Ensure AWS credentials are available via one of:\n\
                       - Environment variables:  \
                     AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY [+ AWS_SESSION_TOKEN]\n\
                       - AWS profile:            \
                     Set AWS_PROFILE=<name>; falls back to 'default'\n\
                       - IAM instance role:      \
                     Works automatically on EC2, ECS, EKS\n\
                     \n\
                     Other optional settings (as env vars):\n\
                       - AWS_REGION              e.g. us-east-1, eu-west-1\n\
                       - AWS_ENDPOINT            Custom S3-compatible endpoint\n\
                       - AWS_S3_ALLOW_HTTP       Set to 'true' for local MinIO dev",
                    bucket, e
                ))
            })?;
        Ok(Self {
            client: Arc::new(client),
            prefix: prefix.trim_end_matches('/').to_string(),
            display_uri,
        })
    }
}

impl S3Storage {
    /// Build the full object key for a subpath.
    fn object_key(&self, subpath: &str) -> String {
        if self.prefix.is_empty() {
            subpath.trim_start_matches('/').to_string()
        } else {
            let p = self.prefix.clone();
            if subpath.is_empty() {
                p
            } else {
                format!("{}/{}", p, subpath.trim_start_matches('/'))
            }
        }
    }
}

impl StorageBackend for S3Storage {
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError> {
        let prefix = self.object_key(dir_path);
        let prefix_path = ObjectStorePath::from(prefix.trim_end_matches('/').to_string() + "/");

        let rt = tokio_runtime();
        let result: Vec<ObjectMeta> = rt
            .block_on(async {
                let mut stream = self.client.list(Some(&prefix_path));
                let mut entries = Vec::new();
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(meta) => entries.push(meta),
                        Err(e) => return Err(e),
                    }
                }
                Ok(entries)
            })
            .map_err(|e| {
                DeltaLensError::Storage(format!("Failed to list S3 path '{}': {}", dir_path, e))
            })?;

        // Return the full s3:// URIs so they can be passed back to read()
        Ok(result
            .into_iter()
            .map(|meta| format!("s3://{}/{}", self.bucket_name(), meta.location))
            .collect())
    }

    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError> {
        // file_path is a full s3:// URI from list() — extract the object key
        let key = file_path
            .strip_prefix("s3://")
            .and_then(|s| s.split_once('/'))
            .map(|(_bucket, key)| key)
            .unwrap_or(file_path);

        let location = ObjectStorePath::from(key.to_string());
        let rt = tokio_runtime();
        let result = rt.block_on(async { self.client.get(&location).await?.bytes().await });

        let bytes = result.map_err(|e| {
            DeltaLensError::Storage(format!("Failed to read S3 object '{}': {}", file_path, e))
        })?;
        Ok(bytes.to_vec())
    }

    fn exists(&self, path: &str) -> Result<bool, DeltaLensError> {
        let key = path
            .strip_prefix("s3://")
            .and_then(|s| s.split_once('/'))
            .map(|(_bucket, key)| key)
            .unwrap_or(path);

        let location = ObjectStorePath::from(format!("{}/", key.trim_end_matches('/')));

        let rt = tokio_runtime();
        let result: Result<Vec<ObjectMeta>, object_store::Error> = rt.block_on(async {
            let mut stream = self.client.list(Some(&location));
            let mut items = Vec::new();
            // Just check if there's at least one item
            if let Some(item) = stream.next().await {
                match item {
                    Ok(meta) => items.push(meta),
                    Err(e) => return Err(e),
                }
            }
            Ok(items)
        });

        match result {
            Ok(items) => Ok(!items.is_empty()),
            Err(e) => Err(DeltaLensError::Storage(format!(
                "Failed to check S3 path '{}': {}",
                path, e
            ))),
        }
    }

    fn root_display(&self) -> &str {
        &self.display_uri
    }
}

impl S3Storage {
    fn bucket_name(&self) -> &str {
        // Extract from display_uri: "s3://bucket/..." -> "bucket"
        self.display_uri
            .strip_prefix("s3://")
            .and_then(|s| s.split('/').next())
            .unwrap_or("unknown")
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Parse a URI and create the appropriate storage backend.
/// Returns `(storage_backend, table_root_path)`.
pub fn storage_for(uri: &str) -> Result<(Box<dyn StorageBackend>, String), DeltaLensError> {
    let uri = uri.trim();

    if uri.starts_with("s3://") {
        let rest = uri.trim_start_matches("s3://");
        let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
        let storage = S3Storage::new(bucket, key, uri.to_string())?;
        let root = uri.trim_end_matches('/').to_string();
        Ok((Box::new(storage), root))
    } else if uri.starts_with("file://") {
        let local_path = uri.trim_start_matches("file://");
        let path = std::path::Path::new(local_path);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(DeltaLensError::Io)?
                .join(path)
        };
        let root = abs.to_string_lossy().to_string();
        Ok((Box::new(LocalStorage::new(root.clone())), root))
    } else {
        // Bare path — treat as local
        let path = std::path::Path::new(uri);
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
}
