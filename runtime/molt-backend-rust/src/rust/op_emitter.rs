use super::emit_helpers::{
    arg0, args2, declare_molt_value, is_assignable_var, out_var, rust_clone, rust_slot_key,
    rust_string_literal, rust_stub_marker, rust_value, var_ref,
};
use super::runtime_surface::runtime_value_call_for_kind;
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

    fn emit_unsupported_op(&mut self, op: &OpIR, reason: impl Into<String>) {
        let marker = rust_stub_marker(op, reason);
        let o = out_var(op);
        if is_assignable_var(&o) {
            let rhs = format!("{{ /* {marker} */ MoltValue::None }}");
            self.emit_line(&declare_molt_value(&o, &rhs, &self.hoisted_vars));
        } else {
            self.emit_line(&format!("/* {marker} */"));
        }
    }

    pub(super) fn emit_op(&mut self, op: &OpIR) {
        if let Some(name) = op.out.as_deref() {
            let out_name = rust_ident(name);
            self.clear_alias(&out_name);
        }

        match op.kind.as_str() {
            "const" | "int_const" => self.emit_op_const(op),
            "const_float" => self.emit_op_const_float(op),
            "const_str" | "string_const" => self.emit_op_const_str(op),
            "const_bool" | "bool_const" => self.emit_op_const_bool(op),
            "const_none" | "none_const" => self.emit_op_const_none(op),
            "const_bytes" => self.emit_op_const_bytes(op),
            "const_bigint" => self.emit_op_const_bigint(op),
            "const_not_implemented" | "const_ellipsis" => self.emit_op_const_not_implemented(op),
            "box" | "box_from_raw_int" => self.emit_op_box(op),
            "load_local" => self.emit_op_load_local(op),
            "load_var" | "copy_var" => self.emit_op_load_var(op),
            "store_var" => self.emit_op_store_var(op),
            "load" | "guarded_load" => self.emit_op_load(op),
            "closure_load" => self.emit_op_closure_load(op),
            "store_local" => self.emit_op_store_local(op),
            "store" | "store_init" => self.emit_op_store(op),
            "closure_store" => self.emit_op_closure_store(op),
            "phi" => self.emit_op_phi(op),
            "add" | "inplace_add" | "binop_add" => self.emit_op_add(op),
            "sub" | "inplace_sub" | "binop_sub" => self.emit_op_sub(op),
            "mul" | "inplace_mul" | "binop_mul" => self.emit_op_mul(op),
            "div" | "true_div" => self.emit_op_div(op),
            "floor_div" | "floordiv" | "binop_floor_div" => self.emit_op_floor_div(op),
            "mod" | "modulo" | "binop_mod" => self.emit_op_mod(op),
            "pow" | "binop_pow" => self.emit_op_pow(op),
            "neg" | "unary_neg" => self.emit_op_neg(op),
            "unary_not" | "not" => self.emit_op_unary_not(op),
            "band" | "bit_and" => self.emit_op_band(op),
            "bor" | "bit_or" => self.emit_op_bor(op),
            "bxor" | "bit_xor" => self.emit_op_bxor(op),
            "lshift" | "shl" => self.emit_op_lshift(op),
            "rshift" | "shr" => self.emit_op_rshift(op),
            "eq" | "cmp_eq" => self.emit_op_eq(op),
            "ne" | "cmp_ne" => self.emit_op_ne(op),
            "lt" | "cmp_lt" => self.emit_op_lt(op),
            "le" | "cmp_le" => self.emit_op_le(op),
            "gt" | "cmp_gt" => self.emit_op_gt(op),
            "ge" | "cmp_ge" => self.emit_op_ge(op),
            "is" | "is_not" => self.emit_op_is(op),
            "in" | "not_in" => self.emit_op_in(op),
            "contains" => self.emit_op_contains(op),
            "and" | "_m_and" => self.emit_op_and(op),
            "or" => self.emit_op_or(op),
            "if" | "branch_false" => self.emit_op_if(op),
            "if_not" | "branch_true" => self.emit_op_if_not(op),
            "else" => self.emit_op_else(op),
            "end_if" => self.emit_op_end_if(op),
            "loop_start" | "while_start" => self.emit_op_loop_start(op),
            "loop_end" | "while_end" => self.emit_op_loop_end(op),
            "loop_break_if_false" => self.emit_op_loop_break_if_false(op),
            "loop_break_if_true" => self.emit_op_loop_break_if_true(op),
            "loop_break_if_exception" => self.emit_op_loop_break_if_exception(op),
            "loop_break" => self.emit_op_loop_break(op),
            "loop_continue" | "loop_carry_update" | "loop_carry_init" => {
                self.emit_op_loop_continue(op)
            }
            "loop_index_next" => self.emit_op_loop_index_next(op),
            "loop_index_start" => self.emit_op_loop_index_start(op),
            "iter" => self.emit_op_iter(op),
            "iter_next" => self.emit_op_iter_next(op),
            "for_range" => self.emit_op_for_range(op),
            "for_iter" => self.emit_op_for_iter(op),
            "range_new" => self.emit_op_range_new(op),
            "end_for" => self.emit_op_end_for(op),
            "break" => self.emit_op_break(op),
            "continue" => self.emit_op_continue(op),
            "return" | "ret" => self.emit_op_return(op),
            "return_none" | "ret_none" | "ret_void" => self.emit_op_return_none(op),
            "call" | "call_func" | "call_internal" => self.emit_op_call(op),
            "call_method" => self.emit_op_call_method(op),
            "call_bind" | "call_indirect" => self.emit_op_call_bind(op),
            "callargs_new" => self.emit_op_callargs_new(op),
            "callargs_push_pos" => self.emit_op_callargs_push_pos(op),
            "callargs_expand_star" => self.emit_op_callargs_expand_star(op),
            "callargs_push_kw" | "callargs_expand_kwstar" => self.emit_op_callargs_push_kw(op),
            "func_new" | "func_new_closure" => self.emit_op_func_new(op),
            "code_new" => self.emit_op_code_new(op),
            "code_slots_init" => self.emit_op_code_slots_init(op),
            "code_slot_set" => self.emit_op_code_slot_set(op),
            "exception_last" | "exception_last_pending" | "exception_finally_pending_observer" => {
                self.emit_op_exception_last(op)
            }
            "exception_stack_depth" | "exception_stack_enter" => {
                self.emit_op_exception_stack_depth(op)
            }
            "exception_clear" => self.emit_op_exception_clear(op),
            "exception_stack_exit" => self.emit_op_exception_stack_exit(op),
            "exception_stack_set_depth" => self.emit_op_exception_stack_set_depth(op),
            "exception_stack_clear" => self.emit_op_exception_stack_clear(op),
            "exception_set_last" => self.emit_op_exception_set_last(op),
            "exception_active" => self.emit_op_exception_active(op),
            "trace_enter_slot" => self.emit_op_trace_enter_slot(op),
            "trace_exit" => self.emit_op_trace_exit(op),
            "frame_locals_set" => self.emit_op_frame_locals_set(op),
            "builtin_func" => self.emit_op_builtin_func(op),
            "print" | "builtin_print" => self.emit_op_print(op),
            "len" | "builtin_len" => self.emit_op_len(op),
            "int" | "cast_int" | "builtin_int" => self.emit_op_int(op),
            "int_from_obj" => self.emit_op_int_from_obj(op),
            "int_from_str_of_obj" => self.emit_op_int_from_str_of_obj(op),
            "float" | "cast_float" | "builtin_float" => self.emit_op_float(op),
            "float_from_obj" => self.emit_op_float_from_obj(op),
            "str" | "cast_str" | "builtin_str" => self.emit_op_str(op),
            "bool" | "cast_bool" | "builtin_bool" => self.emit_op_bool(op),
            "chr" => self.emit_op_chr(op),
            "ord" => self.emit_op_ord(op),
            "ord_at" => self.emit_op_ord_at(op),
            "abs" | "builtin_abs" => self.emit_op_abs(op),
            "build_list" | "alloc" => self.emit_op_build_list(op),
            "build_dict" | "dict_new" => self.emit_op_build_dict(op),
            "list_append" => self.emit_op_list_append(op),
            "get_item" | "subscript" | "index" => self.emit_op_get_item(op),
            "dict_get" => self.emit_op_dict_get(op),
            "set_item" | "store_subscript" | "store_index" => self.emit_op_set_item(op),
            "dict_set" => self.emit_op_dict_set(op),
            "get_attr" | "load_attr" => self.emit_op_get_attr(op),
            "get_attr_name" => self.emit_op_get_attr_name(op),
            "get_attr_name_default" => self.emit_op_get_attr_name_default(op),
            "set_attr" | "store_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                self.emit_op_set_attr(op)
            }
            "enumerate" => self.emit_op_enumerate(op),
            "zip" => self.emit_op_zip(op),
            "sorted" | "builtin_sorted" => self.emit_op_sorted(op),
            "reversed" | "builtin_reversed" => self.emit_op_reversed(op),
            "sum" | "builtin_sum" => self.emit_op_sum(op),
            "any" | "builtin_any" => self.emit_op_any(op),
            "all" | "builtin_all" => self.emit_op_all(op),
            "range" | "builtin_range" => self.emit_op_range(op),
            "module_new" => self.emit_op_module_new(op),
            "class_new" | "object_new" | "builtin_type" => self.emit_op_class_new(op),
            "bound_method_new" => self.emit_op_bound_method_new(op),
            "alloc_class_static" | "alloc_class_trusted" | "alloc_class" => {
                self.emit_op_alloc_class_static(op)
            }
            "object_set_class" => self.emit_op_object_set_class(op),
            "class_set_base" => self.emit_op_class_set_base(op),
            "class_set_layout_version" => self.emit_op_class_set_layout_version(op),
            "class_merge_layout" => self.emit_op_class_merge_layout(op),
            "class_apply_set_name"
            | "class_layout_version"
            | "class_layout_field_count"
            | "class_layout_slot_count" => self.emit_op_class_apply_set_name(op),
            "module_cache_get" | "module_load_cached" => self.emit_op_module_cache_get(op),
            "module_cache_set" => self.emit_op_module_cache_set(op),
            "module_cache_del" => self.emit_op_module_cache_del(op),
            "module_import" => self.emit_op_module_import(op),
            "module_get_attr" | "module_import_from" | "module_get_name" => {
                self.emit_op_module_get_attr(op)
            }
            "module_set_attr" => self.emit_op_module_set_attr(op),
            "nop" | "comment" | "debug_label" | "line" | "type_assert" => self.emit_op_nop(op),
            "str_from_obj" | "repr_from_obj" | "ascii_from_obj" | "bridge_unavailable" => {
                self.emit_op_runtime_value_call(op)
            }
            "br_if" | "branch" => self.emit_op_unstructured_branch(op),
            "alloc_task"
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
            | "check_exception" => self.emit_op_runtime_control_gap(op),
            "inc_ref" | "borrow" | "binding_alias" => self.emit_op_inc_ref(op),
            "dec_ref" | "release" => self.emit_op_dec_ref(op),
            "alloc_instance" | "init_instance" | "instance_set_field" | "instance_get_field"
            | "instance_has_field" => self.emit_op_alloc_instance(op),
            "raise" | "reraise" => self.emit_op_raise(op),
            "try_start" | "try_end" | "except_start" | "except_end" | "finally_start"
            | "finally_end" => self.emit_op_try_start(op),
            "format_string" | "string_format" => self.emit_op_format_string(op),
            "tuple_new" | "list_new" => self.emit_op_tuple_new(op),
            "list_fill_new" => self.emit_op_list_fill_new(op),
            "unpack_sequence" => self.emit_op_unpack_sequence(op),
            "string_join" => self.emit_op_string_join(op),
            _ => self.emit_op_other(op),
        }
    }

    fn emit_op_runtime_value_call(&mut self, op: &OpIR) {
        let Some(call) = runtime_value_call_for_kind(op.kind.as_str()) else {
            self.emit_op_other(op);
            return;
        };
        let rhs = match call.rhs(op) {
            Ok(rhs) => rhs,
            Err(reason) => {
                self.emit_unsupported_op(op, reason);
                return;
            }
        };
        let o = out_var(op);
        if is_assignable_var(&o) {
            self.emit_line(&declare_molt_value(&o, &rhs, &self.hoisted_vars));
        } else {
            self.emit_line(&format!("{rhs};"));
        }
    }

    fn emit_op_const(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_const_float(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let f = op.f_value.unwrap_or(0.0);
        let rhs = format!("MoltValue::Float({f:.17})");
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    fn emit_op_const_str(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let s = op.s_value.as_deref().unwrap_or("");
        let rhs = format!("MoltValue::Str({}.to_string())", rust_string_literal(s));
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    fn emit_op_const_bool(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let b = op.value.unwrap_or(0) != 0;
        let rhs = format!("MoltValue::Bool({b})");
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    fn emit_op_const_none(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
    }

    fn emit_op_const_bytes(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "bytes literals require a Rust backend bytes value representation",
        );
    }

    fn emit_op_const_bigint(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let s = op.s_value.as_deref().unwrap_or("0");
        if let Ok(value) = s.parse::<i64>() {
            let rhs = format!("MoltValue::Int({value}i64)");
            self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
        } else {
            self.emit_unsupported_op(
                op,
                "bigint literal exceeds Rust backend i64 value representation",
            );
        }
    }

    fn emit_op_const_not_implemented(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "literal `{}` requires a dedicated Rust backend value representation",
                op.kind
            ),
        );
    }

    fn emit_op_box(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let rhs = op
            .args
            .as_deref()
            .and_then(|args| args.first())
            .map(|src| rust_clone(src))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    fn emit_op_load_local(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let v = var_ref(op);
        self.emit_line(&declare(
            &o,
            &format!("{v}.clone()"),
            &self.hoisted_vars.clone(),
        ));
        self.note_alias(o, v);
    }

    fn emit_op_load_var(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let v = var_ref(op);
        self.emit_line(&declare(
            &o,
            &format!("{v}.clone()"),
            &self.hoisted_vars.clone(),
        ));
        self.note_alias(o, v);
    }

    fn emit_op_store_var(&mut self, op: &OpIR) {
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

    fn emit_op_load(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_closure_load(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_store_local(&mut self, op: &OpIR) {
        let v = var_ref(op);
        if let Some(src) = op.args.as_ref().and_then(|a| a.first()) {
            let s = rust_ident(src);
            self.emit_line(&format!("{v} = {s}.clone();"));
            self.note_alias(v, s);
        } else {
            self.clear_alias(&v);
        }
    }

    fn emit_op_store(&mut self, op: &OpIR) {
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

    fn emit_op_closure_store(&mut self, op: &OpIR) {
        if let Some(args) = &op.args
            && args.len() >= 2
        {
            let slot = format!("__closure_{}", rust_ident(&args[0]));
            let src = rust_ident(&args[1]);
            self.emit_line(&format!("{slot} = {src}.clone();"));
        }
    }

    fn emit_op_phi(&mut self, _op: &OpIR) {

        // Phi nodes are handled by the hoisting logic above; skip here.
    }

    fn emit_op_add(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_sub(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_mul(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_div(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_div({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_floor_div(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_floor_div({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_mod(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_mod({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_pow(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_pow({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_neg(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_neg({a}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_unary_not(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(!molt_bool(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_band(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}) & molt_int(&{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_bor(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}) | molt_int(&{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_bxor(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}) ^ molt_int(&{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_lshift(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}) << (molt_int(&{b}) as u32))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_rshift(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}) >> (molt_int(&{b}) as u32))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_eq(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_eq(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_ne(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(!molt_eq(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_lt(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_lt(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_le(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_le(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_gt(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_gt(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_ge(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_ge(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_is(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_in(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_contains(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_and(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("if !molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_or(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("if molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_if(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if molt_bool(&{cond}) {{"));
        self.indent += 1;
    }

    fn emit_op_if_not(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if !molt_bool(&{cond}) {{"));
        self.indent += 1;
    }

    fn emit_op_else(&mut self, _op: &OpIR) {
        self.indent -= 1;
        self.emit_line("} else {");
        self.indent += 1;
    }

    fn emit_op_end_if(&mut self, _op: &OpIR) {
        self.indent -= 1;
        self.emit_line("}");
    }

    fn emit_op_loop_start(&mut self, _op: &OpIR) {
        self.emit_line("loop {");
        self.indent += 1;
    }

    fn emit_op_loop_end(&mut self, _op: &OpIR) {
        self.indent -= 1;
        self.emit_line("}");
    }

    fn emit_op_loop_break_if_false(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if !molt_bool(&{cond}) {{ break; }}"));
    }

    fn emit_op_loop_break_if_true(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if molt_bool(&{cond}) {{ break; }}"));
    }

    fn emit_op_loop_break_if_exception(&mut self, _op: &OpIR) {
        // Value-less exception-flag break: exit an iterator-consumer loop
        // when a runtime exception is pending (the producer returned the
        // None sentinel on a mid-iteration raise).  Reads the same
        // sacrosanct flag the runtime CHECK_EXCEPTION uses; the still
        // pending exception then rides up the lazy-return path.
        self.emit_line("if molt_exception_pending() != 0 { break; }");
    }

    fn emit_op_loop_break(&mut self, _op: &OpIR) {
        self.emit_line("break;");
    }

    fn emit_op_loop_continue(&mut self, _op: &OpIR) {
        self.emit_line("continue;");
    }

    fn emit_op_loop_index_next(&mut self, op: &OpIR) {
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

    fn emit_op_loop_index_start(&mut self, _op: &OpIR) {

        // Initialization is handled in the loop preamble above; skip here.
    }

    fn emit_op_iter(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let src = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_iter(&{src})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_iter_next(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let iter_var = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_iter_next(&mut {iter_var})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_for_range(&mut self, op: &OpIR) {
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

    fn emit_op_for_iter(&mut self, op: &OpIR) {
        let out = || out_var(op);

        // for_iter (comprehension-inlined): out = loop_var, args[0] = iterable.
        // The comprehension inliner in lib.rs always emits this convention.
        let iter_var = out();
        let iterable = arg0(op);
        self.emit_line(&format!("for {iter_var} in molt_iter_list(&{iterable}) {{"));
        self.indent += 1;
    }

    fn emit_op_range_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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
            &format!("molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_end_for(&mut self, op: &OpIR) {
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

    fn emit_op_break(&mut self, _op: &OpIR) {
        self.emit_line("break;");
    }

    fn emit_op_continue(&mut self, _op: &OpIR) {
        self.emit_line("continue;");
    }

    fn emit_op_return(&mut self, op: &OpIR) {
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

    fn emit_op_return_none(&mut self, _op: &OpIR) {
        self.emit_param_writeback();
        if self.current_is_main {
            self.emit_line("return;");
        } else {
            self.emit_line("return MoltValue::None;");
        }
    }

    fn emit_op_call(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_call_method(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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
                _ => {
                    self.emit_unsupported_op(
                        op,
                        format!("unsupported method `{method}` on `{obj}`"),
                    );
                    return;
                }
            };
            if o == "_" || o == "none" {
                self.emit_line(&format!("{rhs};"));
            } else {
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
        }
    }

    fn emit_op_call_bind(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_callargs_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_callargs_push_pos(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let list = rust_ident(&args[0]);
            let val = rust_ident(&args[1]);
            self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
            self.emit_alias_writeback(&list);
        }
    }

    fn emit_op_callargs_expand_star(&mut self, op: &OpIR) {
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

    fn emit_op_callargs_push_kw(&mut self, _op: &OpIR) {

        // Keyword arguments are currently ignored in the Rust subset.
    }

    fn emit_op_func_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let rhs = if let Some(ref fn_name) = op.s_value {
            let fn_ident = rust_ident(fn_name);
            format!("MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {fn_ident}(args)))")
        } else {
            "MoltValue::None".to_string()
        };
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    fn emit_op_code_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_code_slots_init(&mut self, op: &OpIR) {
        let count = op.value.unwrap_or(0);
        self.emit_line(&format!("molt_code_slots_init({count});"));
    }

    fn emit_op_code_slot_set(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if let Some(code) = args.first() {
            let code = rust_ident(code);
            let code_id = op.value.unwrap_or(0);
            self.emit_line(&format!("molt_code_slot_set({code_id}, &{code});"));
        }
    }

    fn emit_op_exception_last(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_exception_stack_depth(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_exception_clear(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_exception_stack_exit(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let prev = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        self.emit_line(&format!("molt_exception_stack_exit(&{prev});"));
    }

    fn emit_op_exception_stack_set_depth(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let depth = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        self.emit_line(&format!("molt_exception_stack_set_depth(&{depth});"));
    }

    fn emit_op_exception_stack_clear(&mut self, _op: &OpIR) {
        self.emit_line("molt_exception_stack_clear();");
    }

    fn emit_op_exception_set_last(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let exc = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&format!("molt_exception_set_last(&{exc});"));
    }

    fn emit_op_exception_active(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            self.emit_line(&declare(
                &o,
                "molt_exception_active()",
                &self.hoisted_vars.clone(),
            ));
        }
    }

    fn emit_op_trace_enter_slot(&mut self, op: &OpIR) {
        let code_id = op.value.unwrap_or(0);
        self.emit_line(&format!("molt_trace_enter_slot({code_id});"));
    }

    fn emit_op_trace_exit(&mut self, _op: &OpIR) {
        self.emit_line("molt_trace_exit();");
    }

    fn emit_op_frame_locals_set(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let locals = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&format!("molt_frame_locals_set(&{locals});"));
    }

    fn emit_op_builtin_func(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let builtin = op.s_value.as_deref().unwrap_or("");
        self.emit_line(&declare(
            &o,
            &format!("molt_builtin_func({})", rust_string_literal(builtin)),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_print(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let arg_list = args
            .iter()
            .map(|a| rust_clone(a))
            .collect::<Vec<_>>()
            .join(", ");
        self.emit_line(&format!("molt_print(&[{arg_list}]);"));
    }

    fn emit_op_len(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_len(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_int(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_int_from_obj(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Int(molt_int(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_int_from_str_of_obj(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_float(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Float(molt_float(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_float_from_obj(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Float(molt_float(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_str(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Str(molt_str(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_bool(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_bool(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_chr(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_chr(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_ord(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_ord(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_ord_at(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (obj, key) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_ord_at(&{obj}, &{key})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_abs(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_abs({a}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_build_list(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_build_dict(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_list_append(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let list = rust_ident(&args[0]);
            let val = rust_ident(&args[1]);
            self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
            self.emit_alias_writeback(&list);
        }
    }

    fn emit_op_get_item(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_dict_get(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_set_item(&mut self, op: &OpIR) {
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

    fn emit_op_dict_set(&mut self, op: &OpIR) {
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

    fn emit_op_get_attr(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_get_attr_name(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_get_attr_name_default(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_set_attr(&mut self, op: &OpIR) {
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

    fn emit_op_enumerate(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_zip(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_zip(&{a}, &{b})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_sorted(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_sorted(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_reversed(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_reversed(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_sum(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_sum(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_any(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_any(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_all(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(molt_all(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_range(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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
            &format!("molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_module_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        self.emit_line(&declare(
            &o,
            "MoltValue::Dict(vec![])",
            &self.hoisted_vars.clone(),
        ));
    }

    fn emit_op_class_new(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "{} requires a Rust backend object/type representation",
                op.kind
            ),
        );
    }

    fn emit_op_bound_method_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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
            self.emit_unsupported_op(op, "bound_method_new requires method and self");
        }
    }

    fn emit_op_alloc_class_static(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "{} requires a Rust backend class instance representation",
                op.kind
            ),
        );
    }

    fn emit_op_object_set_class(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "object_set_class requires a Rust backend object/type representation",
        );
    }

    fn emit_op_class_set_base(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "class_set_base requires a Rust backend class representation",
        );
    }

    fn emit_op_class_set_layout_version(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "class_set_layout_version requires a Rust backend class representation",
        );
    }

    fn emit_op_class_merge_layout(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "class_merge_layout requires a Rust backend class representation",
        );
    }

    fn emit_op_class_apply_set_name(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("{} requires a Rust backend class representation", op.kind),
        );
    }

    fn emit_op_module_cache_get(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_module_cache_set(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_module_cache_del(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_module_import(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_module_get_attr(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_module_set_attr(&mut self, op: &OpIR) {
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

    fn emit_op_nop(&mut self, op: &OpIR) {
        let out = || out_var(op);

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            self.emit_unsupported_op(
                op,
                format!("marker op `{}` unexpectedly produces output", op.kind),
            );
        }
    }

    fn emit_op_unstructured_branch(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("{} requires Rust backend CFG/block lowering", op.kind),
        );
    }

    fn emit_op_runtime_control_gap(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "{} requires Rust backend runtime-control representation",
                op.kind
            ),
        );
    }

    fn emit_op_inc_ref(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_dec_ref(&mut self, _op: &OpIR) {}

    fn emit_op_alloc_instance(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("instance op `{}` has no Rust backend lowering", op.kind),
        );
    }

    fn emit_op_raise(&mut self, op: &OpIR) {
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

    fn emit_op_try_start(&mut self, _op: &OpIR) {

        // No Rust equivalent in v1 — exception control flow ops are
        // structural markers only.  The actual error handling is done
        // via Result propagation in the generated Rust code.
    }

    fn emit_op_format_string(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_tuple_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_list_fill_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_unpack_sequence(&mut self, op: &OpIR) {
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_string_join(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

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

    fn emit_op_other(&mut self, op: &OpIR) {
        let other = op.kind.as_str();

        self.emit_unsupported_op(op, format!("unsupported Rust backend op `{other}`"));
    }
}
