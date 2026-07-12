use std::fs::File;
use std::io::Write;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::super::super::io_limits::ensure_output_parent_dir;
use super::paths::stdlib_cache_temp_publish_path;

pub(crate) fn sha256_file_hex(path: &Path) -> io::Result<String> {
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
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = hasher.finalize();
    let bytes: &[u8] = digest.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

pub(crate) fn atomic_replace_file(temp_path: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if final_path.exists() {
        let _ = std::fs::remove_file(final_path);
    }
    std::fs::rename(temp_path, final_path)
}

pub(crate) fn sync_published_file(path: &Path) -> io::Result<()> {
    File::options()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

pub(crate) fn write_atomic_text_file(path: &Path, contents: &str) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let temp_path = stdlib_cache_temp_publish_path(path, "text");
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(err) = atomic_replace_file(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    sync_published_file(path)?;
    Ok(())
}
