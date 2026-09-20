use std::collections::HashMap;

use crate::ir::OpIR;
use crate::tir::op_kinds_generated::{SimpleIrVarFieldRole, simpleir_var_field_role_table};
use crate::tir::simple_def_use::{simple_ir_var_field_is_read, visit_simple_ir_defined_names};

use super::super::values::ValueId;
use super::*;

impl<'a> SsaContext<'a> {
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

/// Map wire definitions to semantic SSA results. A local assignment binds its
/// destination and optional value alias to the same value; a later assignment
/// creates a fresh value without retargeting that alias. Real multi-result
/// operations keep separate, densely indexed values.
pub(super) fn visit_simple_ir_ssa_definitions<'a>(
    op: &'a OpIR,
    mut visit: impl FnMut(&'a str, usize),
) {
    let binding = simpleir_var_field_role_table(&op.kind) == SimpleIrVarFieldRole::Definition;
    let mut result_index = 0;
    visit_simple_ir_defined_names(op, |name| {
        if is_variable(name) {
            visit(name, result_index);
            if !binding {
                result_index += 1;
            }
        }
    });
}

pub(super) fn simple_var_field_is_transport_fact(kind: &str) -> bool {
    simpleir_var_field_role_table(kind) != SimpleIrVarFieldRole::Result
}

pub(super) fn simple_var_field_is_value_operand(op: &OpIR) -> bool {
    simple_ir_var_field_is_read(op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_binding_and_snapshot_share_one_ssa_result() {
        for kind in ["store_var", "store_fast"] {
            let op = OpIR {
                kind: kind.into(),
                var: Some("local".into()),
                out: Some("snapshot".into()),
                args: Some(vec!["source".into()]),
                ..OpIR::default()
            };
            let mut definitions = Vec::new();
            visit_simple_ir_ssa_definitions(&op, |name, index| definitions.push((name, index)));
            assert_eq!(definitions, [("snapshot", 0), ("local", 0)]);
        }
    }

    #[test]
    fn multi_results_remain_distinct_and_reserved_names_do_not_create_holes() {
        for names in [
            vec!["source", "first", "second"],
            vec!["source", "True", "first", "none", "second"],
        ] {
            let op = OpIR {
                kind: "unpack_sequence".into(),
                args: Some(names.into_iter().map(str::to_string).collect()),
                ..OpIR::default()
            };
            let mut definitions = Vec::new();
            visit_simple_ir_ssa_definitions(&op, |name, index| definitions.push((name, index)));
            assert_eq!(definitions, [("first", 0), ("second", 1)]);
        }
    }
}
