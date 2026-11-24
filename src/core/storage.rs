use crate::error::DeltaLensError;
use rayon::prelude::*;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

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

    /// List immediate child entries in a directory/subpath whose filename is
    /// lexicographically greater than `start_after_name` (non-recursive).
    /// Cloud object stores implement this via `list_with_offset` to skip
    /// historical log entries preceding `_last_checkpoint`.
    /// Returns `Ok(None)` when offset listing is not supported by the backend.
    fn list_from_offset(
        &self,
        _dir_path: &str,
        _start_after_name: &str,
    ) -> Result<Option<Vec<String>>, DeltaLensError> {
        Ok(None)
    }

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

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    fn resolve_path(&self, path: &str) -> Result<PathBuf, DeltaLensError> {
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(DeltaLensError::Storage(format!(
                "Path escapes table root: {path}"
            )));
        }

        let canonical_root = match std::fs::canonicalize(&self.root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self.root.join(relative));
            }
            Err(error) => return Err(error.into()),
        };
        let candidate = canonical_root.join(relative);

        // Check every existing prefix. This rejects symlinks that resolve outside
        // the table while still allowing paths whose final file does not exist.
        let mut probe = candidate.as_path();
        loop {
            if let Ok(metadata) = std::fs::symlink_metadata(probe) {
                if metadata.file_type().is_symlink() {
                    let resolved = std::fs::canonicalize(probe)?;
                    if !resolved.starts_with(&canonical_root) {
                        return Err(DeltaLensError::Storage(format!(
                            "Path escapes table root through a symlink: {path}"
                        )));
                    }
                }
            }

            if probe == canonical_root {
                break;
            }
            probe = probe.parent().ok_or_else(|| {
                DeltaLensError::Storage(format!("Path escapes table root: {path}"))
            })?;
        }

        Ok(candidate)
    }
}

impl StorageBackend for LocalStorage {
    fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError> {
        let prefix = dir_path.trim_matches('/');

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let names = read_directory_names(open_table_directory(&self.root, dir_path)?)?;

        #[cfg(windows)]
        let names =
            read_directory_names_windows(open_table_directory_windows(&self.root, dir_path)?)?;

        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let names = std::fs::read_dir(self.resolve_path(dir_path)?)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;

        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            let file_name = name.to_string_lossy().to_string();
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
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let mut file = open_table_file(&self.root, file_path)?;

        #[cfg(windows)]
        let mut file = open_table_file_windows(&self.root, file_path)?;

        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let mut file = std::fs::File::open(self.resolve_path(file_path)?)?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        Ok(contents)
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
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // Use the same no-follow open as `read` so a symlink swapped in
            // after path validation cannot make us report an outside entry.
            match open_table_file(&self.root, path) {
                Ok(_) => Ok(true),
                Err(DeltaLensError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(false)
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(windows)]
        {
            // Mirror Unix O_NOFOLLOW parity using handle-pinned opens verified
            // with GetFinalPathNameByHandleW. Opening a directory without
            // FILE_FLAG_BACKUP_SEMANTICS returns PermissionDenied on Windows, so
            // retry via open_table_directory_windows in that case.
            match open_table_file_windows(&self.root, path) {
                Ok(_) => Ok(true),
                Err(DeltaLensError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(false)
                }
                Err(DeltaLensError::Io(ref e))
                    if e.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    match open_table_directory_windows(&self.root, path) {
                        Ok(_) => Ok(true),
                        Err(DeltaLensError::Io(ref e))
                            if e.kind() == std::io::ErrorKind::NotFound =>
                        {
                            Ok(false)
                        }
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Ok(self.resolve_path(path)?.exists())
        }
    }

    fn is_dir(&self, path: &str) -> Result<bool, DeltaLensError> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            match open_table_directory(&self.root, path) {
                Ok(_) => Ok(true),
                Err(DeltaLensError::Io(ref e))
                    if e.kind() == std::io::ErrorKind::NotFound
                        || e.raw_os_error() == Some(libc::ENOTDIR) =>
                {
                    Ok(false)
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(windows)]
        {
            // FILE_FLAG_BACKUP_SEMANTICS can open regular files as well as
            // directories; query metadata on the already-pinned handle.
            match open_table_directory_windows(&self.root, path) {
                Ok(handle) => Ok(handle.metadata()?.is_dir()),
                Err(DeltaLensError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(false)
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Ok(self.resolve_path(path)?.is_dir())
        }
    }

    fn root_display(&self) -> &str {
        &self.root_display
    }
}

#[cfg(windows)]
fn open_table_file_windows(root: &Path, path: &str) -> Result<std::fs::File, DeltaLensError> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(DeltaLensError::Storage(format!(
            "Path escapes table root: {path}"
        )));
    }

    let root_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
        .open(root)?;
    let file = std::fs::File::open(root.join(relative))?;
    ensure_windows_handle_within_root(&root_handle, &file, path)?;
    Ok(file)
}

#[cfg(windows)]
fn ensure_windows_handle_within_root(
    root_handle: &std::fs::File,
    file_handle: &std::fs::File,
    original_path: &str,
) -> Result<(), DeltaLensError> {
    let root_path = windows_final_path(root_handle)?;
    let file_path = windows_final_path(file_handle)?;
    if !file_path.starts_with(&root_path) {
        return Err(DeltaLensError::Storage(format!(
            "Path escapes table root through a reparse point: {original_path}"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_final_path(file: &std::fs::File) -> Result<PathBuf, DeltaLensError> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };

    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let mut buffer = vec![0u16; 512];
    loop {
        // The open handle pins the object while Windows resolves its final path.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as _,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                flags,
            )
        };
        if length == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if (length as usize) < buffer.len() {
            buffer.truncate(length as usize);
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)));
        }
        buffer.resize(length as usize + 1, 0);
    }
}

#[cfg(windows)]
fn open_table_directory_windows(root: &Path, path: &str) -> Result<std::fs::File, DeltaLensError> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(DeltaLensError::Storage(format!(
            "Path escapes table root: {path}"
        )));
    }

    let root_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
        .open(root)?;
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
        .open(root.join(relative))?;
    ensure_windows_handle_within_root(&root_handle, &directory, path)?;
    Ok(directory)
}

#[cfg(windows)]
fn read_directory_names_windows(directory: std::fs::File) -> Result<Vec<OsString>, DeltaLensError> {
    use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
        FILE_ID_BOTH_DIR_INFO,
    };

    let mut names = Vec::new();
    let mut buffer = vec![0u64; 8192];
    let buffer_bytes = std::mem::size_of_val(buffer.as_slice());
    let mut restart = true;

    loop {
        let info_class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        restart = false;

        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle() as _,
                info_class,
                buffer.as_mut_ptr().cast(),
                buffer_bytes as u32,
            )
        };
        if succeeded == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(error.into());
        }

        let buffer_ptr = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            if offset + std::mem::size_of::<FILE_ID_BOTH_DIR_INFO>() > buffer_bytes {
                return Err(DeltaLensError::Storage(
                    "Invalid directory entry returned by Windows".to_string(),
                ));
            }
            let info = unsafe { &*buffer_ptr.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>() };
            if info.FileNameLength % 2 != 0 {
                return Err(DeltaLensError::Storage(
                    "Invalid directory name returned by Windows".to_string(),
                ));
            }
            let name_len = (info.FileNameLength / 2) as usize;
            let name_ptr = std::ptr::addr_of!(info.FileName).cast::<u16>();
            let name_offset = name_ptr as usize - buffer_ptr as usize;
            if name_len * 2 > buffer_bytes.saturating_sub(name_offset) {
                return Err(DeltaLensError::Storage(
                    "Invalid directory entry returned by Windows".to_string(),
                ));
            }
            let name = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                names.push(OsString::from_wide(name));
            }

            let next_offset = info.NextEntryOffset as usize;
            if next_offset == 0 {
                break;
            }
            if next_offset > buffer_bytes - offset {
                return Err(DeltaLensError::Storage(
                    "Invalid directory entry offset returned by Windows".to_string(),
                ));
            }
            offset += next_offset;
        }
    }

    Ok(names)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn is_symlink_at(dir_fd: libc::c_int, name: &std::ffi::CStr) -> bool {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::fstatat(dir_fd, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    rc == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_table_directory(root: &Path, path: &str) -> Result<std::fs::File, DeltaLensError> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(DeltaLensError::Storage(format!(
            "Path escapes table root: {path}"
        )));
    }

    // Open root once with O_DIRECTORY | O_CLOEXEC to pin it against root-swap TOCTOU,
    // then walk every relative component with openat(O_NOFOLLOW).
    let root_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(root)?;

    open_relative_directory_without_symlinks(root_handle, relative)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_relative_directory_without_symlinks(
    mut current: std::fs::File,
    relative: &Path,
) -> Result<std::fs::File, DeltaLensError> {
    for component in relative.components() {
        let name = match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => name,
            _ => {
                return Err(DeltaLensError::Storage(format!(
                    "Path escapes table root: {}",
                    relative.display()
                )));
            }
        };
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| DeltaLensError::Storage("Path contains a NUL byte".to_string()))?;
        let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY;
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ELOOP)
                || (error.raw_os_error() == Some(libc::ENOTDIR)
                    && is_symlink_at(current.as_raw_fd(), &name))
            {
                return Err(DeltaLensError::Storage(format!(
                    "Path escapes table root through a symlink: {}",
                    relative.display()
                )));
            }
            return Err(error.into());
        }
        current = unsafe { std::fs::File::from_raw_fd(fd) };
    }

    Ok(current)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_directory_names(directory: std::fs::File) -> Result<Vec<OsString>, DeltaLensError> {
    let fd = directory.into_raw_fd();
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(error.into());
    }

    let mut names = Vec::new();
    let read_result = loop {
        clear_errno();
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) != 0 {
                break Err(DeltaLensError::Io(error));
            }
            break Ok(());
        }

        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(OsString::from_vec(name.to_bytes().to_vec()));
        }
    };
    let close_result = unsafe { libc::closedir(stream) };
    read_result?;
    if close_result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(names)
}

#[cfg(target_os = "linux")]
fn clear_errno() {
    unsafe {
        *libc::__errno_location() = 0;
    }
}

#[cfg(target_os = "macos")]
fn clear_errno() {
    unsafe {
        *libc::__error() = 0;
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_table_file(root: &Path, path: &str) -> Result<std::fs::File, DeltaLensError> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(DeltaLensError::Storage(format!(
            "Path escapes table root: {path}"
        )));
    }

    // Open root once with O_DIRECTORY | O_CLOEXEC to pin it against root-swap TOCTOU,
    // then walk every relative component with openat(O_NOFOLLOW).
    let root_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(root)?;

    open_relative_without_symlinks(root_handle, relative)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_relative_without_symlinks(
    mut current: std::fs::File,
    relative: &Path,
) -> Result<std::fs::File, DeltaLensError> {
    let mut normals = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => normals.push(name),
            _ => {
                return Err(DeltaLensError::Storage(format!(
                    "Path escapes table root: {}",
                    relative.display()
                )));
            }
        }
    }

    for (index, name) in normals.iter().enumerate() {
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| DeltaLensError::Storage("Path contains a NUL byte".to_string()))?;
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        if index + 1 < normals.len() {
            flags |= libc::O_DIRECTORY;
        }

        // Each component is opened relative to the prior directory descriptor.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ELOOP)
                || (error.raw_os_error() == Some(libc::ENOTDIR)
                    && is_symlink_at(current.as_raw_fd(), &name))
            {
                return Err(DeltaLensError::Storage(format!(
                    "Path escapes table root through a symlink: {}",
                    relative.display()
                )));
            }
            return Err(error.into());
        }
        current = unsafe { std::fs::File::from_raw_fd(fd) };
    }

    Ok(current)
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
    /// into an object store key. Relative paths ALWAYS prepend `self.prefix` and reject
    /// path traversal (`..`, `.`), NUL bytes, backslashes, leading slashes, and empty segments.
    /// Full URIs must match this backend's scheme and bucket and stay beneath `self.prefix`.
    pub fn resolve_key(&self, path: &str) -> Result<String, DeltaLensError> {
        if path.contains('\0') || path.contains('\\') {
            return Err(DeltaLensError::InvalidInput(format!(
                "Object path '{path}' contains forbidden control or backslash characters"
            )));
        }

        if let Some((scheme, rest)) = split_uri_scheme(path) {
            let (authority, key) = rest.split_once('/').unwrap_or((rest, ""));
            if !scheme.eq_ignore_ascii_case(&self.scheme)
                || !authority.eq_ignore_ascii_case(&self.bucket)
            {
                return Err(DeltaLensError::InvalidInput(format!(
                    "Object URI '{path}' does not match configured bucket '{}://{}'",
                    self.scheme, self.bucket
                )));
            }

            let clean_key = key.trim_start_matches('/').trim_end_matches('/');
            if !clean_key.is_empty()
                && clean_key
                    .split('/')
                    .any(|seg| seg.is_empty() || seg == "." || seg == "..")
            {
                return Err(DeltaLensError::InvalidInput(format!(
                    "Object URI '{path}' contains invalid or parent traversal path segments"
                )));
            }
            if !self.prefix.is_empty() {
                let within_prefix = clean_key == self.prefix
                    || clean_key
                        .strip_prefix(&self.prefix)
                        .is_some_and(|tail| tail.starts_with('/'));
                if !within_prefix {
                    return Err(DeltaLensError::InvalidInput(format!(
                        "Object URI '{path}' escapes configured table prefix '{}'",
                        self.prefix
                    )));
                }
            }
            return Ok(clean_key.to_string());
        }

        if path.is_empty() {
            return Ok(self.prefix.clone());
        }

        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty()
            || trimmed.starts_with('/')
            || trimmed
                .split('/')
                .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        {
            return Err(DeltaLensError::InvalidInput(format!(
                "Relative object path '{path}' contains invalid or parent traversal path segments"
            )));
        }

        Ok(self.object_key(trimmed))
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
        let key = self.resolve_key(dir_path)?;
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

    fn list_from_offset(
        &self,
        dir_path: &str,
        start_after_name: &str,
    ) -> Result<Option<Vec<String>>, DeltaLensError> {
        let key = self.resolve_key(dir_path)?;
        let dir_prefix = key.trim_end_matches('/');
        let prefix_opt = if dir_prefix.is_empty() {
            None
        } else {
            Some(ObjectStorePath::from(format!("{dir_prefix}/")))
        };
        let clean_offset = start_after_name.trim_start_matches('/');
        let offset_key = if dir_prefix.is_empty() {
            clean_offset.to_string()
        } else {
            format!("{dir_prefix}/{clean_offset}")
        };
        let offset = ObjectStorePath::from(offset_key);

        let rt = tokio_runtime()?;
        let metas: Vec<object_store::ObjectMeta> = rt
            .block_on(async {
                self.client
                    .list_with_offset(prefix_opt.as_ref(), &offset)
                    .try_collect()
                    .await
            })
            .map_err(|e| Self::map_store_error("list", dir_path, e))?;

        let immediate_prefix = if dir_prefix.is_empty() {
            String::new()
        } else {
            format!("{dir_prefix}/")
        };

        let entries = metas
            .into_iter()
            .filter_map(|meta| {
                let loc = meta.location.as_ref();
                let rel = if immediate_prefix.is_empty() {
                    loc
                } else {
                    loc.strip_prefix(&immediate_prefix)?
                };
                if rel.is_empty() || rel.contains('/') {
                    return None;
                }
                Some(format!("{}://{}/{}", self.scheme, self.bucket, loc))
            })
            .collect();

        Ok(Some(entries))
    }

    fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError> {
        let key = self.resolve_key(file_path)?;
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
            .map(|p| Ok((p.clone(), ObjectStorePath::from(self.resolve_key(p)?))))
            .collect::<Result<Vec<_>, DeltaLensError>>()?;

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
        let key = self.resolve_key(path)?;
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
        let key = self.resolve_key(path)?;
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
    fn test_local_storage_rejects_parent_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(tmp.path().join("outside.json"), b"secret").unwrap();

        let storage = LocalStorage::new(root.to_string_lossy().to_string());
        assert!(matches!(
            storage.read("../outside.json"),
            Err(DeltaLensError::Storage(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_local_storage_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        let sidecars = root.join("_delta_log/_sidecars");
        std::fs::create_dir_all(&sidecars).unwrap();
        let outside = tmp.path().join("outside.parquet");
        std::fs::write(&outside, b"outside table").unwrap();
        symlink(&outside, sidecars.join("outside.parquet")).unwrap();

        let storage = LocalStorage::new(root.to_string_lossy().to_string());
        assert!(matches!(
            storage.read("_delta_log/_sidecars/outside.parquet"),
            Err(DeltaLensError::Storage(_))
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn test_exists_and_is_dir_reject_symlink_escape() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("link")).unwrap();

        let storage = LocalStorage::new(root.to_string_lossy().to_string());
        assert!(matches!(
            storage.exists("link"),
            Err(DeltaLensError::Storage(_))
        ));
        assert!(matches!(
            storage.is_dir("link"),
            Err(DeltaLensError::Storage(_))
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn test_descriptor_relative_read_rejects_symlink_swap() {
        use std::os::unix::fs::symlink;
        use std::os::unix::fs::OpenOptionsExt;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        let safe_dir = root.join("data");
        let outside_dir = tmp.path().join("outside");
        std::fs::create_dir_all(&safe_dir).unwrap();
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(safe_dir.join("file.parquet"), b"inside").unwrap();
        std::fs::write(outside_dir.join("file.parquet"), b"outside").unwrap();

        let root_handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&root)
            .unwrap();
        let resolved_target = Path::new("data/file.parquet");

        std::fs::rename(&safe_dir, root.join("data-original")).unwrap();
        symlink(&outside_dir, &safe_dir).unwrap();

        assert!(open_relative_without_symlinks(root_handle, resolved_target).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn test_descriptor_relative_list_uses_pinned_directory() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        let log_dir = root.join("_delta_log");
        let replacement_dir = tmp.path().join("replacement");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::create_dir(&replacement_dir).unwrap();
        std::fs::write(log_dir.join("inside.json"), b"inside").unwrap();
        std::fs::write(replacement_dir.join("outside.json"), b"outside").unwrap();

        let directory = open_table_directory(&root, "_delta_log").unwrap();
        std::fs::rename(&log_dir, root.join("_delta_log-old")).unwrap();
        symlink(&replacement_dir, &log_dir).unwrap();

        let names = read_directory_names(directory).unwrap();
        assert!(names.iter().any(|name| name == "inside.json"));
        assert!(!names.iter().any(|name| name == "outside.json"));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_handle_path_check_rejects_external_file() {
        use std::os::windows::fs::OpenOptionsExt;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        std::fs::create_dir(&root).unwrap();
        let inside = root.join("inside.json");
        let outside = tmp.path().join("outside.json");
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();

        let storage = LocalStorage::new(root.to_string_lossy().to_string());
        assert_eq!(storage.read("inside.json").unwrap(), b"inside");

        let root_handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
            .open(&root)
            .unwrap();
        let outside_handle = std::fs::File::open(outside).unwrap();
        assert!(matches!(
            ensure_windows_handle_within_root(&root_handle, &outside_handle, "outside.json"),
            Err(DeltaLensError::Storage(message))
                if message.contains("Path escapes table root")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_directory_listing_uses_pinned_handle() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        let log_dir = root.join("_delta_log");
        let replacement_dir = tmp.path().join("replacement");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::create_dir(&replacement_dir).unwrap();
        std::fs::write(log_dir.join("inside.json"), b"inside").unwrap();
        std::fs::write(replacement_dir.join("outside.json"), b"outside").unwrap();

        let directory = open_table_directory_windows(&root, "_delta_log").unwrap();
        std::fs::rename(&log_dir, root.join("_delta_log-old")).unwrap();
        std::fs::rename(&replacement_dir, &log_dir).unwrap();

        let names = read_directory_names_windows(directory).unwrap();
        assert!(names.iter().any(|name| name == "inside.json"));
        assert!(!names.iter().any(|name| name == "outside.json"));
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
            backend.resolve_key("_delta_log").unwrap(),
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

        // Full URI read() works identically when beneath prefix
        let full_bytes = backend.read(&listed[0]).unwrap();
        assert_eq!(rel_bytes, full_bytes);

        // Full URI escaping `self.prefix` (including sibling prefix) MUST be rejected!
        assert!(matches!(
            backend
                .resolve_key("s3://my-bucket/warehouse/other/_delta_log/00000000000000000000.json"),
            Err(DeltaLensError::InvalidInput(_))
        ));
        assert!(matches!(
            backend.resolve_key(
                "s3://my-bucket/warehouse/sales_other/_delta_log/00000000000000000000.json"
            ),
            Err(DeltaLensError::InvalidInput(_))
        ));
        assert!(matches!(
            backend.read("s3://my-bucket/secret/file.json"),
            Err(DeltaLensError::InvalidInput(_))
        ));

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

    #[cfg(feature = "cloud")]
    #[test]
    fn test_object_store_resolve_key_rejects_relative_traversal() {
        use object_store::memory::InMemory;

        let backend = ObjectStoreBackend::from_object_store(
            Arc::new(InMemory::new()),
            "s3",
            "my-bucket",
            "warehouse/sales",
            "s3://my-bucket/warehouse/sales".to_string(),
        );

        // Valid relative paths stay under `self.prefix`.
        assert_eq!(backend.resolve_key("").unwrap(), "warehouse/sales");
        assert_eq!(
            backend.resolve_key("_delta_log").unwrap(),
            "warehouse/sales/_delta_log"
        );
        assert_eq!(
            backend.resolve_key("_delta_log/").unwrap(),
            "warehouse/sales/_delta_log"
        );
        assert_eq!(
            backend
                .resolve_key("_delta_log/00000000000000000000.json")
                .unwrap(),
            "warehouse/sales/_delta_log/00000000000000000000.json"
        );

        // Parent traversal, dot segments, empty segments, leading slashes, backslashes, NUL bytes,
        // and mismatched URI schemes/buckets MUST all be rejected.
        for bad in [
            "..",
            "../other/secret.json",
            "_delta_log/../../secret.json",
            "_delta_log/../_delta_log/00000000000000000000.json",
            "_delta_log/./00000000000000000000.json",
            "_delta_log//00000000000000000000.json",
            "/",
            "/_delta_log/00000000000000000000.json",
            "_delta_log\\..\\secret.json",
            "_delta_log/00000000000000000000.json\0",
            "s3://other-bucket/warehouse/sales/_delta_log/00000000000000000000.json",
            "gs://my-bucket/warehouse/sales/_delta_log/00000000000000000000.json",
            "s3://my-bucket/warehouse/sales/_delta_log/../secret.json",
            "s3://my-bucket/warehouse/sales/_delta_log//secret.json",
        ] {
            assert!(
                matches!(
                    backend.resolve_key(bad),
                    Err(DeltaLensError::InvalidInput(_))
                ),
                "Expected InvalidInput for path: {bad:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_exists_and_is_dir_handle_pinned() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("table");
        let log_dir = root.join("_delta_log");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("00000000000000000000.json"), b"{}\n").unwrap();
        std::fs::write(tmp.path().join("outside.json"), b"secret").unwrap();

        let storage = LocalStorage::new(root.to_string_lossy().to_string());
        assert!(storage.exists("_delta_log").unwrap());
        assert!(storage.is_dir("_delta_log").unwrap());
        assert!(storage
            .exists("_delta_log/00000000000000000000.json")
            .unwrap());
        assert!(!storage
            .is_dir("_delta_log/00000000000000000000.json")
            .unwrap());
        assert!(!storage.exists("_delta_log/missing.json").unwrap());
        assert!(!storage.is_dir("_delta_log/missing").unwrap());
        assert!(matches!(
            storage.exists("../outside.json"),
            Err(DeltaLensError::Storage(_))
        ));
        assert!(matches!(
            storage.is_dir("../outside.json"),
            Err(DeltaLensError::Storage(_))
        ));
    }
}
