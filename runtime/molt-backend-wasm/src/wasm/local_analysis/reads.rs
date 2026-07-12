use crate::OpIR;
use std::collections::BTreeSet;

pub(super) fn collect_read_vars(ops: &[OpIR]) -> BTreeSet<String> {
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
