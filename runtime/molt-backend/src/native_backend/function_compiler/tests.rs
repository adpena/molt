use super::fc::list_index_fast_path::{
    collect_pre_loop_defined_names, generic_list_int_lane_eligible, index_fallback_import_name,
    metadata_only_structured_loop_ops, scan_loop_hoistable_lists, scan_loop_int_sum_reduction,
    store_index_fallback_import_name,
};
use super::{
    FieldStoreMode, FunctionPreanalysis, ScalarRepresentationPlan, alias_root_name,
    box_raw_bool_value, box_raw_i64_value_overflow_safe, cleanup_roots_for_names,
    collect_slot_backed_join_names, def_var_from_boxed_transport, def_var_from_numeric_result,
    import_func_ref, is_cold_module_chunk_function, jump_block, live_exception_rebind_vars_for_op,
    mark_cleanup_root_once, materialize_label_block, preanalyze_function_ir, protect_cleanup_names,
    switch_to_block_materialized, switch_to_block_with_rebind,
};
use crate::repr::ScalarKind;
use crate::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::{
    ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types},
    settings,
    verifier::verify_function,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use std::collections::{BTreeMap, BTreeSet};

fn preanalyze_for_test(
    func_ir: &FunctionIR,
    return_alias_summaries: &BTreeMap<String, crate::passes::ReturnAliasSummary>,
) -> FunctionPreanalysis {
    let representation_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
    preanalyze_function_ir(func_ir, return_alias_summaries, &representation_plan)
}

fn representation_plan_for_ops(ops: &[OpIR]) -> ScalarRepresentationPlan {
    ScalarRepresentationPlan::for_function_ir(&FunctionIR {
        name: "storage_test".to_string(),
        params: vec![],
        ops: ops.to_vec(),
        param_types: None,
        source_file: None,
        is_extern: false,
    })
}

fn representation_plan_for_typed_ops(
    params: &[&str],
    param_types: Option<Vec<&str>>,
    ops: &[OpIR],
) -> ScalarRepresentationPlan {
    ScalarRepresentationPlan::for_function_ir(&FunctionIR {
        name: "container_dispatch_test".to_string(),
        params: params.iter().map(|param| param.to_string()).collect(),
        ops: ops.to_vec(),
        param_types: param_types.map(|types| types.into_iter().map(|ty| ty.to_string()).collect()),
        source_file: None,
        is_extern: false,
    })
}

fn scalar_transport_plan_for_boxed_transport_homes() -> ScalarRepresentationPlan {
    representation_plan_for_ops(&[
        OpIR {
            kind: "const_int".to_string(),
            out: Some("int_home".to_string()),
            value: Some(7),
            type_hint: Some("int".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_bool".to_string(),
            out: Some("bool_home".to_string()),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_float".to_string(),
            out: Some("float_home".to_string()),
            f_value: Some(1.25),
            ..OpIR::default()
        },
    ])
}

fn scalar_transport_plan_for_float_home() -> ScalarRepresentationPlan {
    representation_plan_for_ops(&[OpIR {
        kind: "const_float".to_string(),
        out: Some("float_home".to_string()),
        f_value: Some(1.25),
        ..OpIR::default()
    }])
}

fn list_int_new(out: &str) -> OpIR {
    OpIR {
        kind: "list_int_new".to_string(),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

fn op_kind(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

mod block_control;
mod cleanup_roots;
mod compile;
mod list_index_fast_path;
mod preanalysis;
mod scalar_carriers;
mod shared;
