use super::emit_helpers::{
    arg0, args2, declare_molt_value, is_assignable_var, out_var, rust_clone, rust_slot_key,
    rust_string_literal, rust_value, var_ref,
};
use super::lowering::op_definition_vars;
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

    pub(super) fn emit_unsupported_op(&mut self, op: &OpIR, reason: impl Into<String>) {
        let reason = reason.into();
        // Record failure at dispatch and deliberately emit no source.
        // `emit_source` is private and `compile_checked` rejects this record,
        // so no caller can observe either a partial program or a fabricated
        // value for an unsupported operation.
        self.unsupported_ops
            .push(format!("`{}` (rust backend): {reason}", op.kind));
    }

    pub(super) fn emit_op(&mut self, op: &OpIR) {
        for definition in op_definition_vars(op) {
            self.clear_alias(&definition);
        }

        if self.emit_op_literal(op) {
            return;
        }
        match op.kind.as_str() {
            "const_not_implemented" => self.emit_op_const_not_implemented(op),
            "const_ellipsis" => self.emit_op_const_ellipsis(op),
            "box" | "box_from_raw_int" | "unbox" | "unbox_to_raw_int" => {
                self.emit_op_representation_copy(op)
            }
            "copy" | "load_local" | "load_var" | "copy_var" | "identity_alias"
            | "binding_alias" => self.emit_op_local_copy(op),
            "store_var" => self.emit_op_store_var(op),
            "pos" | "unary_pos" | "type_guard" => self.emit_op_local_copy(op),
            "load" | "guarded_load" => self.emit_op_load(op),
            "closure_load" => self.emit_op_closure_load(op),
            "store_local" => self.emit_op_store_local(op),
            "store" => self.emit_op_store(op),
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
            "if" => self.emit_op_if(op),
            "if_not" => self.emit_op_if_not(op),
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
            kind if molt_ir::tir::op_kinds_generated::simpleir_return_shape(kind)
                == molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Value =>
            {
                self.emit_op_return(op)
            }
            kind if molt_ir::tir::op_kinds_generated::simpleir_return_shape(kind)
                == molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Void =>
            {
                self.emit_op_ret_void(op)
            }
            "call" | "call_func" | "call_internal" => self.emit_op_call(op),
            "call_method" => self.emit_op_call_method(op),
            "call_bind" | "call_indirect" => self.emit_op_call_bind(op),
            "callargs_new" => self.emit_op_callargs_new(op),
            "callargs_push_pos" => self.emit_op_callargs_push_pos(op),
            "callargs_expand_star" => self.emit_op_callargs_expand_star(op),
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
            "frame_home_store" | "frame_home_cell" | "frame_home_private_cell" => {
                self.emit_op_frame_home_store(op)
            }
            "frame_home_load" => self.emit_op_frame_home_load(op),
            "frame_home_take" => self.emit_op_frame_home_take(op),
            "frame_home_clear" => self.emit_op_frame_home_clear(op),
            "builtin_func" => self.emit_op_builtin_func(op),
            "print" | "builtin_print" => self.emit_op_print(op),
            "len" | "builtin_len" => self.emit_op_len(op),
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
            "zip" => self.emit_op_zip(op),
            "sorted" | "builtin_sorted" => self.emit_op_sorted(op),
            "reversed" | "builtin_reversed" => self.emit_op_reversed(op),
            "any" | "builtin_any" => self.emit_op_any(op),
            "all" | "builtin_all" => self.emit_op_all(op),
            "module_new" => self.emit_op_module_new(op),
            "bound_method_new" => self.emit_op_bound_method_new(op),
            "module_cache_get" | "module_load_cached" => self.emit_op_module_cache_get(op),
            "module_cache_set" => self.emit_op_module_cache_set(op),
            "module_cache_del" => self.emit_op_module_cache_del(op),
            "module_get_attr" | "module_get_name" => self.emit_op_module_get_attr(op),
            "module_set_attr" => self.emit_op_module_set_attr(op),
            "nop"
            | "comment"
            | "debug_label"
            | "line"
            | "type_assert"
            | "loop_index_end"
            | "drop_inserted"
            | "exception_region_drops_inserted" => self.emit_op_nop(op),
            "warn_stderr" => self.emit_op_warn_stderr(op),
            "str_from_obj" | "repr_from_obj" | "ascii_from_obj" | "bridge_unavailable" => {
                self.emit_op_runtime_value_call(op)
            }
            "unpack_sequence" => self.emit_op_unpack_sequence(op),
            _ => self.emit_op_other(op),
        }
    }
}
