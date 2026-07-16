use super::emit_helpers::{
    arg0, args2, declare_molt_value, is_assignable_var, out_var, rust_clone, rust_slot_key,
    rust_string_literal, rust_value, var_ref,
};
use super::runtime_surface::runtime_value_call_for_kind;
use super::{RustBackend, rust_ident};
use crate::OpIR;
use std::collections::BTreeSet;

mod builtins;
mod calls;
mod control;
mod exceptions;
mod gaps;
mod modules;
mod numeric;
mod values;

impl RustBackend {
    // Op dispatch stays here; lowering families live in sibling modules.

    fn op_prefers_integer_runtime_lane(&self, op: &OpIR) -> bool {
        self.current_scalar_plan
            .as_ref()
            .is_some_and(|plan| plan.op_prefers_integer_runtime_lane(op))
    }

    fn emit_unsupported_op(&mut self, op: &OpIR, reason: impl Into<String>) {
        let reason = reason.into();
        // Record failure at dispatch and deliberately emit no source.
        // `emit_source` is private and `compile_checked` rejects this record,
        // so no caller can observe either a partial program or a fabricated
        // value for an unsupported operation.
        self.unsupported_ops
            .push(format!("`{}` (rust backend): {reason}", op.kind));
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
            "build_list" | "list_new" | "alloc" => self.emit_op_build_list(op),
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
            "alloc_class" => self.emit_op_alloc_class(op),
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
            "tuple_new" => self.emit_op_tuple_new(op),
            "list_fill_new" => self.emit_op_list_fill_new(op),
            "unpack_sequence" => self.emit_op_unpack_sequence(op),
            "string_join" => self.emit_op_string_join(op),
            _ => self.emit_op_other(op),
        }
    }
}
