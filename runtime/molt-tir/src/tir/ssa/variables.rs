use std::collections::HashMap;

use crate::ir::OpIR;

use super::super::values::ValueId;
use super::*;

impl<'a> SsaContext<'a> {
    /// Get the variable name being defined by an op, if any.
    ///
    /// Side-effect-only ops (set_attr, store_index, del_attr, etc.) may have
    /// an `out` field in SimpleIR but should NOT produce a TIR result value.
    /// The verifier enforces StoreAttr/StoreIndex/DelAttr have 0 results.
    fn get_def_var(&self, op: &OpIR) -> Option<String> {
        if matches!(op.kind.as_str(), "store_var" | "delete_var") {
            return op.var.clone().filter(|v| is_variable(v));
        }
        // Side-effect-only ops: no result value even if `out` is set.
        if matches!(
            op.kind.as_str(),
            "set_attr"
                | "store_attr"
                | "set_attr_name"
                | "set_attr_generic_ptr"
                | "set_attr_generic_obj"
                | "guarded_field_set"
                | "guarded_field_init"
                | "module_cache_set"
                | "module_cache_del"
                | "module_set_attr"
                | "module_del_global"
                | "module_del_global_if_present"
                | "store"
                | "store_init"
                | "store_index"
                | "index_set"
                | "del_attr"
                | "del_attr_name"
                | "del_attr_generic_ptr"
                | "del_attr_generic_obj"
                | "del_index"
                | "raise"
                | "raise_from"
                | "inc_ref"
                | "dec_ref"
        ) {
            return None;
        }
        op.out.clone().filter(|v| is_variable(v))
    }

    pub(super) fn get_def_vars(&self, op: &OpIR) -> Vec<String> {
        if op.kind == "unpack_sequence" {
            return op
                .args
                .as_ref()
                .map(|args| {
                    args.iter()
                        .skip(1)
                        .filter(|v| is_variable(v))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
        }
        // Two-result ops: `var` = results[0], `out` = results[1] (the
        // IterNextUnboxed transport convention; CheckedAdd/CheckedMul carry
        // var = wrapping sum/product, out = overflow flag).
        if op.kind == "iter_next_unboxed" || op.kind == "checked_add" || op.kind == "checked_mul" {
            let mut out = Vec::new();
            if let Some(var) = &op.var
                && is_variable(var)
            {
                out.push(var.clone());
            }
            if let Some(done) = &op.out
                && is_variable(done)
            {
                out.push(done.clone());
            }
            return out;
        }
        self.get_def_var(op).into_iter().collect()
    }

    /// Resolve a variable name to its current SSA ValueId.
    pub(super) fn resolve_var(
        var: &str,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Option<ValueId> {
        var_stacks.get(var).and_then(|s| s.last().copied())
    }

    pub(super) fn resolve_known_var(
        &self,
        var: &str,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Option<ValueId> {
        Self::resolve_var(var, var_stacks).or_else(|| {
            if self.all_vars.contains(var) {
                self.undef_value
            } else {
                None
            }
        })
    }
}

/// Returns true if the name looks like a SimpleIR variable (not a special
/// keyword like "none").
pub(super) fn is_variable(name: &str) -> bool {
    !name.is_empty() && name != "none" && name != "True" && name != "False"
}

pub(super) fn simple_var_field_is_transport_fact(kind: &str) -> bool {
    !matches!(kind, "checked_add" | "checked_mul" | "iter_next_unboxed")
}

pub(super) fn simple_var_field_is_value_operand(op: &OpIR) -> bool {
    if matches!(
        op.kind.as_str(),
        "store_var" | "delete_var" | "checked_add" | "checked_mul" | "iter_next_unboxed"
    ) {
        return false;
    }
    if matches!(op.kind.as_str(), "copy_var" | "load_var")
        && op.args.as_ref().is_some_and(|args| !args.is_empty())
    {
        return false;
    }
    true
}
