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
