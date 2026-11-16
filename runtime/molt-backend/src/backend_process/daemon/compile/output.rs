#[cfg(feature = "native-backend")]
use std::path::Path;

use super::super::cache::DaemonCache;
#[cfg(feature = "native-backend")]
use super::super::cache::capture_native_output;
use super::super::protocol::DaemonCacheKey;
use super::target::DaemonCompiledOutput;
use crate::backend_process::input::content::AddressedInputIdentity;
use crate::backend_process::origin::EmissionOrigin;
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
) -> Result<(Vec<String>, EmissionOrigin), String> {
    match compiled_output {
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
