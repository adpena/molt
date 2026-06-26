use crate::ir::{FunctionIR, OpIR};

pub(crate) fn op(kind: &str, out: Option<&str>, var: Option<&str>, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        out: out.map(str::to_string),
        var: var.map(str::to_string),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
        ..OpIR::default()
    }
}

pub(crate) fn op_v(
    kind: &str,
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
        name: name.to_string(),
        params: params.iter().map(|param| param.to_string()).collect(),
        ops,
        param_types: param_types.map(|types| types.into_iter().map(str::to_string).collect()),
        source_file: None,
        is_extern: false,
    }
}

/// The EXACT post-`overflow_peel` SimpleIR shape (captured live from
/// `tmp/peel_sum.py`'s `compute` — fast structured loop with two
/// `checked_add`s + carried overflow flag + `prev_*` snapshot slots,
/// post-loop dispatch, generic boxed slow loop, exit-arg merge).
pub(crate) fn peeled_compute_func_ir() -> FunctionIR {
    function(
        "peel_compute",
        &["n"],
        None,
        vec![
            op_v("const", Some("v106"), None, &[], 0),
            op_v("const", Some("v108"), None, &[], 0),
            op_v("const", Some("v117"), None, &[], 1),
            op_v("const_bool", Some("_v43"), None, &[], 0),
            op("store_var", None, Some("_bb1_arg0"), &["v108"]),
            op("store_var", None, Some("_bb1_arg1"), &["v106"]),
            op("store_var", None, Some("_bb1_arg2"), &["_v43"]),
            op("store_var", None, Some("_bb1_arg3"), &["v108"]),
            op("store_var", None, Some("_bb1_arg4"), &["v106"]),
            op_v("jump", None, None, &[], 15),
            op_v("label", None, None, &[], 15),
            op("loop_start", None, None, &[]),
            op("load_var", Some("_v16"), Some("_bb1_arg0"), &[]),
            op("load_var", Some("_v17"), Some("_bb1_arg1"), &[]),
            op("load_var", Some("_v40"), Some("_bb1_arg2"), &[]),
            op("load_var", Some("_v41"), Some("_bb1_arg3"), &[]),
            op("load_var", Some("_v42"), Some("_bb1_arg4"), &[]),
            op("lt", Some("v111"), None, &["_v16", "n"]),
            op("not", Some("_v44"), None, &["_v40"]),
            op("and", Some("_v45"), None, &["v111", "_v44"]),
            op("loop_break_if_false", None, None, &["_v45"]),
            op("checked_add", Some("_v47"), Some("_v22"), &["_v17", "_v16"]),
            op("checked_add", Some("_v46"), Some("_v25"), &["_v16", "v117"]),
            op("or", Some("_v48"), None, &["_v46", "_v47"]),
            op("store_var", None, Some("_bb1_arg0"), &["_v25"]),
            op("store_var", None, Some("_bb1_arg1"), &["_v22"]),
            op("store_var", None, Some("_bb1_arg2"), &["_v48"]),
            op("store_var", None, Some("_bb1_arg3"), &["_v16"]),
            op("store_var", None, Some("_bb1_arg4"), &["_v17"]),
            op("loop_continue", None, None, &[]),
            op("loop_end", None, None, &[]),
            op_v("jump", None, None, &[], 19),
            op_v("label", None, None, &[], 19),
            op_v("br_if", None, None, &["_v40"], 20),
            op("store_var", None, Some("_bb5_arg0"), &["_v17"]),
            op_v("jump", None, None, &[], 17),
            op_v("label", None, None, &[], 20),
            op("store_var", None, Some("_bb7_arg0"), &["_v41"]),
            op("store_var", None, Some("_bb7_arg1"), &["_v42"]),
            op_v("jump", None, None, &[], 18),
            op_v("label", None, None, &[], 18),
            op("load_var", Some("_v29"), Some("_bb7_arg0"), &[]),
            op("load_var", Some("_v30"), Some("_bb7_arg1"), &[]),
            op_v("jump", None, None, &[], 21),
            op_v("label", None, None, &[], 21),
            op("lt", Some("v111"), None, &["_v29", "n"]),
            op_v("br_if", None, None, &["v111"], 16),
            op("store_var", None, Some("_bb5_arg0"), &["_v30"]),
            op_v("jump", None, None, &[], 17),
            op_v("label", None, None, &[], 17),
            op("load_var", Some("_v51"), Some("_bb5_arg0"), &[]),
            op("ret", None, Some("_v51"), &["_v51"]),
            op_v("label", None, None, &[], 16),
            op("add", Some("v114"), None, &["_v30", "_v29"]),
            op("add", Some("v118"), None, &["_v29", "v117"]),
            op("store_var", None, Some("_bb7_arg0"), &["v118"]),
            op("store_var", None, Some("_bb7_arg1"), &["v114"]),
            op_v("jump", None, None, &[], 18),
        ],
    )
}
