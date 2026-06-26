use super::{RustBackend, rust_ident};
use crate::OpIR;
use std::collections::BTreeSet;

impl RustBackend {
    // ── Op emission ───────────────────────────────────────────────────────────

    fn op_prefers_integer_runtime_lane(&self, op: &OpIR) -> bool {
        self.current_scalar_plan
            .as_ref()
            .is_some_and(|plan| plan.op_prefers_integer_runtime_lane(op))
    }

    pub(super) fn emit_op(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let _is_hoisted = |name: &str| self.hoisted_vars.contains(name);
        if let Some(name) = op.out.as_deref() {
            let out_name = rust_ident(name);
            self.clear_alias(&out_name);
        }

        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        match op.kind.as_str() {
            // ── Constants ──────────────────────────────────────────────────────
            "const" | "int_const" => {
                let o = out();
                let rhs = if let Some(v) = op.value {
                    format!("MoltValue::Int({v})")
                } else if let Some(f) = op.f_value {
                    format!("MoltValue::Float({f:.17})")
                } else if let Some(ref s) = op.s_value {
                    format!("MoltValue::Str({}.to_string())", rust_string_literal(s))
                } else {
                    "MoltValue::None".to_string()
                };
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_float" => {
                let o = out();
                let f = op.f_value.unwrap_or(0.0);
                let rhs = format!("MoltValue::Float({f:.17})");
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_str" | "string_const" => {
                let o = out();
                let s = op.s_value.as_deref().unwrap_or("");
                let rhs = format!("MoltValue::Str({}.to_string())", rust_string_literal(s));
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_bool" | "bool_const" => {
                let o = out();
                let b = op.value.unwrap_or(0) != 0;
                let rhs = format!("MoltValue::Bool({b})");
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_none" | "none_const" => {
                let o = out();
                self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
            }
            "const_bytes" => {
                let o = out();
                let s = op.s_value.as_deref().unwrap_or("");
                let rhs = format!("MoltValue::Str({}.to_string())", rust_string_literal(s));
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_bigint" => {
                let o = out();
                let s = op.s_value.as_deref().unwrap_or("0");
                let rhs = format!(
                    "MoltValue::Int({}.parse::<i64>().unwrap_or(0))",
                    rust_string_literal(s)
                );
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_not_implemented" | "const_ellipsis" => {
                let o = out();
                self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                // no comment needed — #![allow(unused)] covers it
            }
            "box" | "box_from_raw_int" => {
                let o = out();
                let rhs = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .map(|src| rust_clone(src))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }

            // ── Variable access ────────────────────────────────────────────────
            "load_local" => {
                let o = out();
                let v = var_ref(op);
                self.emit_line(&declare(
                    &o,
                    &format!("{v}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_alias(o, v);
            }
            "load_var" | "copy_var" => {
                let o = out();
                let v = var_ref(op);
                self.emit_line(&declare(
                    &o,
                    &format!("{v}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_alias(o, v);
            }
            "store_var" => {
                if let Some(name) = op.var.as_deref().or(op.out.as_deref()) {
                    let dst = rust_ident(name);
                    self.clear_alias(&dst);
                    let rhs = op
                        .args
                        .as_deref()
                        .and_then(|args| args.first())
                        .map(|src| rust_clone(src))
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&format!("{dst} = {rhs};"));
                }
            }
            "load" | "guarded_load" => {
                let o = out();
                if let Some(obj) = op.args.as_ref().and_then(|a| a.first()) {
                    let obj = rust_value(obj);
                    let slot_key = rust_slot_key(op.value.unwrap_or(0));
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_item(&{obj}, &{slot_key})"),
                        &self.hoisted_vars.clone(),
                    ));
                    let alias_key = format!("__alias_key_{o}");
                    self.emit_line(&declare(
                        &alias_key,
                        &format!("{slot_key}.clone()"),
                        &self.hoisted_vars.clone(),
                    ));
                    self.note_indexed_alias(o, obj, alias_key);
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "closure_load" => {
                let o = out();
                let slot = op
                    .args
                    .as_ref()
                    .and_then(|a| a.first())
                    .map(|s| format!("__closure_{}", rust_ident(s)))
                    .unwrap_or_else(|| var_ref(op));
                self.emit_line(&declare(
                    &o,
                    &format!("{slot}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_alias(o, slot);
            }
            "store_local" => {
                let v = var_ref(op);
                if let Some(src) = op.args.as_ref().and_then(|a| a.first()) {
                    let s = rust_ident(src);
                    self.emit_line(&format!("{v} = {s}.clone();"));
                    self.note_alias(v, s);
                } else {
                    self.clear_alias(&v);
                }
            }
            "store" | "store_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_ident(&args[0]);
                    let value = rust_clone(&args[1]);
                    let slot_key = rust_slot_key(op.value.unwrap_or(0));
                    if is_assignable_var(&obj) {
                        self.emit_line(&format!("molt_set_item(&mut {obj}, {slot_key}, {value});"));
                        self.emit_alias_writeback(&obj);
                    }
                }
            }
            "closure_store" => {
                if let Some(args) = &op.args
                    && args.len() >= 2
                {
                    let slot = format!("__closure_{}", rust_ident(&args[0]));
                    let src = rust_ident(&args[1]);
                    self.emit_line(&format!("{slot} = {src}.clone();"));
                }
            }
            "phi" => {
                // Phi nodes are handled by the hoisting logic above; skip here.
            }

            // ── Arithmetic ─────────────────────────────────────────────────────
            "add" | "inplace_add" | "binop_add" => {
                let o = out();
                let (a, b) = args2(op);
                if self.op_prefers_integer_runtime_lane(op) {
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Int(molt_int(&{a}).wrapping_add(molt_int(&{b})))"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_add({a}.clone(), {b}.clone())"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "sub" | "inplace_sub" | "binop_sub" => {
                let o = out();
                let (a, b) = args2(op);
                if self.op_prefers_integer_runtime_lane(op) {
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Int(molt_int(&{a}).wrapping_sub(molt_int(&{b})))"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_sub({a}.clone(), {b}.clone())"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "mul" | "inplace_mul" | "binop_mul" => {
                let o = out();
                let (a, b) = args2(op);
                if self.op_prefers_integer_runtime_lane(op) {
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Int(molt_int(&{a}).wrapping_mul(molt_int(&{b})))"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_mul({a}.clone(), {b}.clone())"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "div" | "true_div" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_div({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "floor_div" | "floordiv" | "binop_floor_div" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_floor_div({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "mod" | "modulo" | "binop_mod" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_mod({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "pow" | "binop_pow" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_pow({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "neg" | "unary_neg" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_neg({a}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "unary_not" | "not" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(!molt_bool(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // Bitwise
            "band" | "bit_and" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) & molt_int(&{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "bor" | "bit_or" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) | molt_int(&{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "bxor" | "bit_xor" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) ^ molt_int(&{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "lshift" | "shl" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) << (molt_int(&{b}) as u32))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "rshift" | "shr" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) >> (molt_int(&{b}) as u32))"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Comparisons ────────────────────────────────────────────────────
            "eq" | "cmp_eq" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_eq(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ne" | "cmp_ne" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(!molt_eq(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "lt" | "cmp_lt" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_lt(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "le" | "cmp_le" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_le(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "gt" | "cmp_gt" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_gt(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ge" | "cmp_ge" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_ge(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "is" | "is_not" => {
                // Python `is` — identity check (use == for value equality in Rust)
                let o = out();
                let (a, b) = args2(op);
                let negate = op.kind == "is_not";
                let cmp = if negate { "!" } else { "" };
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool({cmp}molt_eq(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "in" | "not_in" => {
                let o = out();
                let (a, b) = args2(op);
                let negate = op.kind == "not_in";
                let prefix = if negate { "!" } else { "" };
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool({prefix}molt_in(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "contains" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = rust_ident(&args[0]);
                    let value = rust_ident(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Bool(molt_in(&{value}, &{container}))"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }

            // ── Boolean logic ──────────────────────────────────────────────────
            "and" | "_m_and" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("if !molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "or" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("if molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Control flow ───────────────────────────────────────────────────
            "if" | "branch_false" => {
                let cond = arg0(op);
                self.emit_line(&format!("if molt_bool(&{cond}) {{"));
                self.indent += 1;
            }
            "if_not" | "branch_true" => {
                let cond = arg0(op);
                self.emit_line(&format!("if !molt_bool(&{cond}) {{"));
                self.indent += 1;
            }
            "else" => {
                self.indent -= 1;
                self.emit_line("} else {");
                self.indent += 1;
            }
            "end_if" => {
                self.indent -= 1;
                self.emit_line("}");
            }
            "loop_start" | "while_start" => {
                self.emit_line("loop {");
                self.indent += 1;
            }
            "loop_end" | "while_end" => {
                self.indent -= 1;
                self.emit_line("}");
            }
            "loop_break_if_false" => {
                let cond = arg0(op);
                self.emit_line(&format!("if !molt_bool(&{cond}) {{ break; }}"));
            }
            "loop_break_if_true" => {
                let cond = arg0(op);
                self.emit_line(&format!("if molt_bool(&{cond}) {{ break; }}"));
            }
            "loop_break_if_exception" => {
                // Value-less exception-flag break: exit an iterator-consumer loop
                // when a runtime exception is pending (the producer returned the
                // None sentinel on a mid-iteration raise).  Reads the same
                // sacrosanct flag the runtime CHECK_EXCEPTION uses; the still
                // pending exception then rides up the lazy-return path.
                self.emit_line("if molt_exception_pending() != 0 { break; }");
            }
            "loop_break" => {
                self.emit_line("break;");
            }
            "loop_continue" | "loop_carry_update" | "loop_carry_init" => {
                self.emit_line("continue;");
            }
            "loop_index_next" => {
                // Update loop index — 1-arg: assign; 2-arg: add-step.
                // After updating the phi var, also write back to the locals frame slot
                // (if any) so that post-loop index reads see the correct value.
                if let Some(ref out_name) = op.out {
                    let o = rust_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    let new_val_expr = if args.len() >= 2 {
                        let current = rust_ident(&args[0]);
                        let step = rust_ident(&args[1]);
                        format!("molt_add({current}.clone(), {step}.clone())")
                    } else if let Some(new_val) = args.first() {
                        format!("{}.clone()", rust_ident(new_val))
                    } else {
                        String::new()
                    };
                    if !new_val_expr.is_empty() {
                        self.emit_line(&format!("{o} = {new_val_expr};"));
                        // Write the updated phi value back to the locals frame so
                        // post-loop `index` ops read the final (not stale) value.
                        if let Some((frame, slot)) = self.phi_to_frame.get(&o).cloned() {
                            self.emit_line(&format!(
                                "molt_set_item(&mut {frame}, {slot}.clone(), {o}.clone());"
                            ));
                        }
                    }
                }
            }
            "loop_index_start" => {
                // Initialization is handled in the loop preamble above; skip here.
            }
            "iter" => {
                let o = out();
                let src = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_iter(&{src})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "iter_next" => {
                let o = out();
                let iter_var = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_iter_next(&mut {iter_var})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "for_range" => {
                // for_range: args = [out_var, start, stop, step]
                let args = op.args.as_deref().unwrap_or(&[]);
                let iter_var = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "_".to_string());
                let start = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let stop = args
                    .get(2)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let step = args
                    .get(3)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(1)".to_string());
                // Emit as a while loop to keep MoltValue
                self.emit_line(&format!("{{ let mut __range_i = molt_int(&{start}); let __range_stop = molt_int(&{stop}); let __range_step = molt_int(&{step});"));
                self.emit_line("while (__range_step > 0 && __range_i < __range_stop) || (__range_step < 0 && __range_i > __range_stop) {");
                self.indent += 1;
                self.emit_line(&format!(
                    "let mut {iter_var}: MoltValue = MoltValue::Int(__range_i);"
                ));
            }
            "for_iter" => {
                // for_iter (comprehension-inlined): out = loop_var, args[0] = iterable.
                // The comprehension inliner in lib.rs always emits this convention.
                let iter_var = out();
                let iterable = arg0(op);
                self.emit_line(&format!("for {iter_var} in molt_iter_list(&{iterable}) {{"));
                self.indent += 1;
            }
            "range_new" => {
                // range_new(start, stop, step) — used by comprehension-inlined source_ops.
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let start = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let stop = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let step = args
                    .get(2)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(1)".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }
            "end_for" => {
                // Range loops open an extra block + while; make sure the index
                // advances before closing the while body.
                let closes_range = op.args.as_ref().is_some_and(|args| !args.is_empty());
                if closes_range {
                    self.emit_line("__range_i += __range_step;");
                }
                if self.indent > 0 {
                    self.indent -= 1;
                }
                self.emit_line("}");
                if closes_range {
                    if self.indent > 0 {
                        self.indent -= 1;
                    }
                    self.emit_line("}");
                }
            }
            "break" => {
                self.emit_line("break;");
            }
            "continue" => {
                self.emit_line("continue;");
            }

            // ── Return ─────────────────────────────────────────────────────────
            "return" | "ret" => {
                if self.current_is_main {
                    self.emit_param_writeback();
                    self.emit_line("return;");
                } else if let Some(val) = op.args.as_ref().and_then(|a| a.first()) {
                    let v = rust_ident(val);
                    self.emit_param_writeback();
                    self.emit_line(&format!("return {v}.clone();"));
                } else if let Some(ref v) = op.var {
                    let v = rust_ident(v);
                    self.emit_param_writeback();
                    self.emit_line(&format!("return {v}.clone();"));
                } else {
                    self.emit_param_writeback();
                    self.emit_line("return MoltValue::None;");
                }
            }
            "return_none" | "ret_none" | "ret_void" => {
                self.emit_param_writeback();
                if self.current_is_main {
                    self.emit_line("return;");
                } else {
                    self.emit_line("return MoltValue::None;");
                }
            }

            // ── Function calls ─────────────────────────────────────────────────
            "call" | "call_func" | "call_internal" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(ref fn_name) = op.s_value {
                    // Direct static call with mutable arg-vector writeback.
                    let fn_ident = rust_ident(fn_name);
                    let call_args: Vec<String> = args.iter().map(|a| rust_clone(a)).collect();
                    self.emit_line(&format!(
                        "let mut __call_args: Vec<MoltValue> = vec![{}];",
                        call_args.join(", ")
                    ));
                    self.emit_line(&format!(
                        "let mut __call_ret: MoltValue = {fn_ident}(&mut __call_args);"
                    ));
                    for (idx, arg) in args.iter().enumerate() {
                        let var = rust_ident(arg);
                        if is_assignable_var(&var) {
                            self.emit_line(&format!(
                                "{var} = __call_args.get({idx}).cloned().unwrap_or({var}.clone());"
                            ));
                            self.emit_alias_writeback(&var);
                        }
                    }
                    if o == "_" || o == "none" {
                        self.emit_line("__call_ret;");
                    } else {
                        self.emit_line(&declare(
                            &o,
                            "__call_ret.clone()",
                            &self.hoisted_vars.clone(),
                        ));
                    }
                } else if args.is_empty() {
                    if o == "_" || o == "none" {
                        self.emit_line("MoltValue::None;");
                    } else {
                        self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                    }
                } else {
                    // Dynamic call: args[0] is the MoltValue::Func to invoke.
                    let func_var = rust_ident(&args[0]);
                    let call_args: Vec<String> = args[1..].iter().map(|a| rust_clone(a)).collect();
                    self.emit_line(&format!(
                        "let mut __call_args: Vec<MoltValue> = vec![{}];",
                        call_args.join(", ")
                    ));
                    self.emit_line(&format!(
                        "let mut __call_ret: MoltValue = molt_call(&{func_var}, &mut __call_args);"
                    ));
                    for (idx, arg) in args[1..].iter().enumerate() {
                        let var = rust_ident(arg);
                        if is_assignable_var(&var) {
                            self.emit_line(&format!(
                                "{var} = __call_args.get({idx}).cloned().unwrap_or({var}.clone());"
                            ));
                            self.emit_alias_writeback(&var);
                        }
                    }
                    if o == "_" || o == "none" {
                        self.emit_line("__call_ret;");
                    } else {
                        self.emit_line(&declare(
                            &o,
                            "__call_ret.clone()",
                            &self.hoisted_vars.clone(),
                        ));
                    }
                }
            }
            "call_method" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                // args: [obj, arg0, arg1, ...]; s_value carries the method name.
                let obj = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "_".to_string());
                let method = op.s_value.as_deref().unwrap_or("");
                let call_args: Vec<String> = args[1..].iter().map(|a| rust_clone(a)).collect();
                if method == "append" {
                    let arg = call_args
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&format!("molt_list_append(&mut {obj}, {arg});"));
                    self.emit_alias_writeback(&obj);
                    if o != "_" && o != "none" {
                        self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                    }
                } else {
                    let rhs = match method {
                        "keys" => format!("molt_dict_keys(&{obj})"),
                        "values" => format!("molt_dict_values(&{obj})"),
                        "items" => format!("molt_dict_items(&{obj})"),
                        "get" => {
                            let key = call_args
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            let default = call_args
                                .get(1)
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            format!(
                                "{{ let __k = {key}; if let Some((_, v)) = if let MoltValue::Dict(d) = &{obj} {{ d.iter().find(|(k,_)| molt_eq(k, &__k)) }} else {{ None }} {{ v.clone() }} else {{ {default} }} }}"
                            )
                        }
                        _ => format!(
                            "/* MOLT_STUB: method {obj}.{method}({}) */ MoltValue::None",
                            call_args.join(", ")
                        ),
                    };
                    if o == "_" || o == "none" {
                        self.emit_line(&format!("{rhs};"));
                    } else {
                        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
                    }
                }
            }
            "call_bind" | "call_indirect" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let rhs = if args.len() >= 2 {
                    let func = rust_ident(&args[0]);
                    let builder = rust_ident(&args[1]);
                    let extra_args = args[2..]
                        .iter()
                        .map(|a| rust_clone(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let extra_stmt = if extra_args.is_empty() {
                        String::new()
                    } else {
                        format!("__call_args.extend(vec![{extra_args}]);")
                    };
                    format!(
                        "{{ let mut __call_args = Vec::new(); \
                           if let MoltValue::List(__pos) = &{builder} {{ \
                               __call_args.extend(__pos.iter().cloned()); \
                           }} else if !matches!({builder}, MoltValue::None) {{ \
                               __call_args.push({builder}.clone()); \
                           }} \
                           {extra_stmt} \
                           let __ret = molt_call(&{func}, &mut __call_args); \
                           __ret }}"
                    )
                } else if let Some(func) = args.first() {
                    format!(
                        "{{ let mut __call_args = Vec::new(); molt_call(&{}, &mut __call_args) }}",
                        rust_ident(func)
                    )
                } else {
                    "MoltValue::None".to_string()
                };
                if o == "_" || o == "none" {
                    self.emit_line(&format!("{rhs};"));
                } else {
                    self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
                }
            }
            "callargs_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let items = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::List(vec![{items}])"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "callargs_push_pos" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = rust_ident(&args[0]);
                    let val = rust_ident(&args[1]);
                    self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
                    self.emit_alias_writeback(&list);
                }
            }
            "callargs_expand_star" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = rust_ident(&args[0]);
                    let other = rust_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __item in molt_iter_list(&{other}) {{ molt_list_append(&mut {list}, __item); }}"
                    ));
                    self.emit_alias_writeback(&list);
                }
            }
            "callargs_push_kw" | "callargs_expand_kwstar" => {
                // Keyword arguments are currently ignored in the Rust subset.
            }
            "func_new" | "func_new_closure" => {
                let o = out();
                let rhs = if let Some(ref fn_name) = op.s_value {
                    let fn_ident = rust_ident(fn_name);
                    format!(
                        "MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {fn_ident}(args)))"
                    )
                } else {
                    "MoltValue::None".to_string()
                };
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "code_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 9 {
                    let filename = rust_ident(&args[0]);
                    let name = rust_ident(&args[1]);
                    let firstlineno = rust_ident(&args[2]);
                    let linetable = rust_ident(&args[3]);
                    let varnames = rust_ident(&args[4]);
                    let names = rust_ident(&args[5]);
                    let argcount = rust_ident(&args[6]);
                    let posonlyargcount = rust_ident(&args[7]);
                    let kwonlyargcount = rust_ident(&args[8]);
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "molt_code_new(&{filename}, &{name}, &{firstlineno}, &{linetable}, &{varnames}, &{names}, &{argcount}, &{posonlyargcount}, &{kwonlyargcount})"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "code_slots_init" => {
                let count = op.value.unwrap_or(0);
                self.emit_line(&format!("molt_code_slots_init({count});"));
            }
            "code_slot_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(code) = args.first() {
                    let code = rust_ident(code);
                    let code_id = op.value.unwrap_or(0);
                    self.emit_line(&format!("molt_code_slot_set({code_id}, &{code});"));
                }
            }
            "exception_last" | "exception_last_pending" | "exception_finally_pending_observer" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    let helper = if matches!(
                        op.kind.as_str(),
                        "exception_last_pending" | "exception_finally_pending_observer"
                    ) {
                        "molt_exception_last_pending()"
                    } else {
                        "molt_exception_last()"
                    };
                    self.emit_line(&declare(&o, helper, &self.hoisted_vars.clone()));
                }
            }
            "exception_stack_depth" | "exception_stack_enter" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    let helper = if op.kind == "exception_stack_enter" {
                        "molt_exception_stack_enter()"
                    } else {
                        "molt_exception_stack_depth()"
                    };
                    self.emit_line(&declare(&o, helper, &self.hoisted_vars.clone()));
                }
            }
            "exception_clear" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&declare(
                        &o,
                        "molt_exception_clear()",
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line("molt_exception_clear();");
                }
            }
            "exception_stack_exit" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let prev = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                self.emit_line(&format!("molt_exception_stack_exit(&{prev});"));
            }
            "exception_stack_set_depth" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let depth = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                self.emit_line(&format!("molt_exception_stack_set_depth(&{depth});"));
            }
            "exception_stack_clear" => {
                self.emit_line("molt_exception_stack_clear();");
            }
            "exception_set_last" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let exc = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&format!("molt_exception_set_last(&{exc});"));
            }
            "exception_active" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&declare(
                        &o,
                        "molt_exception_active()",
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "trace_enter_slot" => {
                let code_id = op.value.unwrap_or(0);
                self.emit_line(&format!("molt_trace_enter_slot({code_id});"));
            }
            "trace_exit" => {
                self.emit_line("molt_trace_exit();");
            }
            "frame_locals_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let locals = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&format!("molt_frame_locals_set(&{locals});"));
            }
            "builtin_func" => {
                let o = out();
                let builtin = op.s_value.as_deref().unwrap_or("");
                self.emit_line(&declare(
                    &o,
                    &format!("molt_builtin_func({})", rust_string_literal(builtin)),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Builtins ───────────────────────────────────────────────────────
            "print" | "builtin_print" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let arg_list = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("molt_print(&[{arg_list}]);"));
            }
            "len" | "builtin_len" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_len(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "int" | "cast_int" | "builtin_int" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "int_from_obj" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "int_from_str_of_obj" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let a = args
                    .first()
                    .map(|s| rust_value(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let base = args
                    .get(1)
                    .map(|s| rust_value(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let has_base = args
                    .get(2)
                    .map(|s| rust_value(s))
                    .unwrap_or_else(|| "MoltValue::Bool(false)".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "{{ let __s = molt_str(&{a}); if molt_bool(&{has_base}) {{ let __base = molt_int(&{base}); MoltValue::Int(if (2..=36).contains(&__base) {{ i64::from_str_radix(__s.trim(), __base as u32).unwrap_or(0) }} else {{ 0 }}) }} else {{ MoltValue::Int(molt_int(&MoltValue::Str(__s))) }} }}"
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }
            "float" | "cast_float" | "builtin_float" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Float(molt_float(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "float_from_obj" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Float(molt_float(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "str" | "cast_str" | "builtin_str" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Str(molt_str(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "bool" | "cast_bool" | "builtin_bool" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_bool(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "chr" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_chr(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ord" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_ord(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ord_at" => {
                let o = out();
                let (obj, key) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_ord_at(&{obj}, &{key})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "abs" | "builtin_abs" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_abs({a}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Collections ────────────────────────────────────────────────────
            "build_list" | "alloc" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let items = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::List(vec![{items}])"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "build_dict" | "dict_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                // args: [k0, v0, k1, v1, ...]
                let mut pairs = Vec::new();
                let mut i = 0;
                while i + 1 < args.len() {
                    let k = rust_ident(&args[i]);
                    let v = rust_ident(&args[i + 1]);
                    pairs.push(format!("({k}.clone(), {v}.clone())"));
                    i += 2;
                }
                let rhs = format!("MoltValue::Dict(vec![{}])", pairs.join(", "));
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "list_append" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = rust_ident(&args[0]);
                    let val = rust_ident(&args[1]);
                    self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
                    self.emit_alias_writeback(&list);
                }
            }
            "get_item" | "subscript" | "index" => {
                let o = out();
                let (obj, key) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_get_item(&{obj}, &{key})"),
                    &self.hoisted_vars.clone(),
                ));
                let alias_key = format!("__alias_key_{o}");
                self.emit_line(&declare(
                    &alias_key,
                    &format!("{key}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_indexed_alias(o, obj, alias_key);
            }
            "dict_get" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let obj = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let key = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                if let Some(default) = args.get(2) {
                    let default = rust_ident(default);
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "{{ let __v = molt_get_item(&{obj}, &{key}); if matches!(__v, MoltValue::None) {{ {default}.clone() }} else {{ __v }} }}"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_item(&{obj}, &{key})"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "set_item" | "store_subscript" | "store_index" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = rust_ident(&args[0]);
                    let key = rust_ident(&args[1]);
                    let val = rust_ident(&args[2]);
                    // Record phi→frame mapping so loop_index_next can write back.
                    self.phi_to_frame
                        .insert(val.clone(), (obj.clone(), key.clone()));
                    self.emit_line(&format!(
                        "molt_set_item(&mut {obj}, {key}.clone(), {val}.clone());"
                    ));
                    self.emit_alias_writeback(&obj);
                }
            }
            "dict_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = rust_ident(&args[0]);
                    let key = rust_ident(&args[1]);
                    let val = rust_ident(&args[2]);
                    self.emit_line(&format!(
                        "molt_set_item(&mut {obj}, {key}.clone(), {val}.clone());"
                    ));
                    self.emit_alias_writeback(&obj);
                }
            }
            "get_attr" | "load_attr" => {
                let o = out();
                let obj = arg0(op);
                let attr = op
                    .s_value
                    .as_deref()
                    .or_else(|| op.args.as_ref().and_then(|a| a.get(1)).map(|s| s.as_str()))
                    .unwrap_or("__unknown__");
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "molt_get_attr(&{obj}, {attr_lit})",
                        attr_lit = rust_string_literal(attr)
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }
            "get_attr_name" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_value(&args[0]);
                    let attr = rust_value(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_attr_name(&{obj}, &{attr})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "get_attr_name_default" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_value(&args[0]);
                    let attr = rust_value(&args[1]);
                    let default = args
                        .get(2)
                        .map(|name| rust_value(name))
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_attr_name_default(&{obj}, &{attr}, &{default})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "set_attr" | "store_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_ident(&args[0]);
                    let value_index = if args.len() >= 3 { 2 } else { 1 };
                    let value = rust_clone(&args[value_index]);
                    let attr = op
                        .s_value
                        .as_deref()
                        .or_else(|| args.get(1).map(|s| s.as_str()))
                        .unwrap_or("__unknown__");
                    if is_assignable_var(&obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {obj}, MoltValue::Str({attr_lit}.to_string()), {value});",
                            attr_lit = rust_string_literal(attr)
                        ));
                        self.emit_alias_writeback(&obj);
                    }
                }
            }

            // ── Enumerate / zip / sorted / reversed ────────────────────────────
            "enumerate" => {
                let o = out();
                let a = arg0(op);
                let start = op
                    .args
                    .as_ref()
                    .and_then(|a| a.get(1))
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!("molt_enumerate(&{a}, molt_int(&{start}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "zip" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_zip(&{a}, &{b})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "sorted" | "builtin_sorted" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_sorted(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "reversed" | "builtin_reversed" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_reversed(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "sum" | "builtin_sum" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_sum(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "any" | "builtin_any" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_any(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "all" | "builtin_all" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_all(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Range ──────────────────────────────────────────────────────────
            "range" | "builtin_range" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let (start, stop, step) = match args.len() {
                    1 => (
                        "MoltValue::Int(0)".to_string(),
                        rust_ident(&args[0]),
                        "MoltValue::Int(1)".to_string(),
                    ),
                    2 => (
                        rust_ident(&args[0]),
                        rust_ident(&args[1]),
                        "MoltValue::Int(1)".to_string(),
                    ),
                    _ => (
                        rust_ident(&args[0]),
                        rust_ident(&args[1]),
                        rust_ident(&args[2]),
                    ),
                };
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }

            "class_new" | "module_new" | "object_new" | "builtin_type" => {
                let o = out();
                self.emit_line(&declare(
                    &o,
                    "MoltValue::Dict(vec![])",
                    &self.hoisted_vars.clone(),
                ));
            }
            "bound_method_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let method = rust_value(&args[0]);
                    let obj = rust_value(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "{{ let __bound_method = {method}.clone(); let __bound_self = {obj}.clone(); MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {{ let mut __bound = vec![__bound_self.clone()]; __bound.extend(args.iter().cloned()); molt_call(&__bound_method, &mut __bound) }})) }}"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "alloc_class_static" | "alloc_class_trusted" | "alloc_class" => {
                let o = out();
                self.emit_line(&declare(
                    &o,
                    "MoltValue::Dict(vec![])",
                    &self.hoisted_vars.clone(),
                ));
            }
            "object_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_ident(&args[0]);
                    let class = rust_clone(&args[1]);
                    if is_assignable_var(&obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {obj}, MoltValue::Str(\"__class__\".to_string()), {class});"
                        ));
                        self.emit_alias_writeback(&obj);
                    }
                }
            }
            "class_set_base" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class_obj = rust_ident(&args[0]);
                    let base = rust_clone(&args[1]);
                    if is_assignable_var(&class_obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {class_obj}, MoltValue::Str(\"__base__\".to_string()), {base});"
                        ));
                        self.emit_alias_writeback(&class_obj);
                    }
                }
            }
            "class_set_layout_version" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class_obj = rust_ident(&args[0]);
                    let version = rust_clone(&args[1]);
                    if is_assignable_var(&class_obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {class_obj}, MoltValue::Str(\"__molt_layout_version__\".to_string()), {version});"
                        ));
                        self.emit_alias_writeback(&class_obj);
                    }
                }
            }
            "class_merge_layout" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let class_obj = rust_ident(&args[0]);
                    let offsets = rust_clone(&args[1]);
                    let size = rust_clone(&args[2]);
                    if is_assignable_var(&class_obj) {
                        let expr =
                            format!("molt_class_merge_layout(&mut {class_obj}, {offsets}, {size})");
                        if is_assignable_var(op.out.as_deref().unwrap_or("")) {
                            let o = out();
                            self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
                        } else {
                            self.emit_line(&format!("{expr};"));
                        }
                        self.emit_alias_writeback(&class_obj);
                    }
                }
            }
            "class_apply_set_name"
            | "class_layout_version"
            | "class_layout_field_count"
            | "class_layout_slot_count" => {}
            "module_cache_get" | "module_load_cached" => {
                let o = out();
                let name = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .map(|name| rust_value(name))
                    .or_else(|| {
                        op.s_value.as_deref().map(|name| {
                            format!("MoltValue::Str({}.to_string())", rust_string_literal(name))
                        })
                    })
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_module_cache_get(&{name})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&format!("molt_module_cache_get(&{name});"));
                }
            }
            "module_cache_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let name = rust_value(&args[0]);
                    let module = rust_clone(&args[1]);
                    let expr = format!("molt_module_cache_set(&{name}, {module})");
                    let o = out();
                    if o != "_" && o != "none" && !o.is_empty() {
                        self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
                    } else {
                        self.emit_line(&format!("{expr};"));
                    }
                }
            }
            "module_cache_del" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(name_arg) = args.first() {
                    let name = rust_value(name_arg);
                    let expr = format!("molt_module_cache_del(&{name})");
                    let o = out();
                    if o != "_" && o != "none" && !o.is_empty() {
                        self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
                    } else {
                        self.emit_line(&format!("{expr};"));
                    }
                }
            }
            "module_import" => {
                let o = out();
                let module = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .map(|name| rust_value(name))
                    .or_else(|| {
                        op.s_value.as_deref().map(|name| {
                            format!("MoltValue::Str({}.to_string())", rust_string_literal(name))
                        })
                    })
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!("molt_import_module(&{module})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "module_get_attr" | "module_import_from" | "module_get_name" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    let module = args
                        .first()
                        .map(|name| rust_value(name))
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "molt_get_attr_name(&{module}, &MoltValue::Str({}.to_string()))",
                            rust_string_literal(attr_str)
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                } else if args.len() >= 2 {
                    let module = rust_value(&args[0]);
                    let attr = rust_value(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_attr_name(&{module}, &{attr})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "module_set_attr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let module = rust_ident(&args[0]);
                    let attr = rust_clone(&args[1]);
                    let value = rust_clone(&args[2]);
                    if is_assignable_var(&module) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {module}, {attr}, {value});"
                        ));
                        self.emit_alias_writeback(&module);
                    }
                }
            }

            // ── No-ops / markers ───────────────────────────────────────────────
            "nop"
            | "comment"
            | "debug_label"
            | "line"
            | "type_assert"
            | "br_if"
            | "branch"
            | "alloc_task"
            | "block_on"
            | "asyncgen_locals_register"
            | "cancel_current"
            | "cancel_token_cancel"
            | "cancel_token_clone"
            | "cancel_token_drop"
            | "cancel_token_get_current"
            | "cancel_token_is_cancelled"
            | "cancel_token_new"
            | "cancel_token_set_current"
            | "cancelled"
            | "check_exception"
            | "ascii_from_obj"
            | "bridge_unavailable" => {
                // Stub: these ops may produce an output variable in the IR.
                // Declare it so downstream phi references compile.
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&format!(
                        "let mut {o}: MoltValue = /* MOLT_STUB: {} */ MoltValue::None;",
                        op.kind
                    ));
                }
            }
            "inc_ref" | "borrow" | "binding_alias" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if o != "_"
                    && o != "none"
                    && !o.is_empty()
                    && let Some(src) = args.first()
                {
                    let src = rust_clone(src);
                    self.emit_line(&declare(&o, &src, &self.hoisted_vars.clone()));
                }
            }
            "dec_ref" | "release" => {}

            // ── Class / instance stubs ─────────────────────────────────────────
            "alloc_instance" | "init_instance" | "instance_set_field" | "instance_get_field"
            | "instance_has_field" => {
                let o = out();
                if o != "_" && o != "none" {
                    self.emit_line(&declare(
                        &o,
                        "MoltValue::Dict(vec![])",
                        &self.hoisted_vars.clone(),
                    ));
                }
            }

            // ── Exception stubs ────────────────────────────────────────────────
            "raise" | "reraise" => {
                // In stub/native-Rust mode, Python exceptions cannot propagate
                // through the Rust call stack.  Instead of silently returning
                // None (which hides real errors), we panic with context so the
                // failure is immediately visible during testing.
                let msg = if op.args.as_ref().is_none_or(|a| a.is_empty()) {
                    "\"Python raise with no argument\"".to_string()
                } else {
                    format!(
                        "\"Python raise: {{:?}}\", {}",
                        &op.args.as_ref().unwrap()[0]
                    )
                };
                self.emit_line(&format!("panic!({msg});"));
            }
            "try_start" | "try_end" | "except_start" | "except_end" | "finally_start"
            | "finally_end" => {
                // No Rust equivalent in v1 — exception control flow ops are
                // structural markers only.  The actual error handling is done
                // via Result propagation in the generated Rust code.
            }

            // ── String operations ──────────────────────────────────────────────
            "format_string" | "string_format" => {
                let o = out();
                // Simple f-string: just convert all args to string and concat
                let args = op.args.as_deref().unwrap_or(&[]);
                let parts = args
                    .iter()
                    .map(|a| format!("molt_str(&{})", rust_ident(a)))
                    .collect::<Vec<_>>()
                    .join(" + &");
                let rhs = if parts.is_empty() {
                    "MoltValue::Str(String::new())".to_string()
                } else {
                    format!("MoltValue::Str({parts})")
                };
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }

            // ── String ops ────────────────────────────────────────────────────
            "str_from_obj" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Str(molt_str(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "repr_from_obj" => {
                // molt_repr already returns MoltValue
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_repr(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Sequence / tuple ops ──────────────────────────────────────────
            "tuple_new" | "list_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let items = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::List(vec![{items}])"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "list_fill_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let count = args
                    .first()
                    .map(|a| rust_ident(a))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let fill = args
                    .get(1)
                    .map(|a| rust_ident(a))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let rhs = format!(
                    "{{ let __n = match &{count} {{ MoltValue::Int(v) => (*v).max(0) as usize, MoltValue::Bool(v) => if *v {{ 1 }} else {{ 0 }}, _ => 0 }}; MoltValue::List(vec![{fill}.clone(); __n]) }}"
                );
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "unpack_sequence" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(seq_name) = args.first() {
                    let seq = rust_ident(seq_name);
                    let outputs = &args[1..];
                    let expected_count = op.value.unwrap_or(outputs.len() as i64).max(0) as usize;
                    self.emit_line(&format!(
                        "let __unpack_seq = molt_unpack_sequence(&{seq}, {expected_count});"
                    ));
                    for (index, out_name) in outputs.iter().take(expected_count).enumerate() {
                        let out = rust_ident(out_name);
                        self.emit_line(&declare(
                            &out,
                            &format!("__unpack_seq[{index}].clone()"),
                            &self.hoisted_vars.clone(),
                        ));
                    }
                }
            }
            "string_join" => {
                // string_join(sep, iterable) → sep.join(str(x) for x in iterable)
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let sep = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Str(\"\".to_string())".to_string());
                let seq = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "_seq".to_string());
                let rhs = format!(
                    "{{ let __sep = molt_str(&{sep}); if let MoltValue::List(ref __items) = {seq} {{ MoltValue::Str(__items.iter().map(|x| molt_str(x)).collect::<Vec<_>>().join(&__sep)) }} else {{ MoltValue::Str(molt_str(&{seq})) }} }}"
                );
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }

            // ── Catch-all stub ─────────────────────────────────────────────────
            other => {
                let o = out();
                let kind = other;
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&format!(
                        "let mut {o}: MoltValue = /* MOLT_STUB: {kind} */ MoltValue::None;"
                    ));
                } else {
                    self.emit_line(&format!("/* MOLT_STUB: {kind} */"));
                }
            }
        }
    }
}

fn rust_value(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        "MoltValue::None".to_string()
    } else {
        rust_ident(name)
    }
}

fn rust_clone(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        "MoltValue::None".to_string()
    } else {
        format!("{}.clone()", rust_ident(name))
    }
}

fn rust_slot_key(offset: i64) -> String {
    format!("MoltValue::Str(\"__slot_{offset}\".to_string())")
}

fn is_assignable_var(name: &str) -> bool {
    !(name.is_empty() || name == "_" || name == "none")
}

fn out_var(op: &OpIR) -> String {
    rust_ident(op.out.as_deref().unwrap_or("_"))
}

fn var_ref(op: &OpIR) -> String {
    rust_ident(op.var.as_deref().unwrap_or("_"))
}

fn arg0(op: &OpIR) -> String {
    op.args
        .as_deref()
        .and_then(|a| a.first())
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string())
}

fn args2(op: &OpIR) -> (String, String) {
    let args = op.args.as_deref().unwrap_or(&[]);
    let a = args
        .first()
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string());
    let b = args
        .get(1)
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string());
    (a, b)
}

fn rust_string_literal(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}
