use crate::OpIR;
use std::collections::BTreeSet;

pub(super) fn simple_ir_var_field_is_read(op: &OpIR) -> bool {
    if matches!(op.kind.as_str(), "copy_var" | "load_var")
        && op.args.as_ref().is_some_and(|args| !args.is_empty())
    {
        return false;
    }
    !matches!(
        op.kind.as_str(),
        // Assignment targets and fused iterator value outputs are definitions,
        // not source reads.
        "store_var" | "store_fast" | "iter_next_unboxed"
    )
}

pub(super) fn simple_ir_defined_names(op: &OpIR) -> Vec<&str> {
    let mut defined = Vec::new();
    if let Some(out) = op.out.as_deref()
        && out != "none"
    {
        defined.push(out);
    }
    if op.kind == "iter_next_unboxed"
        && let Some(var) = op.var.as_deref()
        && var != "none"
    {
        defined.push(var);
    }
    defined
}

fn push_split_name(out: &mut Vec<String>, seen: &mut BTreeSet<String>, name: &str) {
    if name != "none" && seen.insert(name.to_string()) {
        out.push(name.to_string());
    }
}

pub(super) fn split_ir_read_names(op: &OpIR) -> Vec<String> {
    let mut read = Vec::new();
    let mut seen = BTreeSet::new();
    match op.kind.as_str() {
        "unpack_sequence" => {
            if let Some(args) = op.args.as_ref()
                && let Some(seq) = args.first()
            {
                push_split_name(&mut read, &mut seen, seq);
            }
        }
        _ => {
            if let Some(args) = op.args.as_ref() {
                for arg in args {
                    push_split_name(&mut read, &mut seen, arg);
                }
            }
        }
    }
    if simple_ir_var_field_is_read(op)
        && let Some(var) = op.var.as_deref()
    {
        push_split_name(&mut read, &mut seen, var);
    }
    read
}

pub(super) fn split_ir_defined_names(op: &OpIR) -> Vec<String> {
    let mut defined = Vec::new();
    let mut seen = BTreeSet::new();
    for name in simple_ir_defined_names(op) {
        push_split_name(&mut defined, &mut seen, name);
    }
    match op.kind.as_str() {
        "store_var" | "store_fast" => {
            if let Some(var) = op.var.as_deref().or(op.out.as_deref()) {
                push_split_name(&mut defined, &mut seen, var);
            }
        }
        "unpack_sequence" => {
            if let Some(args) = op.args.as_ref() {
                for arg in args.iter().skip(1) {
                    push_split_name(&mut defined, &mut seen, arg);
                }
            }
        }
        _ => {}
    }
    defined
}
