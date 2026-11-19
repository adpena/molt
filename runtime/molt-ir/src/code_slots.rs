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
                let violation = match op.kind {
                    _ if unbound_class_definition => Some(OpShapeViolation::UnboundClassDefinition),
                    crate::OpKind::CodeSlotSet | crate::OpKind::TraceEnterSlot
                        if op
                            .s_value
                            .as_deref()
                            .is_none_or(|symbol| !self.0.contains_key(symbol)) =>
                    {
                        Some(OpShapeViolation::UnboundCodeSlot)
                    }
                    crate::OpKind::CodeSlotsInit if op.value != i64::try_from(self.len()).ok() => {
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
                            kind: op.kind.to_string(),
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

/// The data symbol holding a stateful poll's frame byte size: one
/// target-native `i64` that the object compiling the poll defines, after the
/// coroutine frame split (`molt-passes` `frame_split`) placed every value live
/// across a suspension past the frontend's layout. Task constructors load it,
/// so no constructor needs the poll's body and separately compiled objects
/// agree through the linker. The owner is hex-encoded like a program slot's.
pub fn stateful_frame_size_symbol(poll: &str) -> String {
    format!(
        "__molt_frame_size_{}",
        crate::content_digest::bytes_to_lower_hex(poll.as_bytes())
    )
}

/// Where a task constructor reads the byte size of the frame it allocates.
/// One answer for every backend: the choice is the poll's kind, never where
/// its body is compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFrameSize {
    /// A runtime-native poll: its frame is the frontend layout, a constant.
    Layout,
    /// A compiled poll: its frame-size word ([`stateful_frame_size_symbol`]).
    /// The frame split adds spill slots past the layout, and the object that
    /// compiles the poll defines the word, whichever batch or object that is.
    CompiledWord,
}

impl TaskFrameSize {
    pub fn of_poll(poll: &str) -> Self {
        if crate::runtime_fn_generated::RuntimeFn::from_symbol(poll)
            .and_then(crate::runtime_fn_generated::RuntimeFn::poll_key)
            .is_some()
        {
            Self::Layout
        } else {
            Self::CompiledWord
        }
    }
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

/// The writable data symbol of a cell-eligible module's static cell block
/// (call_anatomy.md 4.5a): one word per layout name, initially unbound,
/// defined once in the program object and imported by every function object.
pub fn static_cell_block_symbol(module: &str) -> String {
    format!(
        "__molt_static_cells_{}",
        crate::content_digest::bytes_to_lower_hex(module.as_bytes())
    )
}

/// The writable data symbol of a literal's cell (call_anatomy.md 4.9): one
/// word per (kind, payload), private to the object that evaluates it and zero
/// until the literal's first evaluation fills it. `kind` is the cell's
/// `molt_codegen_abi::literal_cell::LiteralKind` word; the payload names the
/// cell by digest, so an object holds one cell per distinct literal.
pub fn literal_cell_symbol(kind: u64, payload: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(payload);
    format!(
        "__molt_literal_{kind}_{}",
        crate::content_digest::bytes_to_lower_hex(&digest[..16])
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
        for kind in [crate::OpKind::CodeSlotSet, crate::OpKind::TraceEnterSlot] {
            let mut function = FunctionIR {
                name: "owner".into(),
                ops: vec![crate::OpIR {
                    kind,
                    s_value: Some("owner".into()),
                    args: (kind == crate::OpKind::CodeSlotSet)
                        .then(|| vec!["code".into(), "globals".into()]),
                    ..crate::OpIR::new(kind)
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
            assert_eq!(error.shape.kind, kind.as_str());
            assert_eq!(error.shape.violation, OpShapeViolation::UnboundCodeSlot);
        }
        for kind in [
            crate::OpKind::ClassSetDefinition,
            crate::OpKind::GuardLayout,
        ] {
            let mut function = FunctionIR {
                name: "owner".into(),
                ops: vec![crate::OpIR {
                    kind,
                    class_definition: Some("owner".into()),
                    ..crate::OpIR::new(kind)
                }],
                ..Default::default()
            };
            bindings
                .validate_functions(std::slice::from_ref(&function))
                .unwrap();
            function.ops[0].class_definition = Some("class:absent:P".into());
            let error = bindings.validate_functions(&[function]).unwrap_err();
            assert_eq!(error.shape.family, "class_layout");
            assert_eq!(error.shape.kind, kind.as_str());
            assert_eq!(
                error.shape.violation,
                OpShapeViolation::UnboundClassDefinition
            );
        }
        for count in [None, Some(0), Some(1), Some(2), Some(3)] {
            let functions = [FunctionIR {
                name: "molt_main".into(),
                ops: vec![crate::OpIR {
                    kind: crate::OpKind::CodeSlotsInit,
                    value: count,
                    ..crate::OpIR::new(crate::OpKind::CodeSlotsInit)
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

    #[test]
    fn a_task_frame_size_follows_the_polls_kind() {
        use crate::runtime_fn_generated::RuntimeFn;
        // A runtime poll's frame is its layout.
        assert_eq!(
            TaskFrameSize::of_poll(RuntimeFn::AsyncSleepPoll.symbol()),
            TaskFrameSize::Layout
        );
        // A compiled poll's frame is its frame-size word, whether or not the
        // constructor's own object compiles the poll (2026-11-18: Cranelift
        // took the layout for a poll compiled in another batch, so the frame
        // split's spill slots lay past the allocation).
        assert_eq!(
            TaskFrameSize::of_poll("py.app.count_poll"),
            TaskFrameSize::CompiledWord
        );
        // A runtime entry that is not a poll is no runtime poll.
        assert_eq!(
            TaskFrameSize::of_poll(RuntimeFn::Add.symbol()),
            TaskFrameSize::CompiledWord
        );
    }
}
