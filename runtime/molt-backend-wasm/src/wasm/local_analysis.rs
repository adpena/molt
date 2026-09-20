mod coalescing;
mod runtime_lookup;

use self::coalescing::coalesced_locals;
use self::runtime_lookup::runtime_lookup_only_vars;
use crate::{FunctionIR, OpIR};
use molt_tir::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct LocalVariableAnalysis {
    pub(super) read_vars: BTreeSet<String>,
    pub(super) param_set: BTreeSet<String>,
    pub(super) runtime_lookup_only_vars: BTreeSet<String>,
    pub(super) coalesced_map: BTreeMap<String, String>,
    pub(super) defined_vars: BTreeSet<String>,
    pub(super) used_vars: BTreeSet<String>,
}

pub(super) fn analyze_local_variables(func_ir: &FunctionIR) -> LocalVariableAnalysis {
    let (read_vars, defined_vars) = collect_value_names(&func_ir.ops);
    let param_set: BTreeSet<String> = func_ir.params.iter().cloned().collect();
    let runtime_lookup_only_vars = runtime_lookup_only_vars(&func_ir.ops);
    let coalesced_map = coalesced_locals(func_ir, &read_vars, &param_set);
    // Keep the existing dispatch undefined-value seeding policy separate from
    // field roles. It is a projection of actual reads, not another IR walker.
    let used_vars = read_vars
        .iter()
        .filter(|name| name.starts_with('v'))
        .cloned()
        .collect();

    LocalVariableAnalysis {
        read_vars,
        param_set,
        runtime_lookup_only_vars,
        coalesced_map,
        defined_vars,
        used_vars,
    }
}

fn collect_value_names(ops: &[OpIR]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut reads = BTreeSet::new();
    let mut definitions = BTreeSet::new();
    for op in ops {
        visit_simple_ir_reads(op, |read| {
            reads.insert(read.name.to_string());
        });
        visit_simple_ir_defined_names(op, |name| {
            definitions.insert(name.to_string());
        });
    }
    (reads, definitions)
}

#[cfg(test)]
mod tests {
    use super::{coalesced_locals, collect_value_names};
    use crate::OpIR;
    use std::collections::BTreeSet;

    fn op(kind: &str, args: Option<Vec<&str>>, var: Option<&str>, out: Option<&str>) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: args.map(|a| a.into_iter().map(String::from).collect()),
            var: var.map(String::from),
            out: out.map(String::from),
            ..OpIR::default()
        }
    }

    #[test]
    fn read_vars_includes_args_and_var() {
        let ops = vec![
            op("add", Some(vec!["a", "b"]), None, Some("c")),
            op("load", None, Some("d"), Some("e")),
        ];
        let (read_vars, _) = collect_value_names(&ops);
        assert!(read_vars.contains("a"), "arg 'a' should be in read set");
        assert!(read_vars.contains("b"), "arg 'b' should be in read set");
        assert!(read_vars.contains("d"), "var 'd' should be in read set");
        assert!(
            !read_vars.contains("c"),
            "output-only 'c' should NOT be in read set"
        );
        assert!(
            !read_vars.contains("e"),
            "output-only 'e' should NOT be in read set"
        );
    }

    #[test]
    fn read_vars_output_becomes_live_when_later_read() {
        let ops = vec![
            op("const", None, None, Some("x")),
            op("add", Some(vec!["x", "y"]), None, Some("z")),
        ];
        let (read_vars, _) = collect_value_names(&ops);
        assert!(
            read_vars.contains("x"),
            "'x' should be live since it's read by add"
        );
        assert!(read_vars.contains("y"), "'y' should be live");
        assert!(
            !read_vars.contains("z"),
            "'z' is output-only, should be dead"
        );
    }

    #[test]
    fn dead_local_all_outputs_dead() {
        let ops = vec![
            op("const", None, None, Some("a")),
            op("const", None, None, Some("b")),
            op("const", None, None, Some("c")),
        ];
        let (read_vars, _) = collect_value_names(&ops);
        assert!(read_vars.is_empty(), "no variable is ever read");
    }

    #[test]
    fn binding_destinations_snapshots_and_unpack_outputs_share_field_roles() {
        let ops = vec![
            op(
                "store_var",
                Some(vec!["source"]),
                Some("slot"),
                Some("snapshot"),
            ),
            op(
                "store_var",
                Some(vec!["source"]),
                None,
                Some("binding_only"),
            ),
            op(
                "store_var",
                Some(vec!["source"]),
                Some("discarded_slot"),
                Some("none"),
            ),
            op(
                "unpack_sequence",
                Some(vec!["sequence", "first", "second"]),
                None,
                None,
            ),
            op(
                "copy_var",
                Some(vec!["source"]),
                Some("metadata_only"),
                Some("copy"),
            ),
        ];
        let (reads, definitions) = collect_value_names(&ops);
        assert_eq!(reads, BTreeSet::from(["source".into(), "sequence".into()]));
        assert_eq!(
            definitions,
            BTreeSet::from([
                "slot".into(),
                "snapshot".into(),
                "binding_only".into(),
                "discarded_slot".into(),
                "first".into(),
                "second".into(),
                "copy".into(),
            ])
        );
    }

    #[test]
    fn coalescing_keeps_late_binding_writes_out_of_a_reused_live_slot() {
        let function = crate::FunctionIR {
            params: vec!["source".into(), "replacement".into()],
            ops: vec![
                op("store_var", Some(vec!["source"]), Some("__tmp_slot"), None),
                op("inc_ref", Some(vec!["__tmp_slot"]), None, None),
                op("store_var", Some(vec!["source"]), Some("__tmp_live"), None),
                op(
                    "store_var",
                    Some(vec!["replacement"]),
                    Some("__tmp_slot"),
                    None,
                ),
                op("ret", Some(vec!["__tmp_live"]), None, None),
            ],
            ..crate::FunctionIR::default()
        };
        let (reads, _) = collect_value_names(&function.ops);
        let params = function.params.iter().cloned().collect();
        let coalesced = coalesced_locals(&function, &reads, &params);
        assert_ne!(coalesced["__tmp_slot"], coalesced["__tmp_live"]);
    }
}
