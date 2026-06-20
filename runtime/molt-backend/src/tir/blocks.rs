use super::ops::TirOp;
use super::values::{TirValue, ValueId};

/// Unique identifier for a basic block within a function.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub struct BlockId(pub u32);

/// Structural loop role for a basic block, used to preserve loop markers
/// across the TIR roundtrip so downstream backends (Cranelift, WASM) can
/// reconstruct structured loop constructs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum LoopRole {
    /// Not part of loop boundary structure.
    None,
    /// This block is a loop header introduced by `loop_start`.
    LoopHeader,
    /// This block is a loop-end boundary (`loop_end`).
    LoopEnd,
}

/// Preserved polarity of the original structured loop exit op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum LoopBreakKind {
    BreakIfTrue,
    BreakIfFalse,
}

/// A basic block in SSA form with block arguments (MLIR-style, no phi nodes).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
    /// Generator/coroutine `_poll` state-machine dispatch.
    ///
    /// On entry the `_poll` function reads the saved resume state (via
    /// `molt_obj_get_state(self)`) and dispatches: state 0 (initial entry) takes
    /// the `default` edge (the function's first-entry continuation); every other
    /// saved state takes the matching `cases` edge to the resume continuation of
    /// the suspend op that established that state.
    ///
    /// This is the first-class form of the `state_switch` op.  Unlike `Switch`,
    /// the dispatch value is *implicit* (read from the frame header at lowering
    /// time, not an SSA `ValueId`), because the suspend op `ret`s and the saved
    /// state is restored by the runtime across the suspend boundary — there is no
    /// SSA value live across the `ret` to switch on.  The case/default `args` are
    /// the block-argument incomings supplied on each dispatch edge (the values
    /// live at the dispatch point), placed by the SSA pass exactly like any other
    /// terminator's branch args so phi placement, dominator updates, and
    /// block-renumbering passes handle them for free.
    StateDispatch {
        /// (resume_state_id, resume_block, args)
        cases: Vec<(i64, BlockId, Vec<ValueId>)>,
        /// State 0 (initial entry) target.
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
