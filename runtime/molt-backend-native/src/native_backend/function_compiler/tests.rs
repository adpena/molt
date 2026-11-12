use super::fc::arith_division::emit_raw_int_div_intmin_debug_guard;
use super::fc::list_index_fast_path::{
    collect_pre_loop_defined_names, generic_list_int_lane_eligible, index_fallback_runtime_fn,
    match_loop_int_sum_shape, scan_loop_hoistable_lists, scan_loop_int_sum_reduction,
    store_index_fallback_runtime_fn,
};
use super::fc::loops::metadata_only_structured_loop_ops;
use super::fc::memory::typed_slot_store_runtime_fn;
use super::{
    BlockTransportPlan, FieldStoreMode, FunctionPreanalysis, NativeCleanupRoots, NativeRcAuthority,
    NativeSsaValues, ScalarRepresentationPlan, box_raw_bool_value, box_raw_i64_value_overflow_safe,
    collect_slot_backed_join_names, def_var_from_boxed_transport, def_var_from_numeric_result,
    import_runtime_func_ref, is_cold_module_chunk_function, jump_block, materialize_label_block,
    preanalyze_function_ir, protect_cleanup_names, switch_to_block_materialized,
};
use crate::native_backend::native_ssa::FunctionBuilder;
use crate::native_backend::{NativeFunctionImports, NativeRefcountSite};
use crate::repr::{ContainerStorageKind, ScalarKind};
use crate::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::{
    ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types},
    settings,
    verifier::verify_function,
};
use cranelift_frontend::FunctionBuilderContext;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeFn;
use std::collections::{BTreeMap, BTreeSet};

/// Opcode-unit fixtures author logical operands. Give their actual Cranelift
/// function the same physical context boundary as a compiled Python entry,
/// retaining every authored parameter Value and its independent assertions.
fn complete_unit_fixture_abi(module: &dyn cranelift_module::Module, function: &mut Function) {
    let entry = function.layout.entry_block().expect("fixture entry");
    let mut previous = function.dfg.detach_block_params(entry);
    let parameters = previous.as_slice(&function.dfg.value_lists).to_vec();
    previous.clear(&mut function.dfg.value_lists);
    let context = function.dfg.append_block_param(entry, types::I64);
    for parameter in parameters {
        function.dfg.attach_block_param(entry, parameter);
    }
    function
        .signature
        .params
        .insert(0, AbiParam::new(types::I64));
    super::complete_attached_calls(module, function, context);
}

fn attached_arguments(
    function: &Function,
    instruction: cranelift_codegen::ir::Inst,
) -> &[cranelift_codegen::ir::Value] {
    let arguments = function.dfg.inst_args(instruction);
    let context = function
        .dfg
        .block_params(function.layout.entry_block().unwrap())[0];
    assert_eq!(
        arguments[0], context,
        "attached call must forward the exact entry context"
    );
    &arguments[1..]
}

fn physical_runtime_symbol(logical: &str) -> &'static str {
    molt_ir::runtime_callable_abi_generated::runtime_execution_abi(logical)
        .unwrap_or_else(|| panic!("missing runtime execution ABI: {logical}"))
        .physical_symbol
}

use crate::test_support::native_object_symbols;

fn native_representation_plan_for_test(func_ir: &FunctionIR) -> ScalarRepresentationPlan {
    ScalarRepresentationPlan::for_function_ir_for_target(
        func_ir,
        &crate::tir::TargetInfo::native_release_fast(),
    )
}

fn preanalyze_for_test(func_ir: &FunctionIR) -> FunctionPreanalysis {
    let representation_plan = native_representation_plan_for_test(func_ir);
    preanalyze_function_ir(func_ir, &representation_plan)
}

fn representation_plan_for_ops(ops: &[OpIR]) -> ScalarRepresentationPlan {
    native_representation_plan_for_test(&FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "storage_test".to_string(),
        params: vec![],
        ops: ops.to_vec(),
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    })
}

fn representation_plan_for_typed_ops(
    params: &[&str],
    param_types: Option<Vec<&str>>,
    ops: &[OpIR],
) -> ScalarRepresentationPlan {
    native_representation_plan_for_test(&FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "container_dispatch_test".to_string(),
        params: params.iter().map(|param| param.to_string()).collect(),
        ops: ops.to_vec(),
        param_types: param_types.map(|types| types.into_iter().map(|ty| ty.to_string()).collect()),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    })
}

fn scalar_transport_plan_for_boxed_transport_homes() -> ScalarRepresentationPlan {
    representation_plan_for_ops(&[
        OpIR {
            out: Some("int_home".to_string()),
            value: Some(7),
            type_hint: Some("int".to_string()),
            ..OpIR::new(OpKind::ConstInt)
        },
        OpIR {
            out: Some("bool_home".to_string()),
            value: Some(1),
            ..OpIR::new(OpKind::ConstBool)
        },
        OpIR {
            out: Some("float_home".to_string()),
            f_value: Some(1.25),
            ..OpIR::new(OpKind::ConstFloat)
        },
    ])
}

fn scalar_transport_plan_for_float_home() -> ScalarRepresentationPlan {
    representation_plan_for_ops(&[OpIR {
        out: Some("float_home".to_string()),
        f_value: Some(1.25),
        ..OpIR::new(OpKind::ConstFloat)
    }])
}

fn list_int_new(out: &str, count: &str, fill: &str) -> OpIR {
    OpIR {
        out: Some(out.to_string()),
        args: Some(vec![count.to_string(), fill.to_string()]),
        ..OpIR::new(OpKind::ListIntNew)
    }
}
fn storage_const(out: &str, value: i64) -> OpIR {
    OpIR {
        out: Some(out.into()),
        value: Some(value),
        ..OpIR::new(OpKind::Const)
    }
}

fn op_kind(kind: OpKind) -> OpIR {
    OpIR { ..OpIR::new(kind) }
}

mod attrs;
mod block_control;
mod call_operand_custody;
mod cleanup_roots;
mod compile;
mod list_index_fast_path;
mod literal_ownership;
mod loops;
mod megafn_codegen_snapshot;
mod native_callable_dispatch;
mod preanalysis_alias_and_loops;
mod preanalysis_classification;
mod preanalysis_field_stores;
mod preanalysis_slot_backing;
mod preanalysis_slot_exclusion;
mod scalar_carriers;
mod shared;

/// These IR-only fixtures have no attached runtime invocation. Every dynamic
/// pointer must consequently retain its original cold transition.
fn detached_refcount_site(
    builder: &mut FunctionBuilder<'_>,
    slow: cranelift_codegen::ir::FuncRef,
    operation: RefcountOperation,
) -> NativeRefcountSite {
    NativeRefcountSite {
        slow,
        admission: builder.ins().iconst(types::I64, 0),
        operation,
    }
}
fn detached_imports(builder: &mut FunctionBuilder<'_>) -> NativeFunctionImports {
    let mut imports = NativeFunctionImports::default();
    imports.set_refcount_admission(builder.ins().iconst(types::I64, 0));
    imports
}
