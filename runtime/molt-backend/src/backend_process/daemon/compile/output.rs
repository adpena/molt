#[cfg(feature = "native-backend")]
use std::path::Path;

#[cfg(feature = "wasm-backend")]
use super::super::super::io_limits::write_output;
use super::super::cache::DaemonCache;
#[cfg(feature = "native-backend")]
use super::super::cache::capture_native_output;
#[cfg(feature = "wasm-backend")]
use super::super::cache::insert_daemon_cache_entries;
#[cfg(feature = "wasm-backend")]
use super::super::origin::DaemonCacheEntry;
use super::super::origin::DaemonJobOrigin;
use super::super::protocol::DaemonCacheKey;
use super::target::DaemonCompiledOutput;
#[cfg(feature = "wasm-backend")]
use crate::backend_process::input::ContentIdentity;
use crate::backend_process::input::content::AddressedInputIdentity;
use std::sync::Arc;

/// Publish a compiled artifact at `output` and, where the job's cache policy
/// allows it, keep it in memory. Returns the warnings the job reports.
pub(super) fn write_daemon_compiled_output(
    cache: &DaemonCache,
    output: &str,
    cache_key: Option<&Arc<DaemonCacheKey>>,
    function_cache_key: Option<&Arc<DaemonCacheKey>>,
    compiled_output: DaemonCompiledOutput,
    input: AddressedInputIdentity,
) -> Result<(Vec<String>, DaemonJobOrigin), String> {
    match compiled_output {
        #[cfg(feature = "wasm-backend")]
        DaemonCompiledOutput::Bytes(output_bytes) => {
            write_output(output, output_bytes.as_ref())
                .map_err(|err| format!("failed to write compiled output: {err}"))?;
            let origin = DaemonJobOrigin::new(
                input,
                ContentIdentity::of_bytes(&output_bytes),
                super::super::origin::CodegenOrigin::in_process(),
            );
            insert_daemon_cache_entries(
                cache,
                cache_key,
                function_cache_key,
                Arc::new(DaemonCacheEntry::new(output_bytes, origin.clone())),
            );
            Ok((Vec::new(), origin))
        }
        #[cfg(feature = "native-backend")]
        DaemonCompiledOutput::WrittenToPath {
            codegen,
            output: emitted_output,
        } => capture_native_output(
            cache,
            Path::new(output),
            cache_key,
            function_cache_key,
            input,
            codegen,
            emitted_output,
        ),
    }
}
