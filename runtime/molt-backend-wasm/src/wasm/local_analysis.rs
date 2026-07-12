mod coalescing;
mod defs_uses;
mod reads;
mod runtime_lookup;

use self::coalescing::coalesced_locals;
use self::defs_uses::defined_and_used_value_vars;
use self::reads::collect_read_vars;
use self::runtime_lookup::runtime_lookup_only_vars;
use crate::FunctionIR;
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
    let read_vars = collect_read_vars(&func_ir.ops);
    let param_set: BTreeSet<String> = func_ir.params.iter().cloned().collect();
    let runtime_lookup_only_vars = runtime_lookup_only_vars(&func_ir.ops);
    let coalesced_map = coalesced_locals(func_ir, &read_vars, &param_set);
    let (defined_vars, used_vars) = defined_and_used_value_vars(&func_ir.ops);

    LocalVariableAnalysis {
        read_vars,
        param_set,
        runtime_lookup_only_vars,
        coalesced_map,
        defined_vars,
        used_vars,
    }
}

#[cfg(test)]
mod tests {
    use super::collect_read_vars;
    use crate::OpIR;

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
        let read_vars = collect_read_vars(&ops);
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
        let read_vars = collect_read_vars(&ops);
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
        let read_vars = collect_read_vars(&ops);
        assert!(read_vars.is_empty(), "no variable is ever read");
    }
}
