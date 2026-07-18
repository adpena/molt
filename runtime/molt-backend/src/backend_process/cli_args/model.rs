#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WasmCliOptions {
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) link_relocs: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) table_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) split_runtime_app_table_base: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BackendCliArgs<'a> {
    pub(crate) wants_features: bool,
    pub(crate) wants_daemon: bool,
    pub(crate) socket_path: Option<&'a str>,
    pub(crate) is_wasm: bool,
    pub(crate) is_rust: bool,
    pub(crate) is_luau: bool,
    #[cfg_attr(not(feature = "luau-backend"), allow(dead_code))]
    pub(crate) use_ir_pipeline: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) output_path: Option<&'a str>,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) native_batch_job_file: Option<&'a str>,
    pub(crate) ir_file_path: Option<&'a str>,
    pub(crate) fact_graph_output_path: Option<&'a str>,
    pub(crate) fact_graph_function: Option<&'a str>,
    pub(crate) wasm_options: WasmCliOptions,
    pub(crate) ir_format: &'a str,
}
