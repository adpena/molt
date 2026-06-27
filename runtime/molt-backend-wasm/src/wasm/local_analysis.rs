use super::WasmFrameLocals;
use super::control_flow::has_non_linear_control_flow;
use crate::{FunctionIR, OpIR};
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

fn collect_read_vars(ops: &[OpIR]) -> BTreeSet<String> {
    let mut read_vars = BTreeSet::new();
    for op in ops {
        if let Some(args) = &op.args {
            read_vars.extend(args.iter().cloned());
        }
        if let Some(var) = &op.var {
            read_vars.insert(var.clone());
        }
    }
    read_vars
}

fn runtime_lookup_only_vars(ops: &[OpIR]) -> BTreeSet<String> {
    let mut runtime_lookup_vars: BTreeSet<String> = BTreeSet::new();
    for op in ops {
        if op.kind == "builtin_func"
            && op.s_value.as_deref() == Some("molt_require_intrinsic_runtime")
            && let Some(out) = op.out.as_ref()
        {
            runtime_lookup_vars.insert(out.clone());
        }
    }

    let mut runtime_lookup_only_vars = runtime_lookup_vars.clone();
    for op in ops {
        if let Some(var) = op.var.as_ref()
            && runtime_lookup_vars.contains(var)
        {
            runtime_lookup_only_vars.remove(var);
        }
        if let Some(args) = op.args.as_ref() {
            for (idx, arg) in args.iter().enumerate() {
                if !runtime_lookup_vars.contains(arg) {
                    continue;
                }
                let only_runtime_dispatch = op.kind == "call_func" && idx == 0 && args.len() == 3;
                if !only_runtime_dispatch {
                    runtime_lookup_only_vars.remove(arg);
                }
            }
        }
    }
    runtime_lookup_only_vars
}

fn coalesced_locals(
    func_ir: &FunctionIR,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    if has_non_linear_control_flow(&func_ir.ops) {
        return BTreeMap::new();
    }

    let mut first_write: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_read: BTreeMap<String, usize> = BTreeMap::new();
    for (op_idx, op) in func_ir.ops.iter().enumerate() {
        if let Some(out) = &op.out {
            first_write.entry(out.clone()).or_insert(op_idx);
        }
        if let Some(args) = &op.args {
            for arg in args {
                last_read.insert(arg.clone(), op_idx);
            }
        }
        if let Some(var) = &op.var {
            last_read.insert(var.clone(), op_idx);
        }
    }

    let mut ranges: Vec<(usize, usize, String)> = Vec::new();
    for (name, start) in &first_write {
        if !is_coalescable_local(name, read_vars, param_set) {
            continue;
        }
        let end = last_read.get(name).copied().unwrap_or(*start);
        ranges.push((*start, end, name.clone()));
    }
    ranges.sort_by_key(|range| range.0);

    let mut slot_end: Vec<usize> = Vec::new();
    let mut slot_repr: Vec<String> = Vec::new();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (start, end, name) in &ranges {
        let mut assigned = false;
        for (idx, slot_end_idx) in slot_end.iter_mut().enumerate() {
            if *slot_end_idx < *start {
                *slot_end_idx = *end;
                map.insert(name.clone(), slot_repr[idx].clone());
                assigned = true;
                break;
            }
        }
        if !assigned {
            slot_end.push(*end);
            slot_repr.push(name.clone());
            map.insert(name.clone(), name.clone());
        }
    }
    map
}

fn is_coalescable_local(
    name: &str,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> bool {
    WasmFrameLocals::is_coalescable_value_name(name, read_vars, param_set)
}

fn defined_and_used_value_vars(ops: &[OpIR]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut defined_vars = BTreeSet::new();
    let mut used_vars = BTreeSet::new();
    for op in ops {
        if let Some(args) = &op.args {
            for arg in args {
                if arg != "self" && arg != "none" && arg.starts_with('v') {
                    used_vars.insert(arg.clone());
                }
            }
        }
        if let Some(out) = &op.out
            && out != "none"
        {
            defined_vars.insert(out.clone());
        }
    }
    (defined_vars, used_vars)
}

#[cfg(test)]
mod tests {
    use super::*;

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
