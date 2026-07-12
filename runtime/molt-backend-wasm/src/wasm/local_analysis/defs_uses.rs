use crate::OpIR;
use std::collections::BTreeSet;

pub(super) fn defined_and_used_value_vars(ops: &[OpIR]) -> (BTreeSet<String>, BTreeSet<String>) {
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
