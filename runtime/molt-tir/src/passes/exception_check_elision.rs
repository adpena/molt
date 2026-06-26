use crate::FunctionIR;

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_safe_exception_checks(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_EXC_ELIDE").is_ok() {
        return;
    }
    /// Operations that are guaranteed to never set the exception flag.
    const NEVER_RAISES: &[&str] = &[
        "inc_ref",
        "dec_ref",
        "dec_ref_obj",
        "inc_ref_obj",
        "const_int",
        "const_float",
        "const_bool",
        "const_none",
        "const_string",
        "nop",
        "line",
        "label",
        "state_label",
    ];
    let ops = &func_ir.ops;
    let len = ops.len();
    if len < 2 {
        return;
    }
    let mut remove = vec![false; len];
    for i in 1..len {
        if ops[i].kind != "check_exception" {
            continue;
        }
        // Walk backwards skipping nops, labels, and other non-raising ops
        // to find the "real" predecessor.
        let mut pred_idx = i - 1;
        while pred_idx > 0
            && matches!(
                ops[pred_idx].kind.as_str(),
                "nop" | "line" | "label" | "state_label"
            )
        {
            pred_idx -= 1;
        }
        let pred_kind = ops[pred_idx].kind.as_str();
        if NEVER_RAISES.contains(&pred_kind) {
            remove[i] = true;
        }
    }
    let count = remove.iter().filter(|&&r| r).count();
    if count > 0 {
        let mut new_ops = Vec::with_capacity(len - count);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if !remove[i] {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
}
