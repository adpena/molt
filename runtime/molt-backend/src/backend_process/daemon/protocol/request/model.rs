#[derive(Debug)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct DaemonJobRequest {
    pub(crate) id: String,
    pub(crate) is_wasm: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<String>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_link: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_table_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_split_runtime_runtime_table_min: Option<u32>,
    pub(crate) output: String,
    pub(crate) cache_key: String,
    pub(crate) function_cache_key: Option<String>,
    pub(crate) skip_module_output_if_synced: bool,
    pub(crate) skip_function_output_if_synced: bool,
    pub(crate) probe_cache_only: bool,
    pub(crate) ir: Option<molt_backend::BackendIrDocument>,
    pub(crate) ir_path: Option<String>,
}

#[derive(Debug)]
pub(crate) struct DaemonRequest {
    pub(crate) version: Option<u32>,
    pub(crate) ping: Option<bool>,
    pub(crate) include_health: Option<bool>,
    pub(crate) config_digest: Option<String>,
    pub(crate) jobs: Option<Vec<DaemonJobRequest>>,
}
