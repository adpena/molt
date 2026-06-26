use crate::FunctionIR;
use std::collections::BTreeMap;

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_dead_struct_allocs(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_STRUCT_ELIDE").is_ok() {
        return;
    }
    let mut remove = vec![false; func_ir.ops.len()];
    let alloc_kinds = ["alloc_class", "alloc_class_trusted", "alloc_class_static"];
    let allowed_use_kinds = [
        "store",
        "store_init",
        "guarded_field_set",
        "guarded_field_init",
        "object_set_class",
    ];

    let mut uses_by_name: BTreeMap<&str, Vec<(usize, usize, &str)>> = BTreeMap::new();
    for (use_idx, use_op) in func_ir.ops.iter().enumerate() {
        let Some(args) = use_op.args.as_ref() else {
            continue;
        };
        let kind = use_op.kind.as_str();
        for (pos, arg) in args.iter().enumerate() {
            uses_by_name
                .entry(arg.as_str())
                .or_default()
                .push((use_idx, pos, kind));
        }
    }

    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !alloc_kinds.contains(&op.kind.as_str()) {
            continue;
        }
        let Some(out_name) = op.out.as_deref() else {
            continue;
        };
        let Some(uses) = uses_by_name.get(out_name) else {
            remove[idx] = true;
            continue;
        };
        let mut allowed = true;
        for &(_, pos, use_kind) in uses {
            if pos != 0 || !allowed_use_kinds.contains(&use_kind) {
                allowed = false;
                break;
            }
        }
        if allowed {
            remove[idx] = true;
            for &(use_idx, _, _) in uses {
                remove[use_idx] = true;
            }
        }
    }

    if remove.iter().any(|&flag| flag) {
        let mut new_ops = Vec::with_capacity(func_ir.ops.len());
        for (idx, op) in func_ir.ops.iter().enumerate() {
            if !remove[idx] {
                new_ops.push(op.clone());
            }
        }
        func_ir.ops = new_ops;
    }
}
