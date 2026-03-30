use super::ops::TirOp;
use super::values::{TirValue, ValueId};

/// Unique identifier for a basic block within a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// Structural loop role for a basic block, used to preserve loop markers
/// across the TIR roundtrip so downstream backends (Cranelift, WASM) can
/// reconstruct structured loop constructs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopRole {
    /// Not part of loop boundary structure.
    None,
    /// This block is a loop header introduced by `loop_start`.
    LoopHeader,
    /// This block is a loop-end boundary (`loop_end`).
    LoopEnd,
}

/// Preserved polarity of the original structured loop exit op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopBreakKind {
    BreakIfTrue,
    BreakIfFalse,
}

/// A basic block in SSA form with block arguments (MLIR-style, no phi nodes).
#[derive(Debug, Clone)]
pub struct TirBlock {
    pub id: BlockId,
    /// Block arguments — these replace phi nodes. Predecessor branches
    /// pass values that bind to these arguments on entry.
    pub args: Vec<TirValue>,
    /// Operations in execution order.
    pub ops: Vec<TirOp>,
    /// Block terminator (exactly one per block).
    pub terminator: Terminator,
}

/// Block terminator — controls transfer at the end of a basic block.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Unconditional branch to a target block, passing arguments.
    Branch { target: BlockId, args: Vec<ValueId> },
    /// Conditional branch: if `cond` is truthy, go to `then_block`, else `else_block`.
    CondBranch {
        cond: ValueId,
        then_block: BlockId,
        then_args: Vec<ValueId>,
        else_block: BlockId,
        else_args: Vec<ValueId>,
    },
    /// Multi-way switch on an integer value.
    Switch {
        value: ValueId,
        /// (case_value, target_block, args)
        cases: Vec<(i64, BlockId, Vec<ValueId>)>,
        default: BlockId,
        default_args: Vec<ValueId>,
    },
    /// Return from the function with zero or more values.
    Return { values: Vec<ValueId> },
    /// Marks unreachable code (e.g. after a guaranteed raise).
    Unreachable,
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    #[test]
    fn block_construction_with_args_and_ops() {
        let block = TirBlock {
            id: BlockId(0),
            args: vec![
                TirValue {
                    id: ValueId(0),
                    ty: TirType::I64,
                },
                TirValue {
                    id: ValueId(1),
                    ty: TirType::Bool,
                },
            ],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![ValueId(0), ValueId(1)],
                results: vec![ValueId(2)],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ValueId(2)],
            },
        };

        assert_eq!(block.id, BlockId(0));
        assert_eq!(block.args.len(), 2);
        assert_eq!(block.ops.len(), 1);
        assert!(matches!(block.terminator, Terminator::Return { .. }));
    }

    #[test]
    fn block_with_branch_terminator() {
        let block = TirBlock {
            id: BlockId(0),
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: BlockId(1),
                args: vec![ValueId(0)],
            },
        };

        if let Terminator::Branch { target, args } = &block.terminator {
            assert_eq!(*target, BlockId(1));
            assert_eq!(args.len(), 1);
        } else {
            panic!("expected Branch terminator");
        }
    }

    #[test]
    fn block_with_cond_branch() {
        let block = TirBlock {
            id: BlockId(0),
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: BlockId(1),
                then_args: vec![],
                else_block: BlockId(2),
                else_args: vec![],
            },
        };

        assert!(matches!(block.terminator, Terminator::CondBranch { .. }));
    }
}
