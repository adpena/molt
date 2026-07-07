//! VFS loading and host-injected bundle custody.

use crate::{MountTable, VfsError, VfsState, bundle, dev, tmp};
use std::sync::{Arc, Mutex};

const VFS_BUNDLE_DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
const VFS_BUNDLE_DEFAULT_MAX_ENTRIES: usize = 100_000;
const VFS_BUNDLE_DEFAULT_MAX_PATH_BYTES: usize = 16 * 1024 * 1024;
const VFS_BUNDLE_MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const VFS_BUNDLE_MAX_PATH_BYTES: usize = 4096;
const VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS: &str = "MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS";

#[derive(Clone, Debug)]
pub(crate) struct VfsLoadQuota {
    max_total_bytes: usize,
    max_entries: usize,
    max_path_bytes: usize,
    max_entry_bytes: usize,
    total_bytes: usize,
    entries: usize,
    path_bytes: usize,
}

impl VfsLoadQuota {
    pub(crate) fn from_env() -> Self {
        Self {
            max_total_bytes: env_usize("MOLT_VFS_BUNDLE_MAX_BYTES", VFS_BUNDLE_DEFAULT_MAX_BYTES),
            max_entries: env_usize(
                "MOLT_VFS_BUNDLE_MAX_ENTRIES",
                VFS_BUNDLE_DEFAULT_MAX_ENTRIES,
            ),
            max_path_bytes: env_usize(
                "MOLT_VFS_BUNDLE_MAX_PATH_BYTES",
                VFS_BUNDLE_DEFAULT_MAX_PATH_BYTES,
            ),
            max_entry_bytes: env_usize(
                "MOLT_VFS_BUNDLE_MAX_ENTRY_BYTES",
                VFS_BUNDLE_MAX_ENTRY_BYTES,
            ),
            total_bytes: 0,
            entries: 0,
            path_bytes: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        max_total_bytes: usize,
        max_entries: usize,
        max_path_bytes: usize,
    ) -> Self {
        Self {
            max_total_bytes,
            max_entries,
            max_path_bytes,
            max_entry_bytes: max_total_bytes.max(1),
            total_bytes: 0,
            entries: 0,
            path_bytes: 0,
        }
    }

    pub(crate) fn reserve_entry(&mut self, path: &str, data_len: usize) -> Result<(), VfsError> {
        if path.len() > VFS_BUNDLE_MAX_PATH_BYTES || data_len > self.max_entry_bytes {
            return Err(VfsError::QuotaExceeded);
        }
        let next_entries = self.entries.checked_add(1).ok_or(VfsError::QuotaExceeded)?;
        let next_path_bytes = self
            .path_bytes
            .checked_add(path.len())
            .ok_or(VfsError::QuotaExceeded)?;
        let next_total_bytes = self
            .total_bytes
            .checked_add(data_len)
            .ok_or(VfsError::QuotaExceeded)?;
        if next_entries > self.max_entries
            || next_path_bytes > self.max_path_bytes
            || next_total_bytes > self.max_total_bytes
        {
            return Err(VfsError::QuotaExceeded);
        }
        self.entries = next_entries;
        self.path_bytes = next_path_bytes;
        self.total_bytes = next_total_bytes;
        Ok(())
    }

    pub(crate) fn reserve_additional_bytes(&mut self, bytes: usize) -> Result<(), VfsError> {
        let next_total_bytes = self
            .total_bytes
            .checked_add(bytes)
            .ok_or(VfsError::QuotaExceeded)?;
        if next_total_bytes > self.max_total_bytes {
            return Err(VfsError::QuotaExceeded);
        }
        self.total_bytes = next_total_bytes;
        Ok(())
    }

    #[cfg(feature = "vfs_bundle_tar")]
    fn check_blob_len(&self, len: usize) -> Result<(), VfsError> {
        if len > self.max_total_bytes {
            Err(VfsError::QuotaExceeded)
        } else {
            Ok(())
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn unsafe_follow_host_links_enabled() -> Result<bool, VfsError> {
    match std::env::var(VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS) {
        Ok(raw) if raw == "1" => Ok(true),
        Ok(raw) if raw.is_empty() || raw == "0" => Ok(false),
        Ok(raw) => Err(VfsError::IoError(format!(
            "{VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS} must be unset, 0, or exactly 1; got {raw:?}"
        ))),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(err) => Err(VfsError::IoError(err.to_string())),
    }
}

fn is_host_link(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Walk a directory recursively, returning `(relative_path, contents)` pairs
/// suitable for [`BundleFs::from_entries`].
///
/// Directory bundles are sandbox inputs by default: host symlinks, junctions,
/// mount points, and other Windows reparse points are rejected instead of being
/// followed into `/bundle`. Native development can opt into following them with
/// exactly `MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS=1`; any other set value
/// fails closed so unsafe access is never inferred from loose truthiness.
pub(crate) fn read_dir_recursive(
    base: &str,
    quota: &mut VfsLoadQuota,
) -> Result<Vec<(String, Vec<u8>)>, VfsError> {
    use std::path::Path;

    let base_path = Path::new(base);
    let unsafe_follow_links = unsafe_follow_host_links_enabled()?;
    let base_meta =
        std::fs::symlink_metadata(base_path).map_err(|err| VfsError::IoError(err.to_string()))?;
    if is_host_link(&base_meta) && !unsafe_follow_links {
        return Err(VfsError::IoError(format!(
            "bundle directory root is host link: {base}"
        )));
    }

    let mut result = Vec::new();
    let mut stack = vec![base.to_string()];
    let mut visited_dirs = std::collections::BTreeSet::new();

    while let Some(dir) = stack.pop() {
        let canonical_dir =
            std::fs::canonicalize(&dir).map_err(|err| VfsError::IoError(err.to_string()))?;
        if !visited_dirs.insert(canonical_dir) {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(err) => return Err(VfsError::IoError(err.to_string())),
        };
        for entry in entries {
            let entry = entry.map_err(|err| VfsError::IoError(err.to_string()))?;
            let path = entry.path();
            let link_meta = std::fs::symlink_metadata(&path)
                .map_err(|err| VfsError::IoError(err.to_string()))?;
            let rel = path
                .strip_prefix(base_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let is_link = is_host_link(&link_meta);
            if is_link && !unsafe_follow_links {
                return Err(VfsError::IoError(format!(
                    "bundle directory contains host link: {rel}"
                )));
            }
            let meta = if is_link {
                std::fs::metadata(&path).map_err(|err| VfsError::IoError(err.to_string()))?
            } else {
                link_meta
            };
            if meta.is_dir() {
                stack.push(path.to_string_lossy().into_owned());
            } else if meta.is_file() {
                let expected_len = usize::try_from(meta.len()).unwrap_or(0);
                quota.reserve_entry(&rel, expected_len)?;
                let data =
                    std::fs::read(&path).map_err(|err| VfsError::IoError(err.to_string()))?;
                if data.len() > expected_len {
                    quota.reserve_additional_bytes(data.len() - expected_len)?;
                }
                result.push((rel, data));
            }
        }
    }
    Ok(result)
}

/// Lazily build a [`VfsState`] from environment variables.
///
/// Reads:
/// - `MOLT_VFS_BUNDLE` - path to a directory or `.tar` file mounted at `/bundle`.
/// - `MOLT_VFS_TMP_QUOTA_MB` - quota in MiB for the `/tmp` mount (default 64).
///
/// Returns `None` when `MOLT_VFS_BUNDLE` is not set.
type InjectedBundleEntries = Vec<(String, Vec<u8>)>;

struct InjectedBundleState {
    entries: InjectedBundleEntries,
    quota: VfsLoadQuota,
    error: Option<VfsError>,
}

/// Global slot for bundle data injected by the host before `_start`.
/// On Cloudflare Workers, worker.js writes the tar/entry data here
/// via `molt_vfs_inject_entry` before calling the WASM entry point.
static INJECTED_BUNDLE: Mutex<Option<InjectedBundleState>> = Mutex::new(None);

/// Host calls this to inject bundle entries before `_start`.
/// Each entry is (path, content). Called from JS or the WASM host.
///
/// # Safety
///
/// `path_ptr` must reference `path_len` bytes. When `data_len > 0`,
/// `data_ptr` must reference `data_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_vfs_inject_entry(
    path_ptr: *const u8,
    path_len: usize,
    data_ptr: *const u8,
    data_len: usize,
) {
    if path_ptr.is_null() || (data_len > 0 && data_ptr.is_null()) {
        return;
    }
    if path_len > VFS_BUNDLE_MAX_PATH_BYTES {
        return;
    }
    let path = unsafe { std::slice::from_raw_parts(path_ptr, path_len) };
    let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
    let path_str = String::from_utf8_lossy(path).to_string();
    let mut guard = INJECTED_BUNDLE.lock().unwrap();
    let state = guard.get_or_insert_with(|| InjectedBundleState {
        entries: Vec::new(),
        quota: VfsLoadQuota::from_env(),
        error: None,
    });
    if state.error.is_some() {
        return;
    }
    if path_str.is_empty()
        || path_str.starts_with('/')
        || path_str.contains("..")
        || path_str.contains('\0')
    {
        state.error = Some(VfsError::IoError("invalid injected VFS path".to_string()));
        return;
    }
    if let Err(err) = state.quota.reserve_entry(&path_str, data_len) {
        state.entries.clear();
        state.error = Some(err);
        return;
    }
    state.entries.push((path_str, data.to_vec()));
}

/// Host calls this to signal all entries have been injected.
/// Returns the number of entries loaded.
#[unsafe(no_mangle)]
pub extern "C" fn molt_vfs_inject_finish() -> i32 {
    let guard = INJECTED_BUNDLE.lock().unwrap();
    guard.as_ref().map_or(0, |state| {
        if state.error.is_some() {
            -1
        } else {
            state.entries.len() as i32
        }
    })
}

/// Load VFS from injected entries (WASM) or environment (native).
pub fn load_vfs() -> Option<VfsState> {
    match load_vfs_inner() {
        Ok(state) => state,
        Err(err) => panic!("failed to load VFS bundle: {err}"),
    }
}

pub(crate) fn load_vfs_inner() -> Result<Option<VfsState>, VfsError> {
    let injected = INJECTED_BUNDLE.lock().unwrap().take();
    if let Some(state) = injected {
        if let Some(err) = state.error {
            return Err(err);
        }
        if !state.entries.is_empty() {
            let mut mt = MountTable::new();
            let bundle = bundle::BundleFs::try_from_entries(state.entries)?;
            mt.add_mount("/bundle", Arc::new(bundle));
            add_runtime_mounts(&mut mt);
            return Ok(Some(VfsState::from_table(mt)));
        }
    }

    let Some(bundle_path) = std::env::var("MOLT_VFS_BUNDLE").ok() else {
        return Ok(None);
    };

    let mut mt = MountTable::new();
    let mut quota = VfsLoadQuota::from_env();

    if std::path::Path::new(&bundle_path).is_dir() {
        let entries = read_dir_recursive(&bundle_path, &mut quota)?;
        let bundle = bundle::BundleFs::try_from_entries(entries)?;
        mt.add_mount("/bundle", Arc::new(bundle));
    } else if bundle_path.ends_with(".tar") {
        #[cfg(feature = "vfs_bundle_tar")]
        {
            let tar_len = std::fs::metadata(&bundle_path)
                .ok()
                .and_then(|meta| usize::try_from(meta.len()).ok())
                .unwrap_or(0);
            quota.check_blob_len(tar_len)?;
            let tar_bytes =
                std::fs::read(&bundle_path).map_err(|err| VfsError::IoError(err.to_string()))?;
            let bundle = bundle::BundleFs::from_tar_with_quota(&tar_bytes, &mut quota)
                .map_err(VfsError::IoError)?;
            mt.add_mount("/bundle", Arc::new(bundle));
        }
    }

    add_runtime_mounts(&mut mt);
    Ok(Some(VfsState::from_table(mt)))
}

fn add_runtime_mounts(mt: &mut MountTable) {
    let quota_mb = std::env::var("MOLT_VFS_TMP_QUOTA_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    mt.add_mount("/tmp", Arc::new(tmp::TmpFs::new(quota_mb)));
    mt.add_mount("/dev", Arc::new(dev::DevFs::new()));
}
