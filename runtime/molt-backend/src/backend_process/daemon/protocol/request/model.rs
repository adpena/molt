#[cfg(feature = "native-backend")]
use crate::backend_process::NativeTargetOptions;
#[cfg(feature = "wasm-backend")]
use crate::backend_process::WasmTargetOptions;

#[derive(Debug)]
pub(crate) struct DaemonJobRequest {
    pub(crate) id: String,
    pub(crate) target: DaemonJobTarget,
    pub(crate) output: String,
    pub(crate) cache_key: String,
    pub(crate) function_cache_key: Option<String>,
    pub(crate) skip_module_output_if_synced: bool,
    pub(crate) skip_function_output_if_synced: bool,
    pub(crate) probe_cache_only: bool,
    pub(crate) ir: Option<molt_backend::BackendIrDocument>,
    pub(crate) ir_path: Option<String>,
}

/// The artifact a daemon job builds. The daemon serves native and WASM jobs
/// for the backends this binary contains; the request parser refuses others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DaemonJobTarget {
    #[cfg(feature = "native-backend")]
    Native(NativeTargetOptions),
    #[cfg(feature = "wasm-backend")]
    Wasm(WasmTargetOptions),
}

#[derive(Debug)]
pub(crate) struct DaemonRequest {
    pub(crate) version: Option<u32>,
    pub(crate) ping: Option<bool>,
    pub(crate) include_health: Option<bool>,
    pub(crate) config_digest: Option<String>,
    pub(crate) jobs: Option<Vec<DaemonJobRequest>>,
}
