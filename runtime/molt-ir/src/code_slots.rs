//! Program-owned bindings for stable compiled-code symbols.
//!
//! Function bodies carry symbols; only the final program assigns dense runtime
//! indices. Cached object jobs import these bindings without carrying their
//! application-specific values. Class statement symbols (HF-220) share this
//! index space: a statement's slot is the identity its classes carry.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::FunctionIR;
use crate::ir_schema::{FunctionOpShapeDiagnostic, OpShapeDiagnostic, OpShapeViolation};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CodeSlotBindings(BTreeMap<String, u64>);

impl TryFrom<BTreeMap<String, u64>> for CodeSlotBindings {
    type Error = String;

    fn try_from(bindings: BTreeMap<String, u64>) -> Result<Self, Self::Error> {
        let mut occupied = vec![false; bindings.len()];
        for (symbol, index) in &bindings {
            if symbol.is_empty() {
                return Err("compiled code slot requires a nonempty owner symbol".into());
            }
            let slot = usize::try_from(*index)
                .ok()
                .and_then(|index| occupied.get_mut(index))
                .ok_or_else(|| {
                    format!("compiled code slot `{symbol}` index {index} is not dense")
                })?;
            if std::mem::replace(slot, true) {
                return Err(format!(
                    "compiled code slot index {index} has multiple owners"
                ));
            }
        }
        Ok(Self(bindings))
    }
}

impl<'de> Deserialize<'de> for CodeSlotBindings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(BTreeMap::<String, u64>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

impl CodeSlotBindings {
    pub fn resolve(&self, symbol: &str) -> Result<u64, String> {
        self.0
            .get(symbol)
            .copied()
            .ok_or_else(|| format!("compiled code slot `{symbol}` has no program binding"))
    }

    /// The program slot of the class statement a stamp or layout guard names
    /// (HF-220), for targets that resolve bindings at compile time.
    pub fn resolve_class_definition(&self, op: &crate::OpIR) -> Result<u64, String> {
        let symbol = op
            .class_definition
            .as_deref()
            .ok_or_else(|| format!("`{}` names no class statement", op.kind))?;
        self.resolve(symbol)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, u64)> {
        self.0
            .iter()
            .map(|(symbol, index)| (symbol.as_str(), *index))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Validate the actual program once, before pruning, batching or emission.
    /// Unused bindings remain valid after dead-function elimination.
    pub fn validate_functions(
        &self,
        functions: &[FunctionIR],
    ) -> Result<(), FunctionOpShapeDiagnostic> {
        for function in functions {
            for (op_index, op) in function.ops.iter().enumerate() {
                let unbound_class_definition = op
                    .class_definition
                    .as_deref()
                    .is_some_and(|symbol| !self.0.contains_key(symbol));
                let violation = match op.kind.as_str() {
                    _ if unbound_class_definition => Some(OpShapeViolation::UnboundClassDefinition),
                    "code_slot_set" | "trace_enter_slot"
                        if op
                            .s_value
                            .as_deref()
                            .is_none_or(|symbol| !self.0.contains_key(symbol)) =>
                    {
                        Some(OpShapeViolation::UnboundCodeSlot)
                    }
                    "code_slots_init" if op.value != i64::try_from(self.len()).ok() => {
                        Some(OpShapeViolation::CodeSlotCount {
                            expected: self.len(),
                            actual: op.value,
                        })
                    }
                    _ => None,
                };
                if let Some(violation) = violation {
                    return Err(FunctionOpShapeDiagnostic {
                        function: function.name.clone(),
                        op_index,
                        shape: OpShapeDiagnostic {
                            family: if violation == OpShapeViolation::UnboundClassDefinition {
                                "class_layout"
                            } else {
                                "code_metadata"
                            },
                            kind: op.kind.clone(),
                            violation,
                        },
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramSlotKind {
    Code,
    Module,
}

/// Collision-free, target-independent immutable binding symbols. Domains and
/// UTF-8 owners remain distinct; no hash or punctuation replacement can alias.
pub fn program_slot_symbol(kind: ProgramSlotKind, owner: &str) -> String {
    let domain = match kind {
        ProgramSlotKind::Code => "code",
        ProgramSlotKind::Module => "module",
    };
    format!(
        "__molt_{domain}_slot_{}",
        crate::content_digest::bytes_to_lower_hex(owner.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_admission_requires_unique_dense_indices() {
        for source in [
            r#"{"a":1}"#,
            r#"{"a":0,"b":0}"#,
            r#"{"":0}"#,
            r#"{"a":-1}"#,
            r#"{"a":18446744073709551615}"#,
        ] {
            assert!(serde_json::from_str::<CodeSlotBindings>(source).is_err());
        }
        let bindings: CodeSlotBindings = serde_json::from_str(r#"{"b":0,"a":1}"#).unwrap();
        assert_eq!(bindings.resolve("a"), Ok(1));
        assert_eq!(bindings.resolve("b"), Ok(0));
        assert!(bindings.resolve("absent").is_err());
        assert_eq!(
            serde_json::to_string(&bindings).unwrap(),
            r#"{"a":1,"b":0}"#
        );
    }

    #[test]
    fn program_bindings_admit_unused_owners_and_diagnose_each_missing_reference() {
        let bindings: CodeSlotBindings = serde_json::from_str(r#"{"unused":0,"owner":1}"#).unwrap();
        for kind in ["code_slot_set", "trace_enter_slot"] {
            let mut function = FunctionIR {
                name: "owner".into(),
                ops: vec![crate::OpIR {
                    kind: kind.into(),
                    s_value: Some("owner".into()),
                    args: (kind == "code_slot_set").then(|| vec!["code".into(), "globals".into()]),
                    ..Default::default()
                }],
                ..Default::default()
            };
            bindings
                .validate_functions(std::slice::from_ref(&function))
                .unwrap();
            function.ops[0].s_value = Some("absent".into());
            let error = bindings.validate_functions(&[function]).unwrap_err();
            assert_eq!(error.function, "owner");
            assert_eq!(error.op_index, 0);
            assert_eq!(error.shape.kind, kind);
            assert_eq!(error.shape.violation, OpShapeViolation::UnboundCodeSlot);
        }
        for kind in ["class_set_definition", "guard_layout"] {
            let mut function = FunctionIR {
                name: "owner".into(),
                ops: vec![crate::OpIR {
                    kind: kind.into(),
                    class_definition: Some("owner".into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            bindings
                .validate_functions(std::slice::from_ref(&function))
                .unwrap();
            function.ops[0].class_definition = Some("class:absent:P".into());
            let error = bindings.validate_functions(&[function]).unwrap_err();
            assert_eq!(error.shape.family, "class_layout");
            assert_eq!(error.shape.kind, kind);
            assert_eq!(
                error.shape.violation,
                OpShapeViolation::UnboundClassDefinition
            );
        }
        for count in [None, Some(0), Some(1), Some(2), Some(3)] {
            let functions = [FunctionIR {
                name: "molt_main".into(),
                ops: vec![crate::OpIR {
                    kind: "code_slots_init".into(),
                    value: count,
                    ..Default::default()
                }],
                ..Default::default()
            }];
            let result = bindings.validate_functions(&functions);
            if count == Some(2) {
                result.unwrap();
            } else {
                assert_eq!(
                    result.unwrap_err().shape.violation,
                    OpShapeViolation::CodeSlotCount {
                        expected: 2,
                        actual: count
                    }
                );
            }
        }
    }

    #[test]
    fn linkage_names_preserve_distinct_owner_bytes() {
        assert_ne!(
            program_slot_symbol(ProgramSlotKind::Code, "pkg"),
            program_slot_symbol(ProgramSlotKind::Module, "pkg")
        );
        assert_ne!(
            program_slot_symbol(ProgramSlotKind::Code, "a.b"),
            program_slot_symbol(ProgramSlotKind::Code, "a_b")
        );
        assert_ne!(
            program_slot_symbol(ProgramSlotKind::Code, "é"),
            program_slot_symbol(ProgramSlotKind::Code, "e")
        );
        assert_eq!(
            program_slot_symbol(ProgramSlotKind::Code, "a.b"),
            "__molt_code_slot_612e62"
        );
    }
}
