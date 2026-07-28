use std::collections::HashMap;

use crate::ir::OpIR;
use crate::tir::op_kinds_generated::{SimpleIrVarFieldRole, simpleir_var_field_role_table};
use crate::tir::simple_def_use::{simple_ir_defined_names, simple_ir_var_field_is_read};

use super::super::values::ValueId;
use super::*;

impl<'a> SsaContext<'a> {
    pub(super) fn get_def_vars(&self, op: &OpIR) -> Vec<String> {
        simple_ir_defined_names(op)
            .into_iter()
            .filter(|name| is_variable(name))
            .collect()
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
    simpleir_var_field_role_table(kind) != SimpleIrVarFieldRole::Result
}

pub(super) fn simple_var_field_is_value_operand(op: &OpIR) -> bool {
    simple_ir_var_field_is_read(op)
}
