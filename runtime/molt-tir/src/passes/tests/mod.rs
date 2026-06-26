use super::*;
use crate::tir::effect_proof::EffectProof;
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::BTreeSet;

fn make_op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..Default::default()
    }
}

fn make_const_int(out: &str, val: i64) -> OpIR {
    OpIR {
        kind: "const".to_string(),
        value: Some(val),
        out: Some(out.to_string()),
        ..Default::default()
    }
}

fn make_store_var(var: &str, arg: &str) -> OpIR {
    OpIR {
        kind: "store_var".to_string(),
        var: Some(var.to_string()),
        args: Some(vec![arg.to_string()]),
        ..Default::default()
    }
}

fn make_const_str(out: &str, value: &str) -> OpIR {
    OpIR {
        kind: "const_str".to_string(),
        out: Some(out.to_string()),
        s_value: Some(value.to_string()),
        ..Default::default()
    }
}

fn make_call_func(out: &str, callee: &str, args: &[&str]) -> OpIR {
    let mut full_args = vec![callee.to_string()];
    full_args.extend(args.iter().map(|a| a.to_string()));
    OpIR {
        kind: "call_func".to_string(),
        out: Some(out.to_string()),
        args: Some(full_args),
        ..Default::default()
    }
}

fn manifest_func(ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: "m".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops,
    }
}

fn make_arith(kind: &str, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..Default::default()
    }
}

fn make_ref_op(kind: &str, arg: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(vec![arg.to_string()]),
        ..Default::default()
    }
}

mod control_and_dead_ops;
mod deforest_dispatch;
mod manifest;
mod rc_and_dead_functions;
mod splitting;
