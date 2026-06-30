pub(crate) const RELOC_TABLE_BASE_DEFAULT: u32 = 4096;

/// WASM profile for import planning.
/// `Full` registers the whole generated host-import registry for process-host
/// compatibility. `Pure` and `Auto` use the runtime-surface planner so modules
/// import only the runtime functions observed from IR, with `Pure` additionally
/// failing closed for the generated process/IO/time capability families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmProfile {
    Full,
    Pure,
    /// Scan IR to include only imports that are actually used.
    Auto,
}

#[derive(Debug, Clone, Copy)]
pub struct WasmCompileOptions {
    pub reloc_enabled: bool,
    pub data_base: u32,
    pub table_base: u32,
    pub split_runtime_runtime_table_min: Option<u32>,
    /// Enable native WASM exception handling (WASM 3.0 EH proposal).
    /// Enabled by default for non-relocatable wasm output; set
    /// `MOLT_WASM_NATIVE_EH=0` to disable explicitly.
    pub native_eh_enabled: bool,
    /// WASM profile for compile-time import planning.
    /// Gated by `MOLT_WASM_PROFILE` environment variable ("auto", "full", or "pure").
    pub wasm_profile: WasmProfile,
}

impl Default for WasmCompileOptions {
    fn default() -> Self {
        Self {
            reloc_enabled: matches!(std::env::var("MOLT_WASM_LINK").as_deref(), Ok("1")),
            data_base: {
                let raw = std::env::var("MOLT_WASM_DATA_BASE")
                    .ok()
                    .and_then(|val| val.parse::<u64>().ok())
                    // Default: 64 MiB. The split-runtime layout shares
                    // linear memory between the Rust runtime WASM module
                    // (whose data segments start at ~1 MiB and whose
                    // dlmalloc heap grows upward from there) and the
                    // output module. A 1 MiB default would collide with
                    // the runtime's data region and cause string-pointer
                    // corruption on large module graphs. 64 MiB leaves
                    // ample headroom for the runtime heap.
                    .unwrap_or(64 * 1024 * 1024);
                let aligned = (raw + 7) & !7;
                aligned.min(u64::from(u32::MAX)) as u32
            },
            table_base: match std::env::var("MOLT_WASM_TABLE_BASE") {
                Ok(value) => value.parse::<u32>().unwrap_or(RELOC_TABLE_BASE_DEFAULT),
                Err(_) => RELOC_TABLE_BASE_DEFAULT,
            },
            split_runtime_runtime_table_min: std::env::var(
                "MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN",
            )
            .ok()
            .and_then(|value| value.parse::<u32>().ok()),
            native_eh_enabled: !matches!(std::env::var("MOLT_WASM_NATIVE_EH").as_deref(), Ok("0")),
            wasm_profile: match std::env::var("MOLT_WASM_PROFILE").as_deref() {
                Ok("auto") => WasmProfile::Auto,
                Ok("pure") => WasmProfile::Pure,
                Ok("full") => WasmProfile::Full,
                _ => WasmProfile::Auto,
            },
        }
    }
}
