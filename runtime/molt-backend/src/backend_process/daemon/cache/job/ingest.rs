use std::path::Path;
use std::sync::Arc;

use super::super::entries::insert_daemon_cache_entries;
use super::super::state::DaemonCache;

pub(crate) fn maybe_cache_output_file(
    cache: &mut DaemonCache,
    output_path: &Path,
    cache_key: &str,
    function_cache_key: &str,
    warnings: &mut Vec<String>,
) {
    if cache_key.is_empty() && function_cache_key.is_empty() {
        return;
    }
    let metadata = match std::fs::metadata(output_path) {
        Ok(metadata) => metadata,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': metadata failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    let output_len = metadata.len();
    if cache
        .max_bytes
        .is_some_and(|max_bytes| output_len > max_bytes as u64)
    {
        let warning = format!(
            "skipped daemon memory cache for '{}' ({} bytes exceeds cache budget)",
            output_path.display(),
            output_len
        );
        eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
        warnings.push(warning);
        return;
    }
    let bytes = match std::fs::read(output_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': read failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    insert_daemon_cache_entries(
        cache,
        cache_key,
        function_cache_key,
        Arc::from(bytes.into_boxed_slice()),
    );
}
