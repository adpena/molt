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

impl CallableMetadata {
    pub fn from_functions(functions: &[FunctionIR]) -> Self {
        Self::from_functions_with_marker_filter(functions, |_| true)
    }

    /// Final bodies author constructor/escape facts, never source marker
    /// values that optimization may have transported through physical frames.
    pub fn from_definitions(functions: &[FunctionIR]) -> Self {
        Self::from_functions_with_marker_filter(functions, |_| false)
    }

    /// Only compiler-produced partition provenance may suppress source marker
    /// capture. The restrict-only codegen_partition flag is not provenance.
    pub fn from_functions_with_marker_filter(
        functions: &[FunctionIR],
        capture_markers: impl Fn(&FunctionIR) -> bool,
    ) -> Self {
        let mut metadata = Self::default();
        for function in functions.iter().filter(|function| !function.is_extern) {
            let capture_markers = capture_markers(function);
            // A binding records the callable visible at this program point.
            // None means a formerly known callable was overwritten: later
            // reserved markers must diagnose rather than reuse its old identity.
            let mut callable_names: BTreeMap<&str, Option<&str>> = BTreeMap::new();
            let mut constants = BTreeMap::new();
            for (op_index, op) in function.ops.iter().enumerate() {
                if capture_markers
                    && op.kind == "set_attr_generic_obj"
                    && let Some(attr) = op.s_value.as_deref()
                    && (attr == "__molt_closure_size__"
                        || TrampolineTaskKind::from_marker_attr(attr).is_some())
                {
                    let args = op.args.as_deref().unwrap_or_default();
                    assert!(
                        args.len() == 2,
                        "callable marker {attr} in {} at op {op_index} requires two operands",
                        function.name
                    );
                    match callable_names.get(args[0].as_str()) {
                        // Arbitrary runtime objects can carry these attributes;
                        // only a statically constructed callable authors this
                        // compile-time trampoline metadata.
                        None => {}
                        Some(None) => panic!(
                            "callable marker {attr} in {} at op {op_index} uses overwritten callable binding {}",
                            function.name, args[0]
                        ),
                        // Preserve the existing task-body eligibility gate:
                        // ordinary callables' runtime attributes do not author
                        // task metadata, regardless of their value.
                        Some(Some(name))
                            if attr != "__molt_closure_size__" && !name.ends_with("_poll") => {}
                        Some(Some(name)) => {
                            let value = constants.get(args[1].as_str()).copied().unwrap_or_else(|| panic!(
                                "callable marker {attr} for {name} in {} at op {op_index} requires a source-point integer value; {} is unknown",
                                function.name, args[1]
                            ));
                            if attr == "__molt_closure_size__" {
                                assert!(value >= 0, "negative closure size for {name}");
                                insert_consistent(
                                    &mut metadata.task_closure_sizes,
                                    name.to_string(),
                                    value,
                                    "callable closure size",
                                );
                            } else if value != 0 {
                                // Existing task-body eligibility only. This
                                // suffix never classifies generated ownership.
                                let kind = TrampolineTaskKind::from_marker_attr(attr)
                                    .expect("only task marker attributes are collected")
                                    .trampoline_kind();
                                insert_consistent(
                                    &mut metadata.task_kinds,
                                    name.to_string(),
                                    kind,
                                    "callable task kind",
                                );
                            }
                        }
                    }
                }

                // Reads, including marker interpretation, precede writes in
                // one operation. Every canonical definition invalidates stale
                // facts, not only definitions emitted by the cases below.
                if capture_markers {
                    crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
                        constants.remove(name);
                        if let Some(binding) = callable_names.get_mut(name) {
                            *binding = None;
                        }
                    });
                }
                match op.kind.as_str() {
                    "const" | "const_int" | "const_bool" if capture_markers => {
                        if let Some(name) = &op.out {
                            let value = op.value.unwrap_or(0);
                            constants.insert(
                                name.as_str(),
                                if op.kind == "const_bool" {
                                    i64::from(value != 0)
                                } else {
                                    value
                                },
                            );
                        }
                    }
                    "func_new" | "func_new_closure" => {
                        let Some(name) = &op.s_value else { continue };
                        let arity = usize::try_from(op.value.unwrap_or(0))
                            .unwrap_or_else(|_| panic!("negative callable arity for {name}"));
                        metadata.escaped_callable_targets.insert(name.clone());
                        if capture_markers && let Some(out) = &op.out {
                            callable_names.insert(out.as_str(), Some(name.as_str()));
                        }
                        insert_consistent(
                            &mut metadata.trampoline_specs,
                            name.clone(),
                            (arity, op.kind == "func_new_closure"),
                            "callable trampoline specification",
                        );
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

    /// Retain original facts across transformations that can separate the
    /// producer and marker into different physical functions. Conflicting
    /// facts are a compiler invariant violation, never last-writer-wins.
    pub fn merge(&mut self, other: Self) {
        self.escaped_callable_targets
            .extend(other.escaped_callable_targets);
        for (name, spec) in other.trampoline_specs {
            insert_consistent(
                &mut self.trampoline_specs,
                name,
                spec,
                "callable trampoline specification",
            );
        }
        for (name, kind) in other.task_kinds {
            insert_consistent(&mut self.task_kinds, name, kind, "callable task kind");
        }
        for (name, size) in other.task_closure_sizes {
            insert_consistent(
                &mut self.task_closure_sizes,
                name,
                size,
                "callable closure size",
            );
        }
    }
}

// These tests use transport metadata only; no runtime or target compiler.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpIR;

    fn fixture() -> FunctionIR {
        FunctionIR {
            name: "source".into(),
            ops: vec![
                OpIR {
                    kind: "func_new_closure".into(),
                    s_value: Some("worker_poll".into()),
                    value: Some(0),
                    out: Some("callable".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".into(),
                    value: Some(7),
                    out: Some("flag".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(3),
                    out: Some("size".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    s_value: Some("__molt_is_coroutine__".into()),
                    args: Some(vec!["callable".into(), "flag".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    s_value: Some("__molt_closure_size__".into()),
                    args: Some(vec!["callable".into(), "size".into()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }
    }

    #[test]
    fn source_metadata_survives_separated_producer_and_markers() {
        let function = fixture();
        let source = CallableMetadata::from_functions(std::slice::from_ref(&function));
        let mut prefix = function.clone();
        prefix.ops.truncate(3);
        let mut suffix = function;
        suffix.name = "physical_suffix".into();
        suffix.ops.drain(..3);
        let mut final_facts = CallableMetadata::from_definitions(&[prefix, suffix]);
        assert!(final_facts.task_kinds.is_empty());
        assert!(final_facts.task_closure_sizes.is_empty());
        final_facts.merge(source);
        assert_eq!(
            final_facts.task_kinds["worker_poll"],
            TrampolineKind::Coroutine
        );
        assert_eq!(final_facts.task_closure_sizes["worker_poll"], 3);
        assert_eq!(final_facts.trampoline_specs["worker_poll"], (0, true));
        assert!(final_facts.escaped_callable_targets.contains("worker_poll"));
    }

    #[test]
    fn metadata_is_deterministic_across_function_order_and_ignores_extern_ops() {
        let source = fixture();
        let mut declaration = source.clone();
        declaration.name = "external".into();
        declaration.is_extern = true;
        declaration.ops[0].value = Some(99);
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
        changed.ops[2].value = Some(4);
        let mut facts = CallableMetadata::from_functions(&[source]);
        facts.merge(CallableMetadata::from_functions(&[changed]));
    }

    #[test]
    #[should_panic(expected = "requires two operands")]
    fn malformed_marker_is_reported_with_function_context() {
        let mut source = fixture();
        source.ops[3].args = Some(vec!["callable".into()]);
        CallableMetadata::from_functions(&[source]);
    }
    #[test]
    fn marker_uses_earlier_value_and_callable_before_later_redefinitions() {
        let mut source = fixture();
        source.ops.push(OpIR {
            kind: "const_int".into(),
            value: Some(9),
            out: Some("size".into()),
            ..OpIR::default()
        });
        source.ops.push(OpIR {
            kind: "func_new_closure".into(),
            s_value: Some("other_poll".into()),
            value: Some(0),
            out: Some("callable".into()),
            ..OpIR::default()
        });
        let facts = CallableMetadata::from_functions(&[source]);
        assert_eq!(facts.task_closure_sizes["worker_poll"], 3);
        assert_eq!(facts.task_kinds["worker_poll"], TrampolineKind::Coroutine);
        assert!(!facts.task_closure_sizes.contains_key("other_poll"));
        assert!(!facts.task_kinds.contains_key("other_poll"));
    }

    #[test]
    #[should_panic(expected = "requires a source-point integer value")]
    fn arbitrary_result_redefinition_invalidates_marker_constant() {
        let mut source = fixture();
        source.ops.insert(
            4,
            OpIR {
                kind: "call".into(),
                s_value: Some("dynamic_size".into()),
                out: Some("size".into()),
                ..OpIR::default()
            },
        );
        CallableMetadata::from_functions(&[source]);
    }

    #[test]
    #[should_panic(expected = "uses overwritten callable binding")]
    fn arbitrary_result_redefinition_invalidates_callable_identity() {
        let mut source = fixture();
        source.ops.insert(
            3,
            OpIR {
                kind: "call".into(),
                s_value: Some("dynamic_callable".into()),
                out: Some("callable".into()),
                ..OpIR::default()
            },
        );
        CallableMetadata::from_functions(&[source]);
    }

    #[test]
    #[should_panic(expected = "requires a source-point integer value")]
    fn forward_constant_definition_does_not_author_marker_metadata() {
        let mut source = fixture();
        let size = source.ops.remove(2);
        source.ops.push(size);
        CallableMetadata::from_functions(&[source]);
    }

    #[test]
    fn optimization_restriction_flag_is_not_source_provenance() {
        let mut source = fixture();
        source.codegen_partition = true;
        let facts = CallableMetadata::from_functions(&[source]);
        assert_eq!(facts.task_closure_sizes["worker_poll"], 3);
        assert_eq!(facts.task_kinds["worker_poll"], TrampolineKind::Coroutine);
    }

    #[test]
    fn final_definition_scan_uses_retained_source_not_lowered_marker_values() {
        let source = fixture();
        let mut final_body = source.clone();
        final_body.ops[2] = OpIR {
            kind: "index".into(),
            args: Some(vec!["split_frame".into(), "slot".into()]),
            out: Some("size".into()),
            ..OpIR::default()
        };
        let mut facts = CallableMetadata::from_functions(&[source]);
        facts.merge(CallableMetadata::from_definitions(&[final_body]));
        assert_eq!(facts.task_closure_sizes["worker_poll"], 3);
    }

    #[test]
    fn ordinary_callable_runtime_task_attributes_do_not_require_static_values() {
        let mut source = fixture();
        source.ops[0].s_value = Some("ordinary_callable".into());
        source.ops[1] = OpIR {
            kind: "call".into(),
            s_value: Some("runtime_flag".into()),
            out: Some("flag".into()),
            ..OpIR::default()
        };
        let facts = CallableMetadata::from_functions(&[source]);
        assert!(facts.task_kinds.is_empty());
        assert_eq!(facts.task_closure_sizes["ordinary_callable"], 3);
    }
}
