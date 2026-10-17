#[cfg(feature = "native-backend")]
use std::path::Path;

#[cfg(feature = "wasm-backend")]
use super::super::super::io_limits::write_output;
use super::super::cache::DaemonCache;
#[cfg(feature = "wasm-backend")]
use super::super::cache::insert_daemon_cache_entries;
#[cfg(feature = "native-backend")]
use super::super::cache::maybe_cache_output_file;
use super::super::protocol::DaemonCacheKey;
use super::target::DaemonCompiledOutput;
use std::sync::Arc;

/// Publish a compiled artifact at `output` and, where the job's cache policy
/// allows it, keep it in memory. Returns the warnings the job reports.
pub(super) fn write_daemon_compiled_output(
    cache: &DaemonCache,
    output: &str,
    cache_key: Option<&Arc<DaemonCacheKey>>,
    function_cache_key: Option<&Arc<DaemonCacheKey>>,
    compiled_output: DaemonCompiledOutput,
) -> Result<Vec<String>, String> {
    match compiled_output {
        #[cfg(feature = "wasm-backend")]
        DaemonCompiledOutput::Bytes(output_bytes) => {
            write_output(output, output_bytes.as_ref())
                .map_err(|err| format!("failed to write compiled output: {err}"))?;
            insert_daemon_cache_entries(cache, cache_key, function_cache_key, output_bytes);
            Ok(Vec::new())
        }
        #[cfg(feature = "native-backend")]
        DaemonCompiledOutput::WrittenToPath => {
            let mut warnings = Vec::new();
            maybe_cache_output_file(
                cache,
                Path::new(output),
                cache_key,
                function_cache_key,
                &mut warnings,
            );
            Ok(warnings)
        }
    }
}
