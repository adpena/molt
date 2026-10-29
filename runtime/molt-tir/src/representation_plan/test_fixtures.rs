use crate::ir::{FunctionIR, OpIR};

pub(crate) fn op(
    kind: molt_ir::OpKind,
    out: Option<&str>,
    var: Option<&str>,
    args: &[&str],
) -> OpIR {
    OpIR {
        kind: kind,
        out: out.map(str::to_string),
        var: var.map(str::to_string),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
        ..OpIR::new(kind)
    }
}

pub(crate) fn op_v(
    kind: molt_ir::OpKind,
    out: Option<&str>,
    var: Option<&str>,
    args: &[&str],
    value: i64,
) -> OpIR {
    OpIR {
        value: Some(value),
        ..op(kind, out, var, args)
    }
}

pub(crate) fn function(
    name: &str,
    params: &[&str],
    param_types: Option<Vec<&str>>,
    ops: Vec<OpIR>,
) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: params.iter().map(|param| param.to_string()).collect(),
        ops,
        param_types: param_types.map(|types| types.into_iter().map(str::to_string).collect()),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}

/// Exact-bound variant of the post-`overflow_peel` SimpleIR shape (captured live from
/// `tmp/peel_sum.py`'s `compute` — fast structured loop with two
/// `checked_add`s + carried overflow flag + `prev_*` snapshot slots,
/// post-loop dispatch, generic boxed slow loop, exit-arg merge).
pub(crate) fn peeled_compute_func_ir() -> FunctionIR {
    function(
        "peel_compute",
        &["n"],
        None,
        vec![
            op_v(
                molt_ir::OpKind::Const,
                Some("_exact_bound"),
                None,
                &[],
                1_000_000,
            ),
            op_v(molt_ir::OpKind::Const, Some("v106"), None, &[], 0),
            op_v(molt_ir::OpKind::Const, Some("v108"), None, &[], 0),
            op_v(molt_ir::OpKind::Const, Some("v117"), None, &[], 1),
            op_v(molt_ir::OpKind::ConstBool, Some("_v43"), None, &[], 0),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg0"),
                &["v108"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg1"),
                &["v106"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg2"),
                &["_v43"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg3"),
                &["v108"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg4"),
                &["v106"],
            ),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 15),
            op_v(molt_ir::OpKind::Label, None, None, &[], 15),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v16"),
                Some("_bb1_arg0"),
                &[],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v17"),
                Some("_bb1_arg1"),
                &[],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v40"),
                Some("_bb1_arg2"),
                &[],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v41"),
                Some("_bb1_arg3"),
                &[],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v42"),
                Some("_bb1_arg4"),
                &[],
            ),
            op(
                molt_ir::OpKind::Lt,
                Some("fast_guard"),
                None,
                &["_v16", "_exact_bound"],
            ),
            op(molt_ir::OpKind::Not, Some("_v44"), None, &["_v40"]),
            op(
                molt_ir::OpKind::And,
                Some("_v45"),
                None,
                &["fast_guard", "_v44"],
            ),
            op(molt_ir::OpKind::LoopBreakIfFalse, None, None, &["_v45"]),
            op(
                molt_ir::OpKind::CheckedAdd,
                Some("_v47"),
                Some("_v22"),
                &["_v17", "_v16"],
            ),
            op(
                molt_ir::OpKind::CheckedAdd,
                Some("_v46"),
                Some("_v25"),
                &["_v16", "v117"],
            ),
            op(molt_ir::OpKind::Or, Some("_v48"), None, &["_v46", "_v47"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg0"),
                &["_v25"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg1"),
                &["_v22"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg2"),
                &["_v48"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg3"),
                &["_v16"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg4"),
                &["_v17"],
            ),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 19),
            op_v(molt_ir::OpKind::Label, None, None, &[], 19),
            op_v(molt_ir::OpKind::BrIf, None, None, &["_v40"], 20),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb5_arg0"),
                &["_v17"],
            ),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 17),
            op_v(molt_ir::OpKind::Label, None, None, &[], 20),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb7_arg0"),
                &["_v41"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb7_arg1"),
                &["_v42"],
            ),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 18),
            op_v(molt_ir::OpKind::Label, None, None, &[], 18),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v29"),
                Some("_bb7_arg0"),
                &[],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v30"),
                Some("_bb7_arg1"),
                &[],
            ),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 21),
            op_v(molt_ir::OpKind::Label, None, None, &[], 21),
            op(
                molt_ir::OpKind::Lt,
                Some("slow_guard"),
                None,
                &["_v29", "_exact_bound"],
            ),
            op_v(molt_ir::OpKind::BrIf, None, None, &["slow_guard"], 16),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb5_arg0"),
                &["_v30"],
            ),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 17),
            op_v(molt_ir::OpKind::Label, None, None, &[], 17),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v51"),
                Some("_bb5_arg0"),
                &[],
            ),
            op(molt_ir::OpKind::Ret, None, Some("_v51"), &["_v51"]),
            op_v(molt_ir::OpKind::Label, None, None, &[], 16),
            op(molt_ir::OpKind::Add, Some("v114"), None, &["_v30", "_v29"]),
            op(molt_ir::OpKind::Add, Some("v118"), None, &["_v29", "v117"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb7_arg0"),
                &["v118"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb7_arg1"),
                &["v114"],
            ),
            op_v(molt_ir::OpKind::Jump, None, None, &[], 18),
        ],
    )
}
