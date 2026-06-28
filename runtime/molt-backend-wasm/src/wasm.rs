use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_abi::{
    CALL_INDIRECT_IMPORTS, CALL_INDIRECT_MAX_ARITY, GEN_CONTROL_SIZE, POLL_TABLE_IMPORTS,
    RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS, RUNTIME_CALLABLE_IMPORTS,
    RuntimeCallableResult, TAG_EXCEPTION_INDEX, TASK_KIND_COROUTINE, TASK_KIND_FUTURE,
    TASK_KIND_GENERATOR, TypeSectionExt, emit_static_type_section, poll_table_import_slot,
};
use crate::wasm_binary::{
    emit_call, emit_call_indirect, emit_i32_const, emit_ref_func, emit_return_call,
    emit_simple_call, emit_table_index_i64, encode_u32_leb128_padded,
};
use crate::wasm_data::{DataSegmentRef, WasmDataSegments};
use crate::wasm_import_tracking::{TrackedImportIds, selected_import_id};
pub use crate::wasm_options::{WasmCompileOptions, WasmProfile};
pub(crate) mod body;
mod class_def_layout;
mod compile_pipeline;
mod constant_ops;
mod context;
mod control_flow;
mod data_segments;
mod frame_locals;
mod function_emitter;
mod function_frame;
pub(crate) mod lir_fast;
mod local_analysis;
mod module_abi;
mod multi_return_layout;
mod op_loop;
mod state_dispatch;
mod tir_pipeline;
mod trampoline_analysis;
use crate::wasm_plan::{
    DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES,
    compute_lir_wasm_lowering_plans_from_final_ir_with_escaped, detect_multi_return_candidates,
    gpu_runtime_call_symbol, is_shared_drop_fact_marker, wasm_scalar_integer_fast_path_for_op,
    wasm_scalar_truthiness_fast_path_for_name, wasm_specialized_container_import,
};
use crate::wasm_values::{
    ConstantCache, INT_MASK, IntFastLane, POINTER_MASK, box_bool, box_int, box_none, box_pending,
    emit_box_bool_from_i32, emit_box_int_from_local_opt, emit_branch_truthiness_i32,
    emit_f64_to_i64_canonical, emit_inline_int_range_check, emit_trusted_int_fast_path_guard_close,
    emit_trusted_int_fast_path_guard_open, emit_unbox_int_local_trusted_opt,
    emit_unbox_int_local_trusted_tee_opt, stable_ic_site_id,
};
use crate::{FunctionIR, OpIR, SimpleIR, TrampolineKind, TrampolineSpec};
#[cfg(test)]
pub(in crate::wasm) use frame_locals::WasmFrameLocalKind;
pub(in crate::wasm) use frame_locals::{
    WasmFrameLocals, WasmFrameSyntheticLocal, WasmLiteralScratchLocals,
};
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{
    BlockType, Catch, CodeSection, ConstExpr, ElementMode, ElementSection, ElementSegment,
    Elements, Encode, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    ImportSection, Instruction, MemorySection, Module, RefType, TableSection, TableType,
    TypeSection, ValType,
};
#[cfg(test)]
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

pub struct WasmBackend {
    module: Module,
    types: TypeSection,
    funcs: FunctionSection,
    codes: CodeSection,
    exports: ExportSection,
    imports: ImportSection,
    memories: MemorySection,
    tables: TableSection,
    func_count: u32,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    // Wrapped in TrackedImportIds to record which imports are actually referenced
    // during code emission (see MOLT_WASM_IMPORT_AUDIT).
    import_ids: TrackedImportIds,
    data_segments: WasmDataSegments,
    molt_main_index: Option<u32>,
    options: WasmCompileOptions,
    /// Number of tail calls emitted via `return_call` (WASM tail calls proposal).
    tail_calls_emitted: usize,
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
            options,
            tail_calls_emitted: 0,
        }
    }
}

#[cfg(test)]
mod tests;
