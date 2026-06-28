use super::*;

/// Transparent wrapper around `BTreeMap<String, u32>` that records which
/// import names are actually looked up during code emission.  Every
/// `Index<&str>` access inserts the key into a shared `BTreeSet` so we can
/// compute the set of *unused* imports after compilation finishes.
///
/// The `used` set is behind `Rc<RefCell<…>>` so that clones (needed to
/// work around borrow-checker constraints during `compile_func`) share
/// the same tracking set as the original.
pub(super) struct CompileFuncContext<'a> {
    pub(super) func_map: &'a BTreeMap<String, u32>,
    pub(super) func_indices: &'a BTreeMap<String, u32>,
    pub(super) trampoline_map: &'a BTreeMap<String, u32>,
    pub(super) table_base: u32,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) reloc_enabled: bool,
    /// Functions eligible for multi-value return optimization.
    /// Maps function name -> number of return values (2 or 3).
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    /// Functions whose WASM signature includes a leading closure (i64) parameter.
    /// The `call_guarded` fast path must extract closure bits from the callee
    /// object and prepend them to the argument list when calling these targets.
    pub(super) closure_functions: &'a BTreeSet<String>,
    /// Functions that escape through function-object creation ops and therefore
    /// must preserve callable-object dispatch semantics when invoked via
    /// `call_guarded`.
    pub(super) escaped_callable_targets: &'a BTreeSet<String>,
    /// Linear-memory offset of a scratch buffer used to spill `call_func` args.
    pub(super) call_func_spill_offset: u32,
    /// Linear-memory offset of a shared scratch buffer used for outlined class_def
    /// payloads (bases followed by attribute key/value pairs).
    pub(super) class_def_spill_offset: u32,
    /// Data segment ref for the 8-byte scratch slot used by `const_str` ops.
    pub(super) const_str_scratch_segment: DataSegmentRef,
    /// Precomputed production LIR-fast decisions keyed by function name.
    pub(super) lir_lowering_plans: &'a crate::wasm_plan::WasmFunctionLoweringPlans,
    /// Functions proven to return one of their parameters by alias.
    pub(super) return_alias_summaries: &'a BTreeMap<String, crate::passes::ReturnAliasSummary>,
}
