use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SandboxStatus {
    #[serde(rename = "isSandboxed")]
    pub is_sandboxed: bool,
    #[serde(rename = "hasAwsAccess")]
    pub has_aws_access: bool,
}

/// RAII guard for security-scoped AWS directory access.
/// On macOS, holds a CFURL reference and stops accessing on drop.
pub struct AwsDirAccess {
    pub aws_dir_path: PathBuf,
    #[cfg(target_os = "macos")]
    scoped_url: *const std::ffi::c_void,
}

// Safety: CFURL is an immutable, thread-safe CoreFoundation object.
unsafe impl Send for AwsDirAccess {}
unsafe impl Sync for AwsDirAccess {}

impl Drop for AwsDirAccess {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if !self.scoped_url.is_null() {
                unsafe {
                    macos::ffi::CFURLStopAccessingSecurityScopedResource(self.scoped_url);
                    macos::ffi::CFRelease(self.scoped_url);
                }
            }
        }
    }
}

/// Check if the app is running inside the macOS App Sandbox.
pub fn is_sandboxed() -> bool {
    std::env::var("APP_SANDBOX_CONTAINER_ID").is_ok()
}

/// Check if a stored bookmark exists in the plugin store.
#[cfg(feature = "gui")]
pub fn has_stored_bookmark(app_handle: &tauri::AppHandle) -> bool {
    use tauri_plugin_store::StoreExt;
    match app_handle.store("sandbox.json") {
        Ok(store) => store.get("aws_dir_bookmark").is_some(),
        Err(_) => false,
    }
}

/// Grant access to the ~/.aws directory via folder picker.
/// Shows an open-folder dialog, creates a security-scoped bookmark, and stores it.
#[cfg(feature = "gui")]
pub async fn grant_aws_dir_access(app_handle: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    use tauri_plugin_dialog::DialogExt;

    let default_aws_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".aws");

    let (tx, rx) = tokio::sync::oneshot::channel();
    app_handle
        .dialog()
        .file()
        .set_title("Select your ~/.aws directory")
        .set_directory(&default_aws_dir)
        .pick_folder(move |folder_path| {
            let _ = tx.send(folder_path);
        });

    let file_path = rx
        .await
        .map_err(|_| AppError::General("Dialog channel closed".to_string()))?
        .ok_or_else(|| AppError::General("Folder selection cancelled".to_string()))?;

    let path_buf: PathBuf = file_path
        .as_path()
        .ok_or_else(|| AppError::General("Invalid file path from dialog".to_string()))?
        .to_path_buf();

    // Validate: should be named '.aws' or contain a 'config' file
    let name_matches = path_buf
        .file_name()
        .map(|n| n == ".aws")
        .unwrap_or(false);
    let has_config = tokio::fs::try_exists(path_buf.join("config"))
        .await
        .unwrap_or(false);
    let is_aws_dir = name_matches || has_config;

    if !is_aws_dir {
        return Err(AppError::Config(
            "Selected folder doesn't appear to be an AWS directory. Please select ~/.aws/"
                .to_string(),
        ));
    }

    // Create and store security-scoped bookmark (macOS only)
    #[cfg(target_os = "macos")]
    {
        use base64::Engine;
        let bookmark_data = macos::create_bookmark(&path_buf)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bookmark_data);

        use tauri_plugin_store::StoreExt;
        let store = app_handle
            .store("sandbox.json")
            .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;
        store.set("aws_dir_bookmark", serde_json::json!(encoded));
        store
            .save()
            .map_err(|e| AppError::General(format!("Failed to save bookmark: {}", e)))?;
    }

    // On non-macOS, just store the path string
    #[cfg(not(target_os = "macos"))]
    {
        use tauri_plugin_store::StoreExt;
        let store = app_handle
            .store("sandbox.json")
            .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;
        store.set(
            "aws_dir_bookmark",
            serde_json::json!(path_buf.to_string_lossy()),
        );
        store
            .save()
            .map_err(|e| AppError::General(format!("Failed to save bookmark: {}", e)))?;
    }

    Ok(path_buf)
}

/// Activate stored bookmark and start accessing the security-scoped resource.
/// Sets AWS_CONFIG_FILE and AWS_SHARED_CREDENTIALS_FILE env vars.
/// Returns an AwsDirAccess guard that stops access on drop.
#[cfg(feature = "gui")]
pub fn activate_aws_dir_access(app_handle: &tauri::AppHandle) -> Result<AwsDirAccess, AppError> {
    use tauri_plugin_store::StoreExt;

    let store = app_handle
        .store("sandbox.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;
    let encoded = store
        .get("aws_dir_bookmark")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| AppError::General("No stored AWS directory bookmark".to_string()))?;

    #[cfg(target_os = "macos")]
    {
        use base64::Engine;
        let bookmark_data = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .map_err(|e| AppError::General(format!("Failed to decode bookmark: {}", e)))?;

        let (url, path, is_stale) = macos::resolve_bookmark(&bookmark_data)?;

        if is_stale {
            log::warn!("AWS directory bookmark is stale — re-grant required");
            unsafe { macos::ffi::CFRelease(url) };
            return Err(AppError::General(
                "AWS directory bookmark is stale. Please re-grant access to ~/.aws".to_string(),
            ));
        }

        // Start accessing the security-scoped resource
        let ok = unsafe { macos::ffi::CFURLStartAccessingSecurityScopedResource(url) };
        if ok == 0 {
            unsafe {
                macos::ffi::CFRelease(url);
            }
            return Err(AppError::General(
                "Failed to start accessing security-scoped resource".to_string(),
            ));
        }

        // Set env vars so the AWS SDK and our code read from the bookmarked path
        set_aws_env_vars(&path);

        Ok(AwsDirAccess {
            aws_dir_path: path,
            scoped_url: url,
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let path = PathBuf::from(&encoded);
        set_aws_env_vars(&path);

        Ok(AwsDirAccess {
            aws_dir_path: path,
        })
    }
}

/// Set AWS environment variables to point to the given directory.
///
/// # Safety
/// `std::env::set_var` is unsafe in Rust 2024 because environment variables are
/// process-global shared state. This function is called exactly once during
/// single-threaded app setup in `activate_aws_dir_access()`, before Tauri's
/// async runtime spawns worker threads. The `OnceLock` guard ensures it cannot
/// be called more than once.
static AWS_ENV_INITIALIZED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn set_aws_env_vars(aws_dir: &std::path::Path) {
    AWS_ENV_INITIALIZED.get_or_init(|| {
        let config_file = aws_dir.join("config");
        if config_file.exists() {
            // Safety: guaranteed single-threaded by OnceLock + app startup ordering
            unsafe { std::env::set_var("AWS_CONFIG_FILE", &config_file) };
        }
        let credentials_file = aws_dir.join("credentials");
        if credentials_file.exists() {
            // Safety: guaranteed single-threaded by OnceLock + app startup ordering
            unsafe { std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &credentials_file) };
        }
    });
}

/// Get the sandbox status.
#[cfg(feature = "gui")]
pub fn get_sandbox_status(app_handle: &tauri::AppHandle) -> SandboxStatus {
    SandboxStatus {
        is_sandboxed: is_sandboxed(),
        has_aws_access: !is_sandboxed() || has_stored_bookmark(app_handle),
    }
}

// macOS-specific CoreFoundation FFI for security-scoped bookmarks
#[cfg(target_os = "macos")]
mod macos {
    use crate::error::AppError;
    use std::path::PathBuf;

    pub mod ffi {
        use std::ffi::c_void;

        pub type CFURLRef = *const c_void;
        pub type CFDataRef = *const c_void;
        pub type CFErrorRef = *mut c_void;
        pub type CFAllocatorRef = *const c_void;
        pub type CFIndex = isize;
        pub type Boolean = u8;

        pub const K_CF_ALLOCATOR_DEFAULT: CFAllocatorRef = std::ptr::null();
        pub const K_CF_URL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE: usize = 1 << 11;
        pub const K_CF_URL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE: usize = 1 << 10;

        unsafe extern "C" {
            pub fn CFURLCreateFromFileSystemRepresentation(
                allocator: CFAllocatorRef,
                buffer: *const u8,
                buf_len: CFIndex,
                is_directory: Boolean,
            ) -> CFURLRef;

            pub fn CFURLCreateBookmarkData(
                allocator: CFAllocatorRef,
                url: CFURLRef,
                options: usize,
                resource_properties_to_include: *const c_void,
                relative_to_url: CFURLRef,
                error: *mut CFErrorRef,
            ) -> CFDataRef;

            pub fn CFURLCreateByResolvingBookmarkData(
                allocator: CFAllocatorRef,
                bookmark: CFDataRef,
                options: usize,
                relative_to_url: CFURLRef,
                resource_properties_to_exclude: *const c_void,
                is_stale: *mut Boolean,
                error: *mut CFErrorRef,
            ) -> CFURLRef;

            pub fn CFURLStartAccessingSecurityScopedResource(url: CFURLRef) -> Boolean;
            pub fn CFURLStopAccessingSecurityScopedResource(url: CFURLRef);

            pub fn CFDataGetLength(data: CFDataRef) -> CFIndex;
            pub fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
            pub fn CFDataCreate(
                allocator: CFAllocatorRef,
                bytes: *const u8,
                length: CFIndex,
            ) -> CFDataRef;

            pub fn CFRelease(cf: *const c_void);

            pub fn CFURLGetFileSystemRepresentation(
                url: CFURLRef,
                resolve_against_base: Boolean,
                buffer: *mut u8,
                max_buf_len: CFIndex,
            ) -> Boolean;
        }
    }

    /// Create a security-scoped bookmark for a directory path.
    pub fn create_bookmark(path: &std::path::Path) -> Result<Vec<u8>, AppError> {
        let path_bytes = path.as_os_str().as_encoded_bytes();

        unsafe {
            let url = ffi::CFURLCreateFromFileSystemRepresentation(
                ffi::K_CF_ALLOCATOR_DEFAULT,
                path_bytes.as_ptr(),
                path_bytes.len() as ffi::CFIndex,
                1, // is_directory = true
            );

            if url.is_null() {
                return Err(AppError::General(
                    "Failed to create CFURL from path".to_string(),
                ));
            }

            let mut error: ffi::CFErrorRef = std::ptr::null_mut();
            let bookmark_data = ffi::CFURLCreateBookmarkData(
                ffi::K_CF_ALLOCATOR_DEFAULT,
                url,
                ffi::K_CF_URL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE,
                std::ptr::null(),
                std::ptr::null(),
                &mut error,
            );

            ffi::CFRelease(url);

            if bookmark_data.is_null() {
                return Err(AppError::General(
                    "Failed to create security-scoped bookmark".to_string(),
                ));
            }

            let length = ffi::CFDataGetLength(bookmark_data) as usize;
            let ptr = ffi::CFDataGetBytePtr(bookmark_data);
            let bytes = std::slice::from_raw_parts(ptr, length).to_vec();

            ffi::CFRelease(bookmark_data);

            Ok(bytes)
        }
    }

    /// Resolve a security-scoped bookmark.
    /// Returns (CFURLRef, path, is_stale). Caller must manage the CFURLRef lifetime.
    pub fn resolve_bookmark(
        bookmark_bytes: &[u8],
    ) -> Result<(ffi::CFURLRef, PathBuf, bool), AppError> {
        unsafe {
            let cf_data = ffi::CFDataCreate(
                ffi::K_CF_ALLOCATOR_DEFAULT,
                bookmark_bytes.as_ptr(),
                bookmark_bytes.len() as ffi::CFIndex,
            );

            if cf_data.is_null() {
                return Err(AppError::General(
                    "Failed to create CFData from bookmark bytes".to_string(),
                ));
            }

            let mut is_stale: ffi::Boolean = 0;
            let mut error: ffi::CFErrorRef = std::ptr::null_mut();

            let url = ffi::CFURLCreateByResolvingBookmarkData(
                ffi::K_CF_ALLOCATOR_DEFAULT,
                cf_data,
                ffi::K_CF_URL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE,
                std::ptr::null(),
                std::ptr::null(),
                &mut is_stale,
                &mut error,
            );

            ffi::CFRelease(cf_data);

            if url.is_null() {
                return Err(AppError::General(
                    "Failed to resolve bookmark — access may need to be re-granted".to_string(),
                ));
            }

            // Extract file system path from CFURL
            let mut buf = vec![0u8; 4096];
            let ok = ffi::CFURLGetFileSystemRepresentation(
                url,
                1, // resolve against base
                buf.as_mut_ptr(),
                buf.len() as ffi::CFIndex,
            );

            if ok == 0 {
                ffi::CFRelease(url);
                return Err(AppError::General(
                    "Failed to get path from resolved bookmark URL".to_string(),
                ));
            }

            let path_len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let path_str = std::str::from_utf8(&buf[..path_len]).map_err(|_| {
                ffi::CFRelease(url);
                AppError::General("Invalid UTF-8 in resolved bookmark path".to_string())
            })?;

            Ok((url, PathBuf::from(path_str), is_stale != 0))
        }
    }
}
