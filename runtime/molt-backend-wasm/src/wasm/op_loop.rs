use super::context::CompileFuncContext;
use super::function_frame::WasmFunctionFrame;
use super::module_abi::WasmCallableCallSiteAbi;
use super::{WasmBackend, WasmFrameLocals};
use crate::FunctionIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};

mod builder_ops;
mod call_emit;
mod call_ops;
mod control_ops;
mod core_runtime_ops;
mod emit_sequence;
mod local_slot_ops;
mod local_state_ops;
mod numeric_ops;
mod object_attr_ops;
mod result_sink;
mod runtime_service_ops;

/// Function-wide ownership/liveness authority for generic WASM lowering.
///
/// A function may be emitted as one plain stream, jumpful ranges, or individual
/// state-machine operations. RC coalescing and last-use analysis are properties
/// of the complete function and therefore must never be recomputed per emitted
/// slice. Once the terminal TIR drop pipeline publishes `drop_inserted`, those
/// explicit IncRef/DecRef operations are the sole RC authority and the legacy
/// SimpleIR coalescer is disabled exactly as it is in the native backend.
pub(super) struct WasmFunctionAnalysis {
    rc_skip_inc: HashSet<usize>,
    rc_skip_dec: HashSet<String>,
}

impl WasmFunctionAnalysis {
    pub(super) fn for_function(func: &FunctionIR) -> Self {
        let last_use = molt_tir::passes::build_last_use_map(&func.ops);
        let drop_inserted = func.ops.iter().any(|op| op.kind == "drop_inserted");
        let (rc_skip_inc, rc_skip_dec) = if drop_inserted {
            (HashSet::new(), HashSet::new())
        } else {
            molt_tir::passes::compute_rc_coalesce_skips(&func.ops, &last_use)
        };
        Self {
            rc_skip_inc,
            rc_skip_dec,
        }
    }
}

#[cfg(test)]
mod analysis_tests {
    use super::*;
    use crate::OpIR;

    fn op(kind: &str, out: Option<&str>, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: out.map(str::to_string),
            args: Some(args.iter().map(|value| (*value).to_string()).collect()),
            ..OpIR::default()
        }
    }

    fn function(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: "rc_authority".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }
    }

    #[test]
    fn drop_inserted_function_disables_legacy_rc_coalescing() {
        let analysis = WasmFunctionAnalysis::for_function(&function(vec![
            op("drop_inserted", None, &[]),
            op("inc_ref", Some("owned"), &["value"]),
            op("dec_ref", None, &["value"]),
            op("ret_void", None, &[]),
        ]));
        assert!(analysis.rc_skip_inc.is_empty());
        assert!(analysis.rc_skip_dec.is_empty());
    }

    #[test]
    fn legacy_rc_and_last_use_are_planned_over_the_complete_function() {
        let analysis = WasmFunctionAnalysis::for_function(&function(vec![
            op("inc_ref", Some("owned"), &["value"]),
            op("call", Some("result"), &["owned"]),
            op("dec_ref", None, &["value"]),
            op("ret", None, &["result"]),
        ]));
        let last_use = molt_tir::passes::build_last_use_map(
            &function(vec![
                op("inc_ref", Some("owned"), &["value"]),
                op("call", Some("result"), &["owned"]),
                op("dec_ref", None, &["value"]),
                op("ret", None, &["result"]),
            ])
            .ops,
        );
        assert_eq!(last_use.get("result"), Some(&3));
        assert_eq!(last_use.get("owned"), Some(&1));
        assert_eq!(analysis.rc_skip_inc, HashSet::from([0, 2]));
        assert!(analysis.rc_skip_dec.is_empty());
    }
}

pub(super) struct WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) func_ir: &'a FunctionIR,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) call_site_abi: &'a WasmCallableCallSiteAbi<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) exception_handler_region_indices: &'a BTreeSet<usize>,
    pub(super) frame: &'a WasmFunctionFrame,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
    pub(super) tail_call_enabled: bool,
    pub(super) tail_call_count: &'a Cell<usize>,
    pub(super) analysis: &'a WasmFunctionAnalysis,
}

impl<'a, 'ctx> WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) fn locals(&self) -> &WasmFrameLocals {
        self.frame.locals()
    }

    pub(super) fn const_cache(&self) -> &ConstantCache {
        self.frame.const_cache()
    }

    pub(super) fn scalar_plan(&self) -> &ScalarRepresentationPlan {
        self.frame.scalar_plan()
    }

    pub(super) fn arena_local(&self) -> Option<u32> {
        self.frame.arena_local()
    }
}
