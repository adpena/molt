use crate::ir::ModuleRegistryIR;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_data::WasmDataSegments;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmCompileOptions;
use crate::wasm_table::WasmTableRelocations;
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
    /// Number of imported functions before the first defined function body.
    ///
    /// Relocatable data-pointer sites are recorded against defined-function
    /// body ordinals, not absolute function indices, because import stripping
    /// can shrink the imported-function prefix before reloc sections are
    /// attached.
    pub(in crate::wasm) func_import_count: u32,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    // Wrapped in TrackedImportIds to record which imports are actually referenced
    // during code emission (see MOLT_WASM_IMPORT_AUDIT).
    pub(in crate::wasm) import_ids: TrackedImportIds,
    pub(in crate::wasm) data_segments: WasmDataSegments,
    pub(in crate::wasm) table_relocations: WasmTableRelocations,
    pub(in crate::wasm) molt_main_index: Option<u32>,
    pub(in crate::wasm) molt_host_init_index: Option<u32>,
    pub(in crate::wasm) options: WasmCompileOptions,
    /// Number of tail calls emitted via `return_call` (WASM tail calls proposal).
    pub(in crate::wasm) tail_calls_emitted: usize,
    pub(in crate::wasm) numeric_lane_stats: WasmNumericLaneStats,
    pub(in crate::wasm) module_registry: Option<ModuleRegistryIR>,
    pub(in crate::wasm) module_registry_segment: Option<DataSegmentRef>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WasmCompileDiagnostics {
    pub tail_calls_emitted: usize,
    pub numeric_lanes: WasmNumericLaneStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmCompileOutput {
    pub wasm: Vec<u8>,
    pub diagnostics: WasmCompileDiagnostics,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WasmNumericLaneStats {
    pub op_loop_additive_inline_int_raw_sites: usize,
    pub op_loop_additive_float_raw_sites: usize,
    pub op_loop_additive_guarded_int_sites: usize,
    pub op_loop_additive_boxed_runtime_sites: usize,
    pub op_loop_bitwise_inline_int_raw_sites: usize,
    pub op_loop_bitwise_guarded_int_sites: usize,
    pub op_loop_bitwise_boxed_runtime_sites: usize,
    pub op_loop_division_guarded_int_sites: usize,
    pub op_loop_division_boxed_runtime_sites: usize,
}

impl WasmNumericLaneStats {
    pub fn raw_result_total(self) -> usize {
        self.op_loop_additive_inline_int_raw_sites
            + self.op_loop_additive_float_raw_sites
            + self.op_loop_bitwise_inline_int_raw_sites
    }

    pub fn guarded_or_boxed_total(self) -> usize {
        self.op_loop_additive_guarded_int_sites
            + self.op_loop_additive_boxed_runtime_sites
            + self.op_loop_bitwise_guarded_int_sites
            + self.op_loop_bitwise_boxed_runtime_sites
            + self.op_loop_division_guarded_int_sites
            + self.op_loop_division_boxed_runtime_sites
    }

    pub(in crate::wasm) fn record_op_loop_additive_inline_int_raw_site(&mut self) {
        self.op_loop_additive_inline_int_raw_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_additive_float_raw_site(&mut self) {
        self.op_loop_additive_float_raw_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_additive_guarded_int_site(&mut self) {
        self.op_loop_additive_guarded_int_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_additive_boxed_runtime_site(&mut self) {
        self.op_loop_additive_boxed_runtime_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_bitwise_inline_int_raw_site(&mut self) {
        self.op_loop_bitwise_inline_int_raw_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_bitwise_guarded_int_site(&mut self) {
        self.op_loop_bitwise_guarded_int_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_bitwise_boxed_runtime_site(&mut self) {
        self.op_loop_bitwise_boxed_runtime_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_division_guarded_int_site(&mut self) {
        self.op_loop_division_guarded_int_sites += 1;
    }

    pub(in crate::wasm) fn record_op_loop_division_boxed_runtime_site(&mut self) {
        self.op_loop_division_boxed_runtime_sites += 1;
    }
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
            func_import_count: 0,
            import_ids: TrackedImportIds::new(BTreeMap::new()),
            data_segments: WasmDataSegments::new(options.data_base),
            table_relocations: WasmTableRelocations::default(),
            molt_main_index: None,
            molt_host_init_index: None,
            options,
            tail_calls_emitted: 0,
            numeric_lane_stats: WasmNumericLaneStats::default(),
            module_registry: None,
            module_registry_segment: None,
        }
    }

    pub fn with_module_registry(mut self, module_registry: Option<ModuleRegistryIR>) -> Self {
        self.module_registry = module_registry;
        self
    }

    pub(in crate::wasm) fn compile_diagnostics(&self) -> WasmCompileDiagnostics {
        WasmCompileDiagnostics {
            tail_calls_emitted: self.tail_calls_emitted,
            numeric_lanes: self.numeric_lane_stats,
        }
    }
}
