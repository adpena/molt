use crate::OpIR;
use std::collections::BTreeSet;

pub(super) fn runtime_lookup_only_vars(ops: &[OpIR]) -> BTreeSet<String> {
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
