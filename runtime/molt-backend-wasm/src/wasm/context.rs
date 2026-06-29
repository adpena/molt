use super::module_abi::WasmCallableCallSiteAbi;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use std::collections::BTreeMap;

/// Per-module authorities needed while compiling each function body.
pub(super) struct CompileFuncContext<'a> {
    pub(super) call_site_abi: WasmCallableCallSiteAbi<'a>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) reloc_enabled: bool,
    /// Functions eligible for multi-value return optimization.
    /// Maps function name -> number of return values (2 or 3).
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    /// Linear-memory offset of a shared scratch buffer used for outlined class_def
    /// payloads (bases followed by attribute key/value pairs).
    pub(super) class_def_spill_offset: u32,
    /// Data segment ref for the 8-byte scratch slot used by `const_str` ops.
    pub(super) const_str_scratch_segment: DataSegmentRef,
    /// Precomputed production LIR-fast decisions keyed by function name.
    pub(super) lir_lowering_plans: &'a crate::wasm::lir_fast::WasmFunctionLoweringPlans,
}
