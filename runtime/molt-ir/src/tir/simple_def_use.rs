#[cfg(test)]
use std::collections::BTreeSet;

use crate::ir::OpIR;
use crate::tir::op_kinds_generated::{
    SimpleIrReturnShape, SimpleIrVarFieldRole, simpleir_first_trailing_result_arg_table,
    simpleir_out_field_is_metadata, simpleir_return_shape, simpleir_var_field_role_table,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleIrReadField {
    Arg(usize),
    Var,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpleIrRead<'a> {
    pub name: &'a str,
    pub field: SimpleIrReadField,
}

/// Whether `op.var` denotes a source read rather than an assignment target.
///
/// This is the canonical SimpleIR field-role authority used by CFG liveness,
/// dead-operation elimination, megafunction ABI planning, and backends.
pub fn simple_ir_var_field_is_read(op: &OpIR) -> bool {
    match simpleir_var_field_role_table(op.kind.as_str()) {
        SimpleIrVarFieldRole::Read => true,
        SimpleIrVarFieldRole::MetadataWhenArgs => op.args.as_ref().is_none_or(Vec::is_empty),
        SimpleIrVarFieldRole::Definition
        | SimpleIrVarFieldRole::Result
        | SimpleIrVarFieldRole::Forbidden => false,
    }
}

/// Visit names defined directly by an operation result without allocating.
pub fn visit_simple_ir_result_names<'a>(op: &'a OpIR, mut visit: impl FnMut(&'a str)) {
    if simpleir_var_field_role_table(op.kind.as_str()) == SimpleIrVarFieldRole::Result
        && let Some(var) = op.var.as_deref()
        && var != "none"
    {
        visit(var);
    }
    if !simpleir_out_field_is_metadata(op.kind.as_str())
        && let Some(out) = op.out.as_deref()
        && out != "none"
    {
        visit(out);
    }
    if let Some(first_result) = simpleir_first_trailing_result_arg_table(op.kind.as_str())
        && let Some(args) = op.args.as_deref()
    {
        for name in args.iter().skip(first_result).map(String::as_str) {
            if name != "none" {
                visit(name);
            }
        }
    }
}

#[cfg(test)]
fn simple_ir_result_names(op: &OpIR) -> Vec<&str> {
    let mut defined = Vec::new();
    visit_simple_ir_result_names(op, |name| defined.push(name));
    defined
}

#[cfg(test)]
fn push_name(out: &mut Vec<String>, seen: &mut BTreeSet<String>, name: &str) {
    if name != "none" && seen.insert(name.to_string()) {
        out.push(name.to_string());
    }
}

/// Every source read and its canonical field role, in deterministic order.
///
/// Consumers that need positional diagnostics or narrowly-scoped transport
/// exceptions use this API directly. Name-set consumers should insert the
/// borrowed names into their own long-lived set.
pub fn visit_simple_ir_reads<'a>(op: &'a OpIR, mut visit: impl FnMut(SimpleIrRead<'a>)) {
    if let Some(args) = op.args.as_ref() {
        let read_arity = simpleir_first_trailing_result_arg_table(op.kind.as_str())
            .unwrap_or(args.len())
            .min(args.len());
        for (index, name) in args.iter().take(read_arity).enumerate() {
            visit(SimpleIrRead {
                name,
                field: SimpleIrReadField::Arg(index),
            });
        }
    }
    if simple_ir_var_field_is_read(op)
        && let Some(name) = op.var.as_deref()
    {
        visit(SimpleIrRead {
            name,
            field: SimpleIrReadField::Var,
        });
    }
}

/// Visit the canonical value payload of a normal return terminator.
///
/// The generated control-kind registry owns return-family membership. Within
/// that family, `args` is the sole value carrier. Every CFG, SSA, splitter, and
/// backend-side structural transform must consume this helper.
pub fn visit_simple_ir_return_values<'a>(op: &'a OpIR, mut visit: impl FnMut(&'a str)) {
    if simpleir_return_shape(op.kind.as_str()) != SimpleIrReturnShape::Value {
        return;
    }
    if let Some(value) = op.args.as_deref().and_then(|args| args.first()) {
        visit(value);
    }
}

pub fn simple_ir_return_has_value(op: &OpIR) -> bool {
    let mut has_value = false;
    visit_simple_ir_return_values(op, |_| has_value = true);
    has_value
}

#[cfg(test)]
fn simple_ir_reads(op: &OpIR) -> Vec<SimpleIrRead<'_>> {
    let mut reads = Vec::new();
    visit_simple_ir_reads(op, |source| reads.push(source));
    reads
}

#[cfg(test)]
fn simple_ir_read_names(op: &OpIR) -> Vec<String> {
    let mut read = Vec::new();
    let mut seen = BTreeSet::new();
    visit_simple_ir_reads(op, |source| {
        push_name(&mut read, &mut seen, source.name);
    });
    read
}

/// Visit every name defined by an operation without per-op allocation.
/// Consumers that retain names must copy them into their own long-lived set.
pub fn visit_simple_ir_defined_names<'a>(op: &'a OpIR, mut visit: impl FnMut(&'a str)) {
    visit_simple_ir_result_names(op, &mut visit);
    if simpleir_var_field_role_table(op.kind.as_str()) == SimpleIrVarFieldRole::Definition
        && let Some(var) = op.var.as_deref().or(op.out.as_deref())
        && var != "none"
    {
        visit(var);
    }
}

#[cfg(test)]
fn simple_ir_defined_names(op: &OpIR) -> Vec<String> {
    let mut defined = Vec::new();
    let mut seen = BTreeSet::new();
    visit_simple_ir_defined_names(op, |name| {
        push_name(&mut defined, &mut seen, name);
    });
    defined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    #[test]
    fn local_slot_store_targets_are_definitions_not_reads() {
        for kind in ["store_var", "store_fast"] {
            let mut store = op(kind);
            store.var = Some("_bb1_arg0".into());
            store.args = Some(vec!["incoming".into()]);

            assert_eq!(
                simple_ir_reads(&store),
                vec![SimpleIrRead {
                    name: "incoming",
                    field: SimpleIrReadField::Arg(0),
                }]
            );
            assert_eq!(
                simple_ir_defined_names(&store),
                vec!["_bb1_arg0".to_string()]
            );
        }
    }

    #[test]
    fn unpack_sequence_reads_only_input_and_defines_output_args() {
        let mut unpack = op("unpack_sequence");
        unpack.args = Some(vec!["sequence".into(), "first".into(), "second".into()]);

        assert_eq!(
            simple_ir_reads(&unpack),
            vec![SimpleIrRead {
                name: "sequence",
                field: SimpleIrReadField::Arg(0),
            }]
        );
        assert_eq!(
            simple_ir_defined_names(&unpack),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn args_based_local_copies_treat_var_as_metadata() {
        for kind in ["copy_var", "load_var"] {
            let mut copy = op(kind);
            copy.var = Some("local_name".into());
            copy.args = Some(vec!["source".into()]);
            copy.out = Some("result".into());

            assert_eq!(simple_ir_read_names(&copy), vec!["source".to_string()]);
            assert_eq!(simple_ir_defined_names(&copy), vec!["result".to_string()]);
        }
    }

    #[test]
    fn every_var_result_sibling_defines_var_then_out() {
        for kind in ["checked_add", "checked_mul", "iter_next_unboxed"] {
            let mut multi = op(kind);
            multi.args = Some(vec!["lhs".into(), "rhs".into()]);
            multi.var = Some("primary".into());
            multi.out = Some("secondary".into());

            assert_eq!(
                simple_ir_read_names(&multi),
                vec!["lhs".to_string(), "rhs".to_string()],
                "{kind}"
            );
            assert_eq!(
                simple_ir_result_names(&multi),
                vec!["primary", "secondary"],
                "{kind}"
            );
            assert_eq!(
                simple_ir_defined_names(&multi),
                vec!["primary".to_string(), "secondary".to_string()],
                "{kind}"
            );
        }
    }

    #[test]
    fn side_effect_out_metadata_is_not_a_definition() {
        for kind in ["store_index", "module_cache_set", "dec_ref", "raise"] {
            let mut side_effect = op(kind);
            side_effect.args = Some(vec!["input".into()]);
            side_effect.out = Some("transport_only".into());

            assert!(simple_ir_result_names(&side_effect).is_empty(), "{kind}");
            assert_eq!(simple_ir_read_names(&side_effect), vec!["input"], "{kind}");
        }
    }

    #[test]
    fn generated_return_family_shares_one_value_carrier_authority() {
        let mut terminator = op("ret");
        terminator.args = Some(vec!["value".into()]);

        let mut values = Vec::new();
        visit_simple_ir_return_values(&terminator, |value| values.push(value));
        assert_eq!(values, ["value"]);
        assert!(simple_ir_return_has_value(&terminator));

        assert!(!simple_ir_return_has_value(&op("ret_void")));
        let mut non_terminator = op("call");
        non_terminator.args = Some(vec!["not_a_return".into()]);
        assert!(!simple_ir_return_has_value(&non_terminator));
    }
}
