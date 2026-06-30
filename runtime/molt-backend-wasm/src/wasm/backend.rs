use crate::wasm_data::WasmDataSegments;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmCompileOptions;
use std::collections::BTreeMap;
use wasm_encoder::{
    CodeSection, ExportSection, FunctionSection, ImportSection, MemorySection, Module,
    TableSection, TypeSection,
};

pub struct WasmBackend {
    pub(in crate::wasm) module: Module,
    pub(in crate::wasm) types: TypeSection,
    pub(in crate::wasm) funcs: FunctionSection,
    pub(in crate::wasm) codes: CodeSection,
    pub(in crate::wasm) exports: ExportSection,
    pub(in crate::wasm) imports: ImportSection,
    pub(in crate::wasm) memories: MemorySection,
    pub(in crate::wasm) tables: TableSection,
    pub(in crate::wasm) func_count: u32,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    // Wrapped in TrackedImportIds to record which imports are actually referenced
    // during code emission (see MOLT_WASM_IMPORT_AUDIT).
    pub(in crate::wasm) import_ids: TrackedImportIds,
    pub(in crate::wasm) data_segments: WasmDataSegments,
    pub(in crate::wasm) molt_main_index: Option<u32>,
    pub(in crate::wasm) molt_host_init_index: Option<u32>,
    pub(in crate::wasm) options: WasmCompileOptions,
    /// Number of tail calls emitted via `return_call` (WASM tail calls proposal).
    pub(in crate::wasm) tail_calls_emitted: usize,
}

impl Default for WasmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmBackend {
    pub fn new() -> Self {
        Self::with_options(WasmCompileOptions::default())
    }

    pub fn with_options(options: WasmCompileOptions) -> Self {
        Self {
            module: Module::new(),
            types: TypeSection::new(),
            funcs: FunctionSection::new(),
            codes: CodeSection::new(),
            exports: ExportSection::new(),
            imports: ImportSection::new(),
            memories: MemorySection::new(),
            tables: TableSection::new(),
            func_count: 0,
            import_ids: TrackedImportIds::new(BTreeMap::new()),
            data_segments: WasmDataSegments::new(options.data_base),
            molt_main_index: None,
            molt_host_init_index: None,
            options,
            tail_calls_emitted: 0,
        }
    }
}
