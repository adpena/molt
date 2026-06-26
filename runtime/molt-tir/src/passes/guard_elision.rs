use crate::FunctionIR;
use std::collections::HashSet;

pub fn eliminate_redundant_guard_tags(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_GUARD_ELIM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len == 0 {
        return;
    }

    // Collect names of all values produced by ops that guarantee a
    // correct NaN-box tag by construction.
    let mut typed_outputs: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        let guaranteed = matches!(
            op.kind.as_str(),
            "const"
                | "const_int"
                | "const_float"
                | "const_bool"
                | "const_none"
                | "const_str"
                | "const_bytes"
                | "add"
                | "sub"
                | "mul"
                | "div"
                | "floordiv"
                | "mod"
                | "pow"
                | "neg"
                | "unary_neg"
                | "lt"
                | "le"
                | "gt"
                | "ge"
                | "eq"
                | "ne"
                | "is"
                | "and"
                | "or"
                | "not"
                | "band"
                | "bor"
                | "bxor"
                | "lshift"
                | "rshift"
                | "invert"
                | "list_new"
                | "tuple_new"
                | "dict_new"
                | "set_new"
                | "list_getitem"
                | "tuple_getitem"
                | "dict_getitem"
        );
        if guaranteed && let Some(out) = op.out.as_ref() {
            typed_outputs.insert(out.clone());
        }
    }

    let mut remove = vec![false; len];
    for (idx, op) in ops.iter().enumerate() {
        if op.kind != "guard_tag" && op.kind != "guard_type" {
            continue;
        }
        let args = match op.args.as_ref() {
            Some(args) if !args.is_empty() => args,
            _ => continue,
        };
        // If the value being guarded is provably typed, remove the guard.
        if typed_outputs.contains(&args[0]) {
            remove[idx] = true;
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
