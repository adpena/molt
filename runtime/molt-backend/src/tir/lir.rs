use std::collections::HashMap;

use super::blocks::BlockId;
use super::ops::TirOp;
use super::types::TirType;
use super::values::ValueId;

/// Canonical low-level representation classes carried through backend-facing SSA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LirRepr {
    DynBox,
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
}
