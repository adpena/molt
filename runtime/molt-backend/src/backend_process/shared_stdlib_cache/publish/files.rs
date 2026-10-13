use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use molt_ir::content_digest::bytes_to_lower_hex;
use sha2::{Digest, Sha256};

pub(super) fn sha256_file_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(bytes_to_lower_hex(digest.as_ref()))
}

/// The Python eviction owner reads this directory's mtime. The caller holds its
/// existing publication stripe; immutable archive/object bytes are never touched.
/// Match cache_eviction.AUTOMATIC_PRUNE_INTERVAL_SECONDS to avoid metadata writes
/// on every hot lookup. Optional recency failure cannot revoke admitted bytes.
pub(super) fn refresh_cache_entry_recency(directory: &Path) {
    use std::time::{Duration, SystemTime};

    let refresh = || -> io::Result<()> {
        let metadata = std::fs::symlink_metadata(directory)?;
        if !metadata.is_dir() {
            return Ok(());
        }
        let now = SystemTime::now();
        if now.duration_since(metadata.modified()?).unwrap_or_default() < Duration::from_secs(300) {
            return Ok(());
        }
        let mut options = File::options();
        #[cfg(not(windows))]
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_WRITE_ATTRIBUTES,
            };
            options
                .access_mode(FILE_WRITE_ATTRIBUTES)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(directory)?.set_modified(now)
    };
    let _ = refresh();
}
