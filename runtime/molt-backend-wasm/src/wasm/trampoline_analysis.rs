use crate::{SimpleIR, TrampolineKind};
use molt_tir::trampolines::CallableMetadata;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct WasmTrampolineAnalysis {
    pub(super) escaped_callable_targets: BTreeSet<String>,
    pub(super) task_kinds: BTreeMap<String, TrampolineKind>,
    pub(super) task_closure_sizes: BTreeMap<String, i64>,
    pub(super) default_trampoline_spec: BTreeMap<String, (usize, bool)>,
    /// Whether a direct WASM call to each user function leaves one boxed-i64
    /// result on the operand stack. Defined functions always do (ret_void
    /// materializes None); extern declarations follow their canonical
    /// FunctionIR signature.
    pub(super) function_abi_returns_value: BTreeMap<String, bool>,
}

#[cfg(test)]
pub(super) fn analyze_wasm_trampolines(ir: &SimpleIR) -> WasmTrampolineAnalysis {
    analyze_wasm_trampolines_with_source(ir, CallableMetadata::from_functions(&ir.functions))
}

pub(super) fn analyze_wasm_trampolines_with_source(
    ir: &SimpleIR,
    mut source: CallableMetadata,
) -> WasmTrampolineAnalysis {
    source.merge(CallableMetadata::from_definitions(&ir.functions));
    let CallableMetadata {
        escaped_callable_targets,
        trampoline_specs: func_trampoline_spec,
        task_kinds,
        task_closure_sizes,
    } = source;
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    let mut default_trampoline_spec: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    let mut function_abi_returns_value: BTreeMap<String, bool> = BTreeMap::new();
    for func_ir in &ir.functions {
        let default_has_closure = func_ir
            .params
            .first()
            .is_some_and(|name| name == crate::MOLT_CLOSURE_PARAM_NAME);
        let mut default_arity = func_ir.params.len();
        if default_has_closure && default_arity > 0 {
            default_arity = default_arity.saturating_sub(1);
        }
        let spec = func_trampoline_spec
            .get(&func_ir.name)
            .copied()
            .unwrap_or((default_arity, default_has_closure));
        default_trampoline_spec.insert(func_ir.name.clone(), spec);
        let abi_returns_value = if func_ir.is_extern {
            func_ir
                .extern_signature()
                .unwrap_or_else(|error| panic!("invalid WASM extern function declaration: {error}"))
                .returns_value
        } else {
            true
        };
        function_abi_returns_value.insert(func_ir.name.clone(), abi_returns_value);
    }

    WasmTrampolineAnalysis {
        escaped_callable_targets,
        task_kinds,
        task_closure_sizes,
        default_trampoline_spec,
        function_abi_returns_value,
    }
}

// The final ABI catalog must include generated partitions while source
// callable marker facts survive removal/separation by body optimization.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_task_metadata_and_final_partition_abis_have_distinct_custody() {
        let mut source = CallableMetadata::default();
        source
            .task_kinds
            .insert("worker_poll".into(), TrampolineKind::Coroutine);
        source.task_closure_sizes.insert("worker_poll".into(), 3);
        source
            .trampoline_specs
            .insert("worker_poll".into(), (0, true));
        source.escaped_callable_targets.insert("worker_poll".into());
        let ir = SimpleIR {
            functions: vec![
                crate::FunctionIR {
                    name: "worker_poll".into(),
                    params: vec![crate::MOLT_CLOSURE_PARAM_NAME.into()],
                    ..crate::FunctionIR::default()
                },
                crate::FunctionIR {
                    name: "opaque_partition".into(),
                    params: vec!["frame".into()],
                    codegen_partition: true,
                    ..crate::FunctionIR::default()
                },
            ],
            profile: None,
        };
        let analysis = analyze_wasm_trampolines_with_source(&ir, source);
        assert_eq!(
            analysis.task_kinds["worker_poll"],
            TrampolineKind::Coroutine
        );
        assert_eq!(analysis.task_closure_sizes["worker_poll"], 3);
        assert_eq!(analysis.default_trampoline_spec["worker_poll"], (0, true));
        assert_eq!(
            analysis.default_trampoline_spec["opaque_partition"],
            (1, false)
        );
        assert!(analysis.function_abi_returns_value["opaque_partition"]);
        assert!(
            !analysis
                .escaped_callable_targets
                .contains("opaque_partition")
        );
    }
}
