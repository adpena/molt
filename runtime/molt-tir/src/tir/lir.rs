use std::collections::HashMap;

use super::blocks::BlockId;
use super::ops::TirOp;
use super::types::TirType;
use super::values::ValueId;

/// Canonical low-level representation classes carried through backend-facing SSA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LirRepr {
    DynBox,
    Ref64,
    I64,
    F64,
    Bool1,
}

impl LirRepr {
    /// Pick the default low-level representation for a semantic TIR type.
    ///
    /// This intentionally stays conservative in the first slice: only the
    /// machine-scalar lanes needed for hot arithmetic/control paths become
    /// unboxed by default.
    pub fn for_type(ty: &TirType) -> Self {
        match ty {
            TirType::I64 => Self::I64,
            TirType::F64 => Self::F64,
            TirType::Bool => Self::Bool1,
            _ => Self::DynBox,
        }
    }

    /// True for representations whose physical carrier is a runtime reference
    /// word rather than a semantic machine scalar.
    pub fn is_runtime_reference_word(self) -> bool {
        matches!(self, Self::DynBox | Self::Ref64)
    }
}

/// A semantic SSA value paired with its backend-facing representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LirValue {
    pub id: ValueId,
    pub ty: TirType,
    pub repr: LirRepr,
}

/// A low-level operation. In the first Task 1 slice, LIR reuses TIR op payloads
/// and makes representation explicit on results. Task 2 expands this into a
/// richer lowering surface.
#[derive(Debug, Clone)]
pub struct LirOp {
    pub tir_op: TirOp,
    pub result_values: Vec<LirValue>,
}

/// Block terminator for LIR.
#[derive(Debug, Clone)]
pub enum LirTerminator {
    Branch {
        target: BlockId,
        args: Vec<ValueId>,
    },
    CondBranch {
        cond: ValueId,
        then_block: BlockId,
        then_args: Vec<ValueId>,
        else_block: BlockId,
        else_args: Vec<ValueId>,
    },
    Switch {
        value: ValueId,
        cases: Vec<(i64, BlockId, Vec<ValueId>)>,
        default: BlockId,
        default_args: Vec<ValueId>,
    },
    /// Generator/coroutine `_poll` state-machine dispatch (LIR mirror of
    /// [`super::blocks::Terminator::StateDispatch`]).  The dispatch value is
    /// implicit (read from the frame header at codegen time), so unlike `Switch`
    /// it carries no condition `ValueId` — only the per-edge block-argument
    /// incomings.  LIR is the verification carrier here (the native codegen
    /// consumes the SimpleIR round-trip), so this variant exists primarily so
    /// generator `_poll` bodies pass `verify_lir` with correct resume-edge
    /// dominance.
    StateDispatch {
        cases: Vec<(i64, BlockId, Vec<ValueId>)>,
        default: BlockId,
        default_args: Vec<ValueId>,
    },
    Return {
        values: Vec<ValueId>,
    },
    Unreachable,
}

/// A basic block in representation-aware SSA form.
#[derive(Debug, Clone)]
pub struct LirBlock {
    pub id: BlockId,
    pub args: Vec<LirValue>,
    pub ops: Vec<LirOp>,
    pub terminator: LirTerminator,
}

/// A function in representation-aware LIR.
#[derive(Debug, Clone)]
pub struct LirFunction {
    pub name: String,
    pub param_names: Vec<String>,
    pub param_types: Vec<TirType>,
    pub return_types: Vec<TirType>,
    pub blocks: HashMap<BlockId, LirBlock>,
    pub entry_block: BlockId,
    /// Mirror of TirFunction.label_id_map: maps a TIR block-id (u32) to
    /// the label_id (i64) it carries. Required for resolving exception
    /// transfer edges encoded as `CheckException`/`TryStart` op `value`
    /// attrs into successor BlockIds during dominator/reachability
    /// analysis. `TryEnd.value` is still round-tripped as pairing metadata
    /// but is not a transfer edge.
    pub label_id_map: HashMap<u32, i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_class_default_representation_stays_boxed() {
        assert_eq!(
            LirRepr::for_type(&TirType::UserClass("Point".into())),
            LirRepr::DynBox
        );
    }

    #[test]
    fn ref64_is_reference_word_not_scalar_i64() {
        assert!(LirRepr::Ref64.is_runtime_reference_word());
        assert!(LirRepr::DynBox.is_runtime_reference_word());
        assert!(!LirRepr::I64.is_runtime_reference_word());
        assert!(!LirRepr::F64.is_runtime_reference_word());
        assert!(!LirRepr::Bool1.is_runtime_reference_word());
    }
}
