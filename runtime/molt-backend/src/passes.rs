use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{HashMap, HashSet};

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn elide_dead_struct_allocs(func_ir: &mut FunctionIR) {
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

    let mut uses_by_name: HashMap<&str, Vec<(usize, usize, &str)>> = HashMap::new();
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

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
const INLINE_OP_LIMIT: usize = 30;

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
fn is_inlineable(func: &FunctionIR, defined_functions: &HashSet<&str>) -> bool {
    if func.ops.len() > INLINE_OP_LIMIT {
        return false;
    }
    for op in &func.ops {
        match op.kind.as_str() {
            "loop_index_start" | "loop_index_end" | "loop_start" | "loop_end"
            | "for_iter_start" | "for_iter_end" | "while_start" | "while_end" | "try_start"
            | "try_end" | "except" | "finally" | "yield" | "yield_from" | "await"
            | "async_for_start" | "ASYNCGEN_NEW" | "GENERATOR_NEW" | "COROUTINE_NEW" => {
                return false;
            }
            "call_internal" => {
                if let Some(target) = op.s_value.as_deref()
                    && defined_functions.contains(target)
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn inline_functions(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_INLINING").is_ok() {
        return;
    }
    let limit: usize = std::env::var("MOLT_INLINE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(INLINE_OP_LIMIT);

    let defined_functions: HashSet<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();

    let mut inlineable: HashMap<String, (Vec<String>, Vec<OpIR>)> = HashMap::new();
    for func in &ir.functions {
        let func_copy = FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops: func.ops.clone(),
            param_types: func.param_types.clone(),
        };
        if func_copy.ops.len() <= limit && is_inlineable(&func_copy, &defined_functions) {
            inlineable.insert(
                func_copy.name.clone(),
                (func_copy.params.clone(), func_copy.ops),
            );
        }
    }

    if inlineable.is_empty() {
        return;
    }

    let mut inline_counter = 0u64;

    for func_ir in &mut ir.functions {
        let mut new_ops: Vec<OpIR> = Vec::with_capacity(func_ir.ops.len());
        let mut changed = false;

        for op in &func_ir.ops {
            if op.kind != "call_internal" {
                new_ops.push(op.clone());
                continue;
            }
            let target_name = match op.s_value.as_deref() {
                Some(name) => name,
                None => {
                    new_ops.push(op.clone());
                    continue;
                }
            };
            let Some((callee_params, callee_ops)) = inlineable.get(target_name) else {
                new_ops.push(op.clone());
                continue;
            };
            let call_args = match op.args.as_ref() {
                Some(args) => args,
                None => {
                    new_ops.push(op.clone());
                    continue;
                }
            };
            let call_out = match op.out.as_deref() {
                Some(out) => out.to_string(),
                None => {
                    new_ops.push(op.clone());
                    continue;
                }
            };

            inline_counter += 1;
            let prefix = format!(
                "_inl{}_{}_",
                inline_counter,
                target_name.replace(|c: char| !c.is_alphanumeric(), "_")
            );

            let mut rename_map: HashMap<String, String> = HashMap::new();
            for (i, param) in callee_params.iter().enumerate() {
                if i < call_args.len() {
                    rename_map.insert(param.clone(), call_args[i].clone());
                }
            }

            for callee_op in callee_ops {
                if callee_op.kind == "ret" || callee_op.kind == "ret_void" {
                    if callee_op.kind == "ret"
                        && let Some(ret_var) = callee_op.var.as_deref()
                    {
                        let renamed = rename_map
                            .get(ret_var)
                            .cloned()
                            .unwrap_or_else(|| format!("{prefix}{ret_var}"));
                        new_ops.push(OpIR {
                            kind: "copy".to_string(),
                            value: None,
                            f_value: None,
                            s_value: None,
                            bytes: None,
                            var: None,
                            args: Some(vec![renamed]),
                            out: Some(call_out.clone()),
                            fast_int: None,
                            fast_float: None,
                            raw_int: None,
                            stack_eligible: None,
                            task_kind: None,
                            container_type: None,
                            type_hint: None,
                        });
                    }
                    continue;
                }

                let mut inlined_op = callee_op.clone();

                if let Some(out) = inlined_op.out.clone() {
                    let renamed = rename_map
                        .get(&out)
                        .cloned()
                        .unwrap_or_else(|| format!("{prefix}{out}"));
                    inlined_op.out = Some(renamed.clone());
                    rename_map.entry(out).or_insert(renamed);
                }

                if let Some(ref args) = inlined_op.args {
                    inlined_op.args = Some(
                        args.iter()
                            .map(|a| {
                                rename_map
                                    .get(a)
                                    .cloned()
                                    .unwrap_or_else(|| format!("{prefix}{a}"))
                            })
                            .collect(),
                    );
                }

                if let Some(ref var) = inlined_op.var {
                    inlined_op.var = Some(
                        rename_map
                            .get(var)
                            .cloned()
                            .unwrap_or_else(|| format!("{prefix}{var}")),
                    );
                }

                new_ops.push(inlined_op);
            }

            changed = true;
        }

        if changed {
            func_ir.ops = new_ops;
        }
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn apply_profile_order(ir: &mut SimpleIR) {
    let Some(profile) = ir.profile.as_ref() else {
        return;
    };
    if profile.hot_functions.is_empty() {
        return;
    }
    let mut ranks: HashMap<String, usize> = HashMap::new();
    for (idx, name) in profile.hot_functions.iter().enumerate() {
        ranks.entry(name.clone()).or_insert(idx);
    }
    let mut original: HashMap<String, usize> = HashMap::new();
    for (idx, func) in ir.functions.iter().enumerate() {
        original.entry(func.name.clone()).or_insert(idx);
    }
    ir.functions.sort_by(|left, right| {
        let left_rank = ranks.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_rank = ranks.get(&right.name).copied().unwrap_or(usize::MAX);
        if left_rank != right_rank {
            return left_rank.cmp(&right_rank);
        }
        let left_idx = original.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_idx = original.get(&right.name).copied().unwrap_or(usize::MAX);
        left_idx
            .cmp(&right_idx)
            .then_with(|| left.name.cmp(&right.name))
    });
}
