use crate::OpIR;
use molt_tir::tir::simple_def_use::{
    SimpleIrReadField, visit_simple_ir_defined_names, visit_simple_ir_reads,
};
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
        let declares_lookup = op.kind == "builtin_func"
            && op.s_value.as_deref() == Some("molt_require_intrinsic_runtime");
        if !declares_lookup {
            visit_simple_ir_defined_names(op, |name| {
                runtime_lookup_only_vars.remove(name);
            });
        }
        visit_simple_ir_reads(op, |read| {
            let only_runtime_dispatch = op.kind == "call_func"
                && read.field == SimpleIrReadField::Arg(0)
                && op.args.as_ref().is_some_and(|args| args.len() == 3);
            if !only_runtime_dispatch {
                runtime_lookup_only_vars.remove(read.name);
            }
        });
    }
    runtime_lookup_only_vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup() -> OpIR {
        OpIR {
            kind: "builtin_func".into(),
            out: Some("lookup".into()),
            s_value: Some("molt_require_intrinsic_runtime".into()),
            ..OpIR::default()
        }
    }

    fn call() -> OpIR {
        OpIR {
            kind: "call_func".into(),
            args: Some(vec!["lookup".into(), "name".into(), "fallback".into()]),
            out: Some("result".into()),
            ..OpIR::default()
        }
    }

    #[test]
    fn binding_and_result_redefinitions_invalidate_runtime_only_lookup() {
        for definition in [
            OpIR {
                kind: "store_var".into(),
                var: Some("lookup".into()),
                args: Some(vec!["replacement".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("slot".into()),
                out: Some("lookup".into()),
                args: Some(vec!["replacement".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".into(),
                out: Some("lookup".into()),
                ..OpIR::default()
            },
        ] {
            assert!(runtime_lookup_only_vars(&[lookup(), definition, call()]).is_empty());
        }
    }

    #[test]
    fn source_name_metadata_does_not_observe_runtime_lookup_value() {
        let metadata = OpIR {
            kind: "copy_var".into(),
            var: Some("lookup".into()),
            args: Some(vec!["other_source".into()]),
            out: Some("copy".into()),
            ..OpIR::default()
        };
        assert_eq!(
            runtime_lookup_only_vars(&[lookup(), metadata, call()]),
            BTreeSet::from(["lookup".into()])
        );
    }
}
