use crate::{FunctionIR, OpIR};

/// Eliminate redundant `guard_tag` ops on typed float/int variables.
///
/// `guard_tag(val, expected_tag)` calls `molt_guard_type` — a runtime
/// function call — to assert the NaN-boxing tag matches. For variables
/// that are provably typed (the result of `const_float`, `const`,
/// float/int arithmetic, or loaded from a typed `store_var` chain),
/// the tag is guaranteed correct and the guard is dead weight.
///
/// In the mandelbrot inner loop, two `guard_tag` ops per iteration add
/// two unnecessary function calls.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
/// Count every textual use of a value name across an op's `args`, `var`, and
/// (deliberately excluded) `out`.  Used by `fuse_method_dispatch` to prove a
/// getattr / callargs temporary is single-use before fusing it away.
fn fuse_count_value_reads(ops: &[OpIR], name: &str) -> usize {
    let mut n = 0usize;
    for op in ops {
        if let Some(args) = &op.args {
            for a in args {
                if a == name {
                    n += 1;
                }
            }
        }
        if op.var.as_deref() == Some(name) {
            n += 1;
        }
    }
    n
}

/// Fuse the `obj.method(args...)` dispatch idiom into a single `call_method_ic`
/// op (the CPython `LOAD_METHOD`/`CALL_METHOD` optimisation).
///
/// The frontend lowers a user-method call on a same-module instance to:
///
/// ```text
/// get_attr_generic_ptr  out=T   args=[recv]  s_value=<method>   # alloc bound method
/// (check_exception/line/nop ...)
/// callargs_new          out=CA                                  # alloc callargs
/// callargs_push_pos     out=_   args=[CA, a0]
/// callargs_push_pos     out=_   args=[CA, a1] ...
/// call_bind             out=R   args=[T, CA]                    # generic dispatch
/// ```
///
/// Both `get_attr_generic_ptr` (bound-method alloc) and `callargs_new`
/// (callargs alloc) recur every call.  This pass rewrites the quartet to:
///
/// ```text
/// call_method_ic        out=R   args=[recv, a0, a1, ...]  s_value=<method>
/// ```
///
/// which lowers to a single allocation-free runtime call (`molt_call_method_icN`).
///
/// SOUNDNESS (each is required before fusing; otherwise the site is left as-is):
///   * `T` (the getattr result) is referenced by EXACTLY this `call_bind` and
///     nowhere else — proven by a whole-function read count.
///   * `CA` (the callargs) is referenced ONLY by its `callargs_push_pos` chain
///     and this `call_bind` — no `callargs_push_kw`, no escape.
///   * Every `callargs_push_pos` for `CA` lies between `callargs_new` and
///     `call_bind` with no intervening control-flow boundary (label/jump/br_if/
///     loop_*/ret/raise), so positional order is preserved.
///   * The `get_attr_generic_ptr` has a single recv arg and an `s_value` method
///     name (it is a method getattr, not a field/dunder access shape).
///
/// The runtime op reproduces getattr+call semantics including all descriptor /
/// instance-shadow / `__getattribute__` fallbacks, so behaviour is preserved.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn fuse_method_dispatch(func_ir: &mut FunctionIR) {
    fuse_method_dispatch_inner(func_ir, std::env::var("MOLT_DISABLE_METHOD_FUSION").is_ok())
}

/// [`fuse_method_dispatch`] with the disable lever explicitly controlled
/// (rather than read from the process-global env), so tests can force it
/// deterministically without racing other parallel tests — `set_var` in one
/// test flips the gate under every concurrently-running test (the
/// poisoned-env-lock / flaky-fusion-test class).
pub(super) fn fuse_method_dispatch_inner(func_ir: &mut FunctionIR, disabled: bool) {
    if disabled {
        return;
    }
    let len = func_ir.ops.len();
    if len < 3 {
        return;
    }

    fn is_control_boundary(kind: &str) -> bool {
        matches!(
            kind,
            "label"
                | "state_label"
                | "jump"
                | "br_if"
                | "if"
                | "else"
                | "end_if"
                | "phi"
                | "loop_start"
                | "loop_end"
                | "loop_continue"
                | "loop_break"
                | "loop_break_if_true"
                | "loop_break_if_false"
                | "loop_break_if_exception"
                | "ret"
                | "raise"
        )
    }

    // Map each value name to the op index that defines it (its `out`).
    let mut def_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, op) in func_ir.ops.iter().enumerate() {
        if let Some(out) = &op.out
            && out != "none"
        {
            def_idx.insert(out.clone(), i);
        }
    }

    // remove[i]=true => drop op i; replace[i]=Some(op) => substitute op i.
    let mut remove = vec![false; len];
    let mut replacement: Vec<Option<OpIR>> = (0..len).map(|_| None).collect();

    for idx in 0..len {
        if func_ir.ops[idx].kind != "call_bind" {
            continue;
        }
        let call = &func_ir.ops[idx];
        let Some(call_args) = call.args.as_ref() else {
            continue;
        };
        if call_args.len() != 2 {
            continue;
        }
        let callee_name = call_args[0].clone();
        let callargs_name = call_args[1].clone();

        // The callee must be defined by a method `get_attr_generic_ptr`.
        let Some(&getattr_idx) = def_idx.get(&callee_name) else {
            continue;
        };
        let getattr = &func_ir.ops[getattr_idx];
        if getattr.kind != "get_attr_generic_ptr" {
            continue;
        }
        let Some(getattr_args) = getattr.args.as_ref() else {
            continue;
        };
        if getattr_args.len() != 1 {
            continue;
        }
        let Some(method_name) = getattr.s_value.clone() else {
            continue;
        };
        let recv_name = getattr_args[0].clone();

        // The callargs must be defined by a `callargs_new`.
        let Some(&callargs_new_idx) = def_idx.get(&callargs_name) else {
            continue;
        };
        if func_ir.ops[callargs_new_idx].kind != "callargs_new" {
            continue;
        }
        if callargs_new_idx >= idx || getattr_idx >= idx {
            continue;
        }

        // The callee temporary must be single-use (this call_bind only).
        if fuse_count_value_reads(&func_ir.ops, &callee_name) != 1 {
            continue;
        }

        // Collect the positional pushes for this callargs builder, in order,
        // and confirm the builder is used ONLY by its pushes and this call.
        let mut arg_names: Vec<String> = Vec::new();
        let mut push_indices: Vec<usize> = Vec::new();
        let mut callargs_extra_use = false;
        let mut ok = true;
        for (j, op) in func_ir.ops.iter().enumerate() {
            if j == callargs_new_idx || j == idx {
                continue;
            }
            let uses_ca = op
                .args
                .as_ref()
                .is_some_and(|a| a.iter().any(|x| x == &callargs_name))
                || op.var.as_deref() == Some(&callargs_name);
            if !uses_ca {
                continue;
            }
            if op.kind == "callargs_push_pos" {
                // Must be inside the new..call window so order is preserved.
                if j <= callargs_new_idx || j >= idx {
                    ok = false;
                    break;
                }
                let a = op.args.as_ref().unwrap();
                if a.len() != 2 || a[0] != callargs_name {
                    ok = false;
                    break;
                }
                push_indices.push(j);
                arg_names.push(a[1].clone());
            } else {
                // Any other consumer (push_kw, expand_star, escape) => bail.
                callargs_extra_use = true;
                break;
            }
        }
        if !ok || callargs_extra_use {
            continue;
        }
        // No control-flow boundary may sit between callargs_new and call_bind,
        // or positional ordering could differ at runtime.
        if (callargs_new_idx + 1..idx).any(|k| is_control_boundary(func_ir.ops[k].kind.as_str())) {
            continue;
        }
        // The fast path family covers 0..=4 positional args; higher arity keeps
        // the legacy lowering (no regression).
        if arg_names.len() > 4 {
            continue;
        }

        // Build the fused op: args = [recv, a0, a1, ...].
        let mut fused_args = Vec::with_capacity(1 + arg_names.len());
        fused_args.push(recv_name);
        fused_args.extend(arg_names);
        let mut fused = OpIR {
            kind: "call_method_ic".to_string(),
            ..Default::default()
        };
        fused.out = func_ir.ops[idx].out.clone();
        fused.args = Some(fused_args);
        fused.s_value = Some(method_name);
        fused.inherit_source_site_from(&func_ir.ops[idx]);

        replacement[idx] = Some(fused);
        remove[getattr_idx] = true;
        remove[callargs_new_idx] = true;
        for p in push_indices {
            remove[p] = true;
        }
    }

    // ── super().method(args) — fuse super_new + get_attr_generic_obj +
    //    callargs + call_indirect into a single `call_super_method_ic`. ──
    for idx in 0..len {
        if remove[idx] || replacement[idx].is_some() {
            continue;
        }
        if func_ir.ops[idx].kind != "call_indirect" {
            continue;
        }
        let call = &func_ir.ops[idx];
        let Some(call_args) = call.args.as_ref() else {
            continue;
        };
        if call_args.len() != 2 {
            continue;
        }
        let callee_name = call_args[0].clone();
        let callargs_name = call_args[1].clone();

        // Callee must be `get_attr_generic_obj(super_obj)` with a method name.
        let Some(&getattr_idx) = def_idx.get(&callee_name) else {
            continue;
        };
        if remove[getattr_idx] {
            continue;
        }
        let getattr = &func_ir.ops[getattr_idx];
        if getattr.kind != "get_attr_generic_obj" {
            continue;
        }
        let Some(getattr_args) = getattr.args.as_ref() else {
            continue;
        };
        if getattr_args.len() != 1 {
            continue;
        }
        let Some(method_name) = getattr.s_value.clone() else {
            continue;
        };
        let super_name = getattr_args[0].clone();

        // The super object must come from `super_new(class, self)`.
        let Some(&super_idx) = def_idx.get(&super_name) else {
            continue;
        };
        if remove[super_idx] {
            continue;
        }
        let super_op = &func_ir.ops[super_idx];
        if super_op.kind != "super_new" {
            continue;
        }
        let Some(super_args) = super_op.args.as_ref() else {
            continue;
        };
        if super_args.len() != 2 {
            continue;
        }
        let class_name = super_args[0].clone();
        let self_name = super_args[1].clone();

        // Callargs must come from a callargs_new and be used only by its pushes.
        let Some(&callargs_new_idx) = def_idx.get(&callargs_name) else {
            continue;
        };
        if remove[callargs_new_idx] || func_ir.ops[callargs_new_idx].kind != "callargs_new" {
            continue;
        }
        if super_idx >= idx || getattr_idx >= idx || callargs_new_idx >= idx {
            continue;
        }

        // The getattr result AND the super object must each be single-use.
        if fuse_count_value_reads(&func_ir.ops, &callee_name) != 1 {
            continue;
        }
        if fuse_count_value_reads(&func_ir.ops, &super_name) != 1 {
            continue;
        }

        let mut arg_names: Vec<String> = Vec::new();
        let mut push_indices: Vec<usize> = Vec::new();
        let mut bail = false;
        for (j, op) in func_ir.ops.iter().enumerate() {
            if j == callargs_new_idx || j == idx {
                continue;
            }
            let uses_ca = op
                .args
                .as_ref()
                .is_some_and(|a| a.iter().any(|x| x == &callargs_name))
                || op.var.as_deref() == Some(&callargs_name);
            if !uses_ca {
                continue;
            }
            if op.kind == "callargs_push_pos" {
                if j <= callargs_new_idx || j >= idx {
                    bail = true;
                    break;
                }
                let a = op.args.as_ref().unwrap();
                if a.len() != 2 || a[0] != callargs_name {
                    bail = true;
                    break;
                }
                push_indices.push(j);
                arg_names.push(a[1].clone());
            } else {
                bail = true;
                break;
            }
        }
        if bail || arg_names.len() > 4 {
            continue;
        }
        if (callargs_new_idx + 1..idx).any(|k| is_control_boundary(func_ir.ops[k].kind.as_str())) {
            continue;
        }

        // Build: call_super_method_ic  args=[class, self, a0, ...]  s_value=M.
        let mut fused_args = Vec::with_capacity(2 + arg_names.len());
        fused_args.push(class_name);
        fused_args.push(self_name);
        fused_args.extend(arg_names);
        let mut fused = OpIR {
            kind: "call_super_method_ic".to_string(),
            ..Default::default()
        };
        fused.out = func_ir.ops[idx].out.clone();
        fused.args = Some(fused_args);
        fused.s_value = Some(method_name);
        fused.inherit_source_site_from(&func_ir.ops[idx]);

        replacement[idx] = Some(fused);
        remove[super_idx] = true;
        remove[getattr_idx] = true;
        remove[callargs_new_idx] = true;
        for p in push_indices {
            remove[p] = true;
        }
    }

    if remove.iter().any(|&r| r) || replacement.iter().any(|r| r.is_some()) {
        let mut new_ops = Vec::with_capacity(len);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if remove[i] {
                continue;
            }
            if let Some(rep) = replacement[i].take() {
                new_ops.push(rep);
            } else {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
}
