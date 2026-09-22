//! Source callable facts captured before physical function partitioning.
//! Symbol names in these maps are referenced callable identities, not names
//! inferred from generated partition spelling.

use super::{TrampolineKind, TrampolineTaskKind};
use crate::FunctionIR;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct CallableMetadata {
    pub escaped_callable_targets: BTreeSet<String>,
    pub trampoline_specs: BTreeMap<String, (usize, bool)>,
    pub task_kinds: BTreeMap<String, TrampolineKind>,
    pub task_closure_sizes: BTreeMap<String, i64>,
}

fn insert_consistent<T: std::fmt::Debug + PartialEq>(
    facts: &mut BTreeMap<String, T>,
    name: String,
    value: T,
    fact: &str,
) {
    if let Some(previous) = facts.get(&name) {
        assert_eq!(previous, &value, "conflicting {fact} for {name}");
    } else {
        facts.insert(name, value);
    }
}

/// Merge immutable callable facts from source and partition custody. Conflicts
/// indicate stale or inconsistent compiler inputs, never an override order.
pub fn merge_callable_facts<T: std::fmt::Debug + PartialEq>(
    retained: &mut BTreeMap<String, T>,
    incoming: BTreeMap<String, T>,
    fact: &str,
) {
    for (name, value) in incoming {
        insert_consistent(retained, name, value, fact);
    }
}

impl CallableMetadata {
    pub fn from_functions(functions: &[FunctionIR]) -> Self {
        let mut metadata = Self::default();
        for function in functions.iter().filter(|function| !function.is_extern) {
            for (op_index, op) in function.ops.iter().enumerate() {
                match op.kind.as_str() {
                    "func_new" | "func_new_closure" => {
                        let name = op.s_value.as_ref().unwrap_or_else(|| {
                            panic!(
                                "{} in {} at op {op_index} requires a callable target",
                                op.kind, function.name
                            )
                        });
                        let arity = usize::try_from(op.value.unwrap_or(0))
                            .unwrap_or_else(|_| panic!("negative callable arity for {name}"));
                        metadata.escaped_callable_targets.insert(name.clone());
                        insert_consistent(
                            &mut metadata.trampoline_specs,
                            name.clone(),
                            (arity, op.kind == "func_new_closure"),
                            "callable trampoline specification",
                        );
                        match (op.task_kind.as_deref(), op.task_closure_size) {
                            (None, None) => {
                                insert_consistent(
                                    &mut metadata.task_kinds,
                                    name.clone(),
                                    TrampolineKind::Plain,
                                    "callable task kind",
                                );
                            }
                            (Some(kind), Some(size)) => {
                                assert!(
                                    size >= 0,
                                    "negative callable task_closure_size for {name} in {} at op {op_index}",
                                    function.name
                                );
                                let kind = TrampolineTaskKind::from_constructor_kind(kind)
                                    .unwrap_or_else(|| {
                                        panic!(
                                            "unknown callable task_kind `{kind}` for {name} in {} at op {op_index}",
                                            function.name
                                        )
                                    })
                                    .trampoline_kind();
                                insert_consistent(
                                    &mut metadata.task_kinds,
                                    name.clone(),
                                    kind,
                                    "callable task kind",
                                );
                                insert_consistent(
                                    &mut metadata.task_closure_sizes,
                                    name.clone(),
                                    size,
                                    "callable closure size",
                                );
                            }
                            _ => panic!(
                                "partial callable task metadata for {name} in {} at op {op_index}: task_kind and task_closure_size must be present together",
                                function.name
                            ),
                        }
                    }
                    "builtin_func" => {
                        if let Some(name) = &op.s_value {
                            metadata.escaped_callable_targets.insert(name.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        metadata
    }

    /// Retain original constructor facts across transformations that can move
    /// or erase the producer in physical partitions. Conflicting facts are a
    /// compiler invariant violation, never last-writer-wins.
    pub fn merge(&mut self, other: Self) {
        self.escaped_callable_targets
            .extend(other.escaped_callable_targets);
        merge_callable_facts(
            &mut self.trampoline_specs,
            other.trampoline_specs,
            "callable trampoline specification",
        );
        merge_callable_facts(&mut self.task_kinds, other.task_kinds, "callable task kind");
        merge_callable_facts(
            &mut self.task_closure_sizes,
            other.task_closure_sizes,
            "callable closure size",
        );
    }
}

// These tests use transport metadata only; no runtime or target compiler.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpIR;

    fn task_constructor(kind: &str, size: i64) -> OpIR {
        OpIR {
            kind: "func_new_closure".into(),
            s_value: Some("worker_poll".into()),
            value: Some(0),
            out: Some("callable".into()),
            task_kind: Some(kind.into()),
            task_closure_size: Some(size),
            ..OpIR::default()
        }
    }

    fn fixture() -> FunctionIR {
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "source".into(),
            ops: vec![task_constructor("coroutine", 3)],
            ..FunctionIR::default()
        }
    }

    #[test]
    fn constructor_metadata_survives_physical_partition_separation() {
        let source = fixture();
        let mut retained = CallableMetadata::from_functions(std::slice::from_ref(&source));
        let physical = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "physical_suffix".into(),
            ops: vec![OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            }],
            codegen_partition: true,
            ..FunctionIR::default()
        };
        retained.merge(CallableMetadata::from_functions(&[physical]));
        assert_eq!(
            retained.task_kinds["worker_poll"],
            TrampolineKind::Coroutine
        );
        assert_eq!(retained.task_closure_sizes["worker_poll"], 3);
        assert_eq!(retained.trampoline_specs["worker_poll"], (0, true));
    }

    #[test]
    fn all_constructor_task_tokens_map_without_symbol_inference() {
        for (token, expected) in [
            ("generator", TrampolineKind::Generator),
            ("coroutine", TrampolineKind::Coroutine),
            ("async_generator", TrampolineKind::AsyncGen),
        ] {
            let function = FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                ops: vec![task_constructor(token, 8)],
                ..FunctionIR::default()
            };
            let facts = CallableMetadata::from_functions(&[function]);
            assert_eq!(facts.task_kinds["worker_poll"], expected);
            assert_eq!(facts.task_closure_sizes["worker_poll"], 8);
        }
    }

    #[test]
    fn direct_constructor_authors_no_task_facts() {
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![OpIR {
                kind: "func_new".into(),
                s_value: Some("ordinary_callable".into()),
                value: Some(2),
                ..OpIR::default()
            }],
            ..FunctionIR::default()
        };
        let facts = CallableMetadata::from_functions(&[function]);
        assert_eq!(facts.task_kinds["ordinary_callable"], TrampolineKind::Plain);
        assert!(facts.task_closure_sizes.is_empty());
        assert_eq!(facts.trampoline_specs["ordinary_callable"], (2, false));
    }

    #[test]
    fn metadata_is_deterministic_across_function_order_and_ignores_extern_ops() {
        let source = fixture();
        let mut declaration = source.clone();
        declaration.name = "external".into();
        declaration.is_extern = true;
        declaration.ops[0].task_closure_size = Some(99);
        for functions in [
            vec![source.clone(), declaration.clone()],
            vec![declaration, source],
        ] {
            let facts = CallableMetadata::from_functions(&functions);
            assert_eq!(facts.trampoline_specs["worker_poll"], (0, true));
            assert_eq!(facts.task_kinds.len(), 1);
            assert_eq!(facts.task_closure_sizes.len(), 1);
        }
    }

    #[test]
    #[should_panic(expected = "conflicting callable closure size")]
    fn metadata_merge_rejects_conflicting_sizes() {
        let source = fixture();
        let mut changed = source.clone();
        changed.ops[0].task_closure_size = Some(4);
        let mut facts = CallableMetadata::from_functions(&[source]);
        facts.merge(CallableMetadata::from_functions(&[changed]));
    }

    #[test]
    #[should_panic(expected = "conflicting callable task kind")]
    fn direct_and_task_constructors_cannot_disagree_across_partitions() {
        let source = fixture();
        let mut direct = source.clone();
        direct.ops[0].task_kind = None;
        direct.ops[0].task_closure_size = None;
        let mut facts = CallableMetadata::from_functions(&[source]);
        facts.merge(CallableMetadata::from_functions(&[direct]));
    }

    #[test]
    #[should_panic(expected = "partial callable task metadata")]
    fn partial_constructor_task_metadata_is_rejected() {
        let mut source = fixture();
        source.ops[0].task_closure_size = None;
        CallableMetadata::from_functions(&[source]);
    }

    #[test]
    #[should_panic(expected = "unknown callable task_kind")]
    fn unknown_constructor_task_kind_is_rejected() {
        let mut source = fixture();
        source.ops[0].task_kind = Some("future".into());
        CallableMetadata::from_functions(&[source]);
    }

    #[test]
    #[should_panic(expected = "negative callable task_closure_size")]
    fn negative_constructor_task_closure_size_is_rejected() {
        let mut source = fixture();
        source.ops[0].task_closure_size = Some(-1);
        CallableMetadata::from_functions(&[source]);
    }
}
