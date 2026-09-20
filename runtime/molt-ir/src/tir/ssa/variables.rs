use std::collections::HashMap;

use crate::ir::OpIR;
use crate::tir::op_kinds_generated::{SimpleIrVarFieldRole, simpleir_var_field_role_table};
use crate::tir::simple_def_use::{
    SimpleIrResultField, simple_ir_binding, simple_ir_var_field_is_read,
    visit_simple_ir_defined_names, visit_simple_ir_results,
};

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
/// creates a fresh value without retargeting that alias. Positional Var/Out
/// operations reserve discarded results; variadic named results stay dense.
pub(super) fn visit_simple_ir_ssa_definitions<'a>(
    op: &'a OpIR,
    mut visit: impl FnMut(&'a str, usize),
) {
    visit_simple_ir_ssa_result_slots(op, |name, index| {
        if let Some(name) = name {
            visit(name, index);
        }
    });
}

pub(super) fn simple_ir_ssa_result_count(op: &OpIR) -> usize {
    let mut count = 0;
    visit_simple_ir_ssa_result_slots(op, |_, index| count = count.max(index + 1));
    count
}

fn visit_simple_ir_ssa_result_slots<'a>(
    op: &'a OpIR,
    mut visit: impl FnMut(Option<&'a str>, usize),
) {
    if simple_ir_binding(op).is_some() {
        visit_simple_ir_defined_names(op, |name| {
            if is_variable(name) {
                visit(Some(name), 0);
            }
        });
        return;
    }
    let mut positional = false;
    let mut result_index = 0;
    visit_simple_ir_results(op, |result| {
        positional |= result.field == SimpleIrResultField::Var;
        let name = result.name.filter(|name| is_variable(name));
        if positional || name.is_some() {
            visit(name, result_index);
            result_index += 1;
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

    #[test]
    fn fixed_result_positions_survive_missing_and_reserved_names() {
        for kind in ["checked_add", "checked_mul", "iter_next_unboxed"] {
            for discard in [None, Some("none")] {
                for (var, out, expected) in [
                    (discard, Some("flag"), vec![("flag", 1)]),
                    (Some("value"), discard, vec![("value", 0)]),
                    (discard, discard, vec![]),
                ] {
                    let op = OpIR {
                        kind: kind.into(),
                        var: var.map(str::to_string),
                        out: out.map(str::to_string),
                        ..OpIR::default()
                    };
                    let mut definitions = Vec::new();
                    visit_simple_ir_ssa_definitions(&op, |name, index| {
                        definitions.push((name, index))
                    });
                    assert_eq!(definitions, expected, "{kind}");
                    assert_eq!(simple_ir_ssa_result_count(&op), 2, "{kind}");
                }
            }
        }
    }
}
