//! Binary and WASM output size measurement.
//!
//! Tracks artifact sizes for regression detection. The Python orchestrator
//! calls this via `cargo test -p molt-harness` to collect size metrics.

use std::path::Path;

/// Measure the size of a file in bytes. Returns 0 if the file does not exist.
pub fn file_size_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Measure sizes of all artifacts in a directory matching an extension.
pub fn artifact_sizes(dir: &Path, extension: &str) -> Vec<(String, u64)> {
    let mut sizes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == extension) {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let size = file_size_bytes(&path);
                sizes.push((name, size));
            }
        }
    }
    sizes.sort_by(|a, b| a.0.cmp(&b.0));
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn file_size_bytes_returns_correct_size() {
        let dir = std::env::temp_dir().join("molt-harness-test-size");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.bin");
        fs::write(&path, b"hello").unwrap();
        assert_eq!(file_size_bytes(&path), 5);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_size_bytes_returns_zero_for_missing() {
        let path = Path::new("/nonexistent/file.bin");
        assert_eq!(file_size_bytes(path), 0);
    }

    #[test]
    fn artifact_sizes_finds_matching_files() {
        let dir = std::env::temp_dir().join("molt-harness-artifacts-test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.wasm"), b"abc").unwrap();
        fs::write(dir.join("b.wasm"), b"abcdef").unwrap();
        fs::write(dir.join("c.txt"), b"ignored").unwrap();

        let sizes = artifact_sizes(&dir, "wasm");
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[0], ("a.wasm".to_string(), 3));
        assert_eq!(sizes[1], ("b.wasm".to_string(), 6));

        fs::remove_dir_all(&dir).unwrap();
    }
}
