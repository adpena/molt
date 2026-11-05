//! VFS loading and host-injected bundle custody.

use crate::{MountTable, VfsBackend, VfsError, VfsStat, VfsState, bundle, dev, tmp};
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

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

    /// Read an entry whose metadata length was already reserved. Preserve that
    /// reservation if the file shrank, and charge growth exactly once.
    fn read_reserved_entry(
        &mut self,
        reader: impl Read,
        expected_len: usize,
    ) -> Result<Vec<u8>, VfsError> {
        let remaining = self
            .max_total_bytes
            .checked_sub(self.total_bytes)
            .ok_or(VfsError::QuotaExceeded)?;
        let limit = expected_len
            .checked_add(remaining)
            .ok_or(VfsError::QuotaExceeded)?
            .min(self.max_entry_bytes);
        let data = read_bounded_payload(reader, limit)?;
        if data.len() > expected_len {
            self.reserve_additional_bytes(data.len() - expected_len)?;
        }
        Ok(data)
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

/// Permit at most one probe byte beyond the admitted size, regardless of
/// payload growth after metadata inspection. No reader is consumed on overflow.
fn read_bounded_payload(reader: impl Read, limit: usize) -> Result<Vec<u8>, VfsError> {
    let probe_limit = u64::try_from(limit)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or(VfsError::QuotaExceeded)?;
    let mut data = Vec::new();
    reader
        .take(probe_limit)
        .read_to_end(&mut data)
        .map_err(|error| VfsError::IoError(error.to_string()))?;
    if data.len() > limit {
        return Err(VfsError::QuotaExceeded);
    }
    Ok(data)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(crate) fn unsafe_follow_host_links_enabled() -> Result<bool, VfsError> {
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
    base: impl AsRef<std::path::Path>,
    quota: &mut VfsLoadQuota,
    unsafe_follow_links: bool,
) -> Result<Vec<(String, Vec<u8>)>, VfsError> {
    let base_path = base.as_ref();
    let base_meta =
        std::fs::symlink_metadata(base_path).map_err(|err| VfsError::IoError(err.to_string()))?;
    if is_host_link(&base_meta) && !unsafe_follow_links {
        return Err(VfsError::IoError(format!(
            "bundle directory root is host link: {}",
            base_path.display()
        )));
    }

    let mut result = Vec::new();
    let mut stack = vec![base_path.to_path_buf()];
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
                stack.push(path);
            } else if meta.is_file() {
                let expected_len =
                    usize::try_from(meta.len()).map_err(|_| VfsError::QuotaExceeded)?;
                quota.reserve_entry(&rel, expected_len)?;
                let file =
                    std::fs::File::open(&path).map_err(|err| VfsError::IoError(err.to_string()))?;
                let data = quota.read_reserved_entry(file, expected_len)?;
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

/// The configured bundle source is captured without touching the host filesystem.
/// Its payload (or load error) is published once, only at backend access.
enum BundleSource {
    Injected(InjectedBundleState),
    Host {
        path: Result<std::path::PathBuf, VfsError>,
        quota: VfsLoadQuota,
        follow_links: Result<bool, VfsError>,
    },
}

impl BundleSource {
    fn load(self) -> Result<bundle::BundleFs, VfsError> {
        match self {
            Self::Injected(state) => {
                if let Some(error) = state.error {
                    return Err(error);
                }
                bundle::BundleFs::try_from_entries(state.entries)
            }
            Self::Host {
                path,
                mut quota,
                follow_links,
            } => {
                let path = path?;
                let follow_links = follow_links?;
                if path.is_dir() {
                    let entries = read_dir_recursive(&path, &mut quota, follow_links)?;
                    return bundle::BundleFs::try_from_entries(entries);
                }
                if path.extension().is_some_and(|extension| extension == "tar") {
                    #[cfg(feature = "vfs_bundle_tar")]
                    {
                        let metadata = std::fs::metadata(&path)
                            .map_err(|err| VfsError::IoError(err.to_string()))?;
                        let tar_len =
                            usize::try_from(metadata.len()).map_err(|_| VfsError::QuotaExceeded)?;
                        quota.check_blob_len(tar_len)?;
                        let file = std::fs::File::open(&path)
                            .map_err(|err| VfsError::IoError(err.to_string()))?;
                        let bytes = read_bounded_payload(file, quota.max_total_bytes)?;
                        return bundle::BundleFs::from_tar_with_quota(&bytes, &mut quota)
                            .map_err(VfsError::IoError);
                    }
                    #[cfg(not(feature = "vfs_bundle_tar"))]
                    return Err(VfsError::IoError(
                        "VFS tar bundles are unavailable in this build".into(),
                    ));
                }
                Err(VfsError::IoError(format!(
                    "invalid VFS bundle source: {}",
                    path.display()
                )))
            }
        }
    }
}

struct LazyBundleFs {
    source: Mutex<Option<BundleSource>>,
    loaded: OnceLock<Result<bundle::BundleFs, VfsError>>,
}

impl LazyBundleFs {
    fn new(source: BundleSource) -> Self {
        Self {
            source: Mutex::new(Some(source)),
            loaded: OnceLock::new(),
        }
    }

    fn get(&self) -> Result<&bundle::BundleFs, VfsError> {
        self.loaded
            .get_or_init(|| {
                let source = self
                    .source
                    .lock()
                    .unwrap()
                    .take()
                    .expect("bundle source must have one initialization owner");
                source.load()
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl VfsBackend for LazyBundleFs {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        self.get()?.open_read(path)
    }
    fn open_read_shared(&self, path: &str) -> Result<Arc<Vec<u8>>, VfsError> {
        self.get()?.open_read_shared(path)
    }
    fn stat(&self, path: &str) -> Result<VfsStat, VfsError> {
        self.get()?.stat(path)
    }
    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError> {
        self.get()?.readdir(path)
    }
    fn exists(&self, path: &str) -> bool {
        self.get().is_ok_and(|bundle| bundle.exists(path))
    }
    fn is_readonly(&self) -> bool {
        true
    }
    fn open_write(&self, _path: &str, _data: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }
    fn open_append(&self, _path: &str, _data: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }
    fn mkdir(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }
    fn unlink(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }
    fn rename(&self, _from: &str, _to: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }
}

/// Capture the startup mount table from injected data or operator settings.
/// Runtime initialization calls this before guest entry, including when no
/// bundle is configured. Consumers retain that decision instead of rereading
/// the environment. Payload loading remains lazy.
/// Routing never reads or validates a host bundle payload. A configured bundle
/// always owns /bundle; a bad source fails there instead of falling through to
/// the host filesystem. Relative host source paths resolve against startup CWD;
/// no canonicalization, symlink resolution, stat or payload read occurs here.
pub fn load_vfs() -> Option<VfsState> {
    let injected = INJECTED_BUNDLE.lock().unwrap().take();
    let source = match injected {
        Some(state) if state.error.is_some() || !state.entries.is_empty() => {
            BundleSource::Injected(state)
        }
        _ => BundleSource::Host {
            path: {
                let path = std::path::PathBuf::from(std::env::var("MOLT_VFS_BUNDLE").ok()?);
                if path.is_absolute() {
                    Ok(path)
                } else {
                    // `absolute` obtains CWD only when required and handles
                    // Windows drive-relative/root-relative paths without I/O
                    // against the selected bundle or resolving its links.
                    std::path::absolute(path).map_err(|error| VfsError::IoError(error.to_string()))
                }
            },
            quota: VfsLoadQuota::from_env(),
            follow_links: unsafe_follow_host_links_enabled(),
        },
    };
    let mut mt = MountTable::new();
    mt.add_mount("/bundle", Arc::new(LazyBundleFs::new(source)));
    add_runtime_mounts(&mut mt);
    Some(VfsState::from_table(mt))
}

fn add_runtime_mounts(mt: &mut MountTable) {
    let quota_mb = std::env::var("MOLT_VFS_TMP_QUOTA_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    mt.add_mount("/tmp", Arc::new(tmp::TmpFs::new(quota_mb)));
    mt.add_mount("/dev", Arc::new(dev::DevFs::new()));
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn oversized_payload_consumes_only_one_probe_byte_beyond_limit() {
        let mut source = Cursor::new([0_u8; 64]);
        assert!(matches!(
            read_bounded_payload(&mut source, 4),
            Err(VfsError::QuotaExceeded)
        ));
        assert_eq!(source.position(), 5);
        let mut exact = Cursor::new(*b"four");
        assert_eq!(read_bounded_payload(&mut exact, 4).unwrap(), b"four");
    }

    #[test]
    fn growing_entry_cannot_exceed_its_individual_limit() {
        let mut quota = VfsLoadQuota::new_for_test(64, 8, 64);
        quota.max_entry_bytes = 4;
        quota.reserve_entry("file", 1).unwrap();
        let mut source = Cursor::new([0_u8; 64]);
        assert!(matches!(
            quota.read_reserved_entry(&mut source, 1),
            Err(VfsError::QuotaExceeded)
        ));
        assert_eq!(source.position(), 5);
        assert_eq!(quota.total_bytes, 1);
    }

    #[test]
    fn growing_entry_uses_only_remaining_total_and_charges_growth_once() {
        let mut quota = VfsLoadQuota::new_for_test(8, 8, 64);
        quota.reserve_entry("earlier", 5).unwrap();
        quota.reserve_entry("current", 1).unwrap();
        let mut oversized = Cursor::new([0_u8; 64]);
        assert!(matches!(
            quota.read_reserved_entry(&mut oversized, 1),
            Err(VfsError::QuotaExceeded)
        ));
        assert_eq!(oversized.position(), 4);
        assert_eq!(quota.total_bytes, 6);
        assert_eq!(
            quota.read_reserved_entry(Cursor::new(*b"abc"), 1).unwrap(),
            b"abc"
        );
        assert_eq!(quota.total_bytes, 8);
    }

    #[test]
    fn shrinking_entry_keeps_existing_metadata_reservation() {
        let mut quota = VfsLoadQuota::new_for_test(8, 8, 64);
        quota.reserve_entry("file", 4).unwrap();
        assert_eq!(
            quota.read_reserved_entry(Cursor::new(*b"a"), 4).unwrap(),
            b"a"
        );
        assert_eq!(quota.total_bytes, 4);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn overflowing_probe_limit_is_rejected_without_reading() {
        let mut source = Cursor::new(*b"a");
        assert!(matches!(
            read_bounded_payload(&mut source, usize::MAX),
            Err(VfsError::QuotaExceeded)
        ));
        assert_eq!(source.position(), 0);
    }
}
