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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleIrResultField {
    Var,
    Out,
    Arg(usize),
}

/// One declared result position. A missing or reserved `none` name discards
/// that position; it never shifts a sibling result into a different role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpleIrResult<'a> {
    pub name: Option<&'a str>,
    pub field: SimpleIrResultField,
}

/// A mutable binding and its optional value snapshot. This borrowed field-role
/// view does not validate names or expand any backend's admitted operation set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpleIrBinding<'a> {
    pub destination: &'a str,
    pub result: Option<&'a str>,
}

/// Interpret binding fields once for every def/use and backend consumer.
/// Binding-only `out` is a destination; only a distinct, non-`none`, non-metadata
/// output beside an explicit `var` is a value snapshot. Delete output metadata
/// never becomes a snapshot. Reserved destination filtering remains with the
/// definition visitor and existing admission checks.
pub fn simple_ir_binding(op: &OpIR) -> Option<SimpleIrBinding<'_>> {
    if simpleir_var_field_role_table(op.kind.as_str()) != SimpleIrVarFieldRole::Definition {
        return None;
    }
    let destination = op.var.as_deref().or(op.out.as_deref())?;
    let result = op.out.as_deref().filter(|out| {
        *out != "none" && *out != destination && !simpleir_out_field_is_metadata(op.kind.as_str())
    });
    Some(SimpleIrBinding {
        destination,
        result,
    })
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

/// Visit result field roles, preserving absent/discarded positions. Bindings
/// expose only their optional snapshot, never their mutable destination, and
/// generated out metadata is not a result.
pub fn visit_simple_ir_results<'a>(op: &'a OpIR, mut visit: impl FnMut(SimpleIrResult<'a>)) {
    if simpleir_var_field_role_table(op.kind.as_str()) == SimpleIrVarFieldRole::Result {
        visit(SimpleIrResult {
            name: op.var.as_deref().filter(|name| *name != "none"),
            field: SimpleIrResultField::Var,
        });
    }
    if !simpleir_out_field_is_metadata(op.kind.as_str()) {
        let name = if let Some(binding) = simple_ir_binding(op) {
            binding.result
        } else {
            op.out.as_deref().filter(|name| *name != "none")
        };
        visit(SimpleIrResult {
            name,
            field: SimpleIrResultField::Out,
        });
    }
    if let Some(first_result) = simpleir_first_trailing_result_arg_table(op.kind.as_str())
        && let Some(args) = op.args.as_deref()
    {
        for (index, name) in args.iter().enumerate().skip(first_result) {
            visit(SimpleIrResult {
                name: (name != "none").then_some(name.as_str()),
                field: SimpleIrResultField::Arg(index),
            });
        }
    }
}

/// Project the ordinary output's value role without reinterpreting metadata
/// or a binding-only destination as a runtime result.
pub fn simple_ir_out_result(op: &OpIR) -> Option<&str> {
    let mut name = None;
    visit_simple_ir_results(op, |result| {
        if result.field == SimpleIrResultField::Out {
            name = result.name;
        }
    });
    name
}

/// Visit live result definitions without allocating. Consumers requiring ABI
/// positions use `visit_simple_ir_results`, not this filtered name projection.
pub fn visit_simple_ir_result_names<'a>(op: &'a OpIR, mut visit: impl FnMut(&'a str)) {
    visit_simple_ir_results(op, |result| {
        if let Some(name) = result.name {
            visit(name);
        }
    });
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

// Compute positions before borrowing their values. Immutable analysis and
// in-place rewriting must visit the identical generated field-role projection.
fn simple_ir_read_fields(op: &OpIR) -> impl Iterator<Item = SimpleIrReadField> + use<> {
    let arg_count = op.args.as_ref().map_or(0, Vec::len);
    let read_arity = simpleir_first_trailing_result_arg_table(op.kind.as_str())
        .unwrap_or(arg_count)
        .min(arg_count);
    (0..read_arity).map(SimpleIrReadField::Arg).chain(
        (simple_ir_var_field_is_read(op) && op.var.is_some()).then_some(SimpleIrReadField::Var),
    )
}

/// Every source read and its canonical field role, in deterministic order.
///
/// Consumers that need positional diagnostics or narrowly-scoped transport
/// exceptions use this API directly. Name-set consumers should insert the
/// borrowed names into their own long-lived set.
pub fn visit_simple_ir_reads<'a>(op: &'a OpIR, mut visit: impl FnMut(SimpleIrRead<'a>)) {
    for field in simple_ir_read_fields(op) {
        let name = match field {
            SimpleIrReadField::Arg(index) => {
                &op.args.as_ref().expect("read argument exists")[index]
            }
            SimpleIrReadField::Var => op.var.as_ref().expect("read variable exists"),
        };
        visit(SimpleIrRead { name, field });
    }
}

/// A unary semantic input, independent of its transport field. Missing or
/// multiple reads fail closed rather than selecting an arbitrary first value.
pub fn simple_ir_single_read(op: &OpIR) -> Option<SimpleIrRead<'_>> {
    let mut source = None;
    let mut count = 0;
    visit_simple_ir_reads(op, |read| {
        count += 1;
        source = Some(read);
    });
    source.filter(|_| count == 1)
}

/// Rewrite only semantic source names. Results, binding destinations and
/// metadata are inaccessible to the callback, even when they collide with a
/// read name. This uses the same allocation-free field walk as read analysis.
pub fn visit_simple_ir_reads_mut(
    op: &mut OpIR,
    mut visit: impl FnMut(SimpleIrReadField, &mut String),
) {
    for field in simple_ir_read_fields(op) {
        let name = match field {
            SimpleIrReadField::Arg(index) => {
                &mut op.args.as_mut().expect("read argument exists")[index]
            }
            SimpleIrReadField::Var => op.var.as_mut().expect("read variable exists"),
        };
        visit(field, name);
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
    if let Some(binding) = simple_ir_binding(op)
        && binding.destination != "none"
    {
        visit(binding.destination);
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
    fn mutable_reads_share_field_roles_and_preserve_nonread_collisions() {
        for kind in [
            "copy",
            "copy_var",
            "load_var",
            "store_var",
            "store_fast",
            "delete_var",
            "checked_add",
            "checked_mul",
            "iter_next_unboxed",
            "unpack_sequence",
            "store_index",
            "ret",
            "ret_void",
            "unmapped_transport",
        ] {
            for args in [None, Some(vec![]), Some(vec!["same".into(); 3])] {
                for var in [None, Some("same".into())] {
                    let mut input = op(kind);
                    input.args = args.clone();
                    input.var = var;
                    input.out = Some("same".into());
                    let mut expected = input.clone();
                    let mut fields = Vec::new();
                    visit_simple_ir_reads(&input, |read| {
                        fields.push(read.field);
                        match read.field {
                            SimpleIrReadField::Arg(index) => {
                                expected.args.as_mut().unwrap()[index] = "rewritten".into();
                            }
                            SimpleIrReadField::Var => expected.var = Some("rewritten".into()),
                        }
                    });
                    let mut mutated_fields = Vec::new();
                    visit_simple_ir_reads_mut(&mut input, |field, name| {
                        mutated_fields.push(field);
                        *name = "rewritten".into();
                    });
                    assert_eq!(mutated_fields, fields, "{kind}");
                    assert_eq!(input.args, expected.args, "{kind}");
                    assert_eq!(input.var, expected.var, "{kind}");
                    assert_eq!(input.out, expected.out, "{kind}");
                }
            }
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
    fn local_stores_distinguish_binding_destinations_from_optional_value_results() {
        for kind in ["store_var", "store_fast"] {
            for (binding, output, results, definitions) in [
                (None, Some("local"), vec![], vec!["local"]),
                (Some("local"), None, vec![], vec!["local"]),
                (Some("local"), Some("local"), vec![], vec!["local"]),
                (Some("local"), Some("none"), vec![], vec!["local"]),
                (
                    Some("local"),
                    Some("result"),
                    vec!["result"],
                    vec!["result", "local"],
                ),
            ] {
                let mut store = op(kind);
                store.var = binding.map(str::to_string);
                store.out = output.map(str::to_string);
                store.args = Some(vec!["source".into()]);
                assert_eq!(simple_ir_result_names(&store), results, "{store:?}");
                assert_eq!(simple_ir_defined_names(&store), definitions, "{store:?}");
                assert_eq!(simple_ir_read_names(&store), vec!["source"], "{store:?}");
                assert_eq!(
                    simple_ir_binding(&store),
                    Some(SimpleIrBinding {
                        destination: binding.or(output).unwrap(),
                        result: results.first().copied(),
                    }),
                    "{store:?}"
                );
            }
        }
        let mut delete = op("delete_var");
        delete.var = Some("local".into());
        delete.out = Some("diagnostic_metadata".into());
        assert!(simple_ir_result_names(&delete).is_empty());
        assert_eq!(simple_ir_defined_names(&delete), vec!["local"]);
        assert_eq!(
            simple_ir_binding(&delete),
            Some(SimpleIrBinding {
                destination: "local",
                result: None
            })
        );
    }

    #[test]
    fn binding_view_preserves_field_shape_without_becoming_name_admission() {
        for kind in ["store_var", "store_fast", "delete_var"] {
            assert_eq!(simple_ir_binding(&op(kind)), None);
            for destination in ["none", ""] {
                let binding = OpIR {
                    var: Some(destination.into()),
                    out: Some("snapshot".into()),
                    ..op(kind)
                };
                let result = (kind != "delete_var").then_some("snapshot");
                assert_eq!(
                    simple_ir_binding(&binding),
                    Some(SimpleIrBinding {
                        destination,
                        result
                    })
                );
                let mut expected = result.into_iter().collect::<Vec<_>>();
                if destination != "none" {
                    expected.push(destination);
                }
                assert_eq!(simple_ir_defined_names(&binding), expected);
            }
        }
        for kind in ["copy_var", "load_var", "iter_next_unboxed", "unknown"] {
            let nonbinding = OpIR {
                var: Some("source".into()),
                out: Some("result".into()),
                ..op(kind)
            };
            assert_eq!(simple_ir_binding(&nonbinding), None, "{kind}");
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
    fn positional_result_fields_keep_discarded_sibling_positions() {
        for kind in ["checked_add", "checked_mul", "iter_next_unboxed"] {
            for discarded in [None, Some("none")] {
                for discard_first in [true, false] {
                    let mut multi = op(kind);
                    multi.var = if discard_first {
                        discarded
                    } else {
                        Some("primary")
                    }
                    .map(str::to_string);
                    multi.out = if discard_first {
                        Some("secondary")
                    } else {
                        discarded
                    }
                    .map(str::to_string);
                    let mut results = Vec::new();
                    visit_simple_ir_results(&multi, |result| results.push(result));
                    assert_eq!(
                        results,
                        [
                            SimpleIrResult {
                                field: SimpleIrResultField::Var,
                                name: (!discard_first).then_some("primary")
                            },
                            SimpleIrResult {
                                field: SimpleIrResultField::Out,
                                name: discard_first.then_some("secondary")
                            },
                        ],
                        "{kind}"
                    );
                    assert_eq!(
                        simple_ir_result_names(&multi),
                        [if discard_first {
                            "secondary"
                        } else {
                            "primary"
                        }]
                    );
                }
            }
        }
        let mut unpack = op("unpack_sequence");
        unpack.args = Some(vec![
            "source".into(),
            "first".into(),
            "none".into(),
            "third".into(),
        ]);
        let mut arguments = Vec::new();
        visit_simple_ir_results(&unpack, |result| {
            if matches!(result.field, SimpleIrResultField::Arg(_)) {
                arguments.push(result);
            }
        });
        assert_eq!(
            arguments,
            [
                SimpleIrResult {
                    field: SimpleIrResultField::Arg(1),
                    name: Some("first")
                },
                SimpleIrResult {
                    field: SimpleIrResultField::Arg(2),
                    name: None
                },
                SimpleIrResult {
                    field: SimpleIrResultField::Arg(3),
                    name: Some("third")
                },
            ]
        );
    }

    #[test]
    fn ordinary_result_projection_excludes_metadata_and_binding_only_destinations() {
        for kind in [
            "store_index",
            "store",
            "guarded_field_set",
            "raise",
            "store_var",
        ] {
            let mut effect = op(kind);
            effect.out = Some("existing".into());
            assert_eq!(simple_ir_out_result(&effect), None, "{kind}");
        }
        let mut binding = op("store_var");
        binding.var = Some("slot".into());
        binding.out = Some("snapshot".into());
        assert_eq!(simple_ir_out_result(&binding), Some("snapshot"));
        let mut checked = op("checked_add");
        checked.var = Some("value".into());
        checked.out = Some("overflow".into());
        assert_eq!(simple_ir_out_result(&checked), Some("overflow"));
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
