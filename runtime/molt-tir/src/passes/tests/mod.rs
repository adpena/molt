use super::*;
use crate::tir::target_info::TargetInfo;
use crate::{ExecutionContextPolicy, FunctionIR, OpIR, SimpleIR};
use std::collections::BTreeSet;

fn eliminate_dead_ops(ir: &mut SimpleIR) {
    super::eliminate_dead_ops(ir, &TargetInfo::native_release_fast());
}

fn make_op(kind: molt_ir::OpKind) -> OpIR {
    OpIR {
        kind: kind,
        ..OpIR::new(kind)
    }
}

fn make_const_int(out: &str, val: i64) -> OpIR {
    OpIR {
        kind: molt_ir::OpKind::Const,
        value: Some(val),
        out: Some(out.to_string()),
        ..OpIR::new(molt_ir::OpKind::Const)
    }
}

fn make_store_var(var: &str, arg: &str) -> OpIR {
    OpIR {
        kind: molt_ir::OpKind::StoreVar,
        var: Some(var.to_string()),
        args: Some(vec![arg.to_string()]),
        ..OpIR::new(molt_ir::OpKind::StoreVar)
    }
}

fn make_const_str(out: &str, value: &str) -> OpIR {
    OpIR {
        kind: molt_ir::OpKind::ConstStr,
        out: Some(out.to_string()),
        s_value: Some(value.to_string()),
        ..OpIR::new(molt_ir::OpKind::ConstStr)
    }
}

fn make_call_func(out: &str, callee: &str, args: &[&str]) -> OpIR {
    let mut full_args = vec![callee.to_string()];
    full_args.extend(args.iter().map(|a| a.to_string()));
    OpIR {
        kind: molt_ir::OpKind::CallFunc,
        out: Some(out.to_string()),
        args: Some(full_args),
        ..OpIR::new(molt_ir::OpKind::CallFunc)
    }
}

fn make_builtin_func(out: &str, runtime_name: &str, arity: i64) -> OpIR {
    OpIR {
        kind: molt_ir::OpKind::BuiltinFunc,
        out: Some(out.to_string()),
        s_value: Some(runtime_name.to_string()),
        value: Some(arity),
        ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
    }
}

fn manifest_func(ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "m".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
        ops,
    }
}

fn make_arith(kind: molt_ir::OpKind, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind,
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..OpIR::new(kind)
    }
}

fn make_ref_op(kind: molt_ir::OpKind, arg: &str) -> OpIR {
    OpIR {
        kind: kind,
        args: Some(vec![arg.to_string()]),
        ..OpIR::new(kind)
    }
}

mod control_and_dead_ops;
mod deforest_dispatch;
mod manifest;
mod pipeline;
mod rc_and_dead_functions;
mod splitting;
mod splitting_control;
