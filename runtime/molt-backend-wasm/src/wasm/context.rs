use super::call_site_abi::WasmCallSiteAbi;
use super::*;

/// Per-module authorities needed while compiling each function body.
pub(super) struct CompileFuncContext<'a> {
    pub(super) call_site_abi: WasmCallSiteAbi<'a>,
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
    pub(super) lir_lowering_plans: &'a crate::wasm_plan::WasmFunctionLoweringPlans,
}
