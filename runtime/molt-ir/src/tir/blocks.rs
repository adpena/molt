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

impl Terminator {
    /// Visit every explicit CFG edge and the values forwarded to its target.
    ///
    /// This is the canonical structural projection of a terminator into CFG
    /// edges. Keeping the variant-to-edge mapping here prevents passes and
    /// backends from growing partial `match` classifiers when a terminator is
    /// added. The visitor is monomorphized and allocation-free.
    #[inline]
    pub fn for_each_edge(&self, mut visit: impl FnMut(BlockId, &[ValueId])) {
        match self {
            Terminator::Branch { target, args } => visit(*target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                visit(*then_block, then_args);
                visit(*else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                for (_, target, args) in cases {
                    visit(*target, args);
                }
                visit(*default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

    /// Mutably visit every explicit CFG edge and its forwarded values.
    ///
    /// Retargeting and block-argument rewrites must use this projection so
    /// `Switch` and `StateDispatch` cannot silently diverge.
    #[inline]
    pub fn for_each_edge_mut(&mut self, mut visit: impl FnMut(&mut BlockId, &mut Vec<ValueId>)) {
        match self {
            Terminator::Branch { target, args } => visit(target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                visit(then_block, then_args);
                visit(else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                for (_, target, args) in cases {
                    visit(target, args);
                }
                visit(default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

    /// Append explicit successor blocks in stable edge order without an
    /// intermediate allocation.
    #[inline]
    pub fn append_successors(&self, out: &mut Vec<BlockId>) {
        self.for_each_edge(|target, _| out.push(target));
    }

    /// Number of explicit CFG edges carried by this terminator.
    #[inline]
    pub fn successor_count(&self) -> usize {
        match self {
            Terminator::Branch { .. } => 1,
            Terminator::CondBranch { .. } => 2,
            Terminator::Switch { cases, .. } | Terminator::StateDispatch { cases, .. } => {
                cases.len() + 1
            }
            Terminator::Return { .. } | Terminator::Unreachable => 0,
        }
    }

    /// Collect explicit successor blocks in stable edge order.
    #[inline]
    pub fn successors(&self) -> Vec<BlockId> {
        let mut successors = Vec::with_capacity(self.successor_count());
        self.append_successors(&mut successors);
        successors
    }

    /// Visit SSA values used directly by the terminator, excluding values
    /// forwarded as successor block arguments.
    #[inline]
    pub fn for_each_direct_value(&self, mut visit: impl FnMut(ValueId)) {
        match self {
            Terminator::Branch { .. } | Terminator::StateDispatch { .. } => {}
            Terminator::CondBranch { cond, .. } => visit(*cond),
            Terminator::Switch { value, .. } => visit(*value),
            Terminator::Return { values } => values.iter().copied().for_each(visit),
            Terminator::Unreachable => {}
        }
    }

    /// Visit every SSA value used by the terminator, including edge arguments.
    #[inline]
    pub fn for_each_value(&self, mut visit: impl FnMut(ValueId)) {
        self.for_each_direct_value(&mut visit);
        self.for_each_edge(|_, args| args.iter().copied().for_each(&mut visit));
    }

    /// Mutably visit every SSA value used by the terminator.
    #[inline]
    pub fn for_each_value_mut(&mut self, mut visit: impl FnMut(&mut ValueId)) {
        match self {
            Terminator::Branch { .. } | Terminator::StateDispatch { .. } => {}
            Terminator::CondBranch { cond, .. } => visit(cond),
            Terminator::Switch { value, .. } => visit(value),
            Terminator::Return { values } => values.iter_mut().for_each(&mut visit),
            Terminator::Unreachable => {}
        }
        self.for_each_edge_mut(|_, args| args.iter_mut().for_each(&mut visit));
    }

    /// Return whether any explicit CFG edge targets `target`.
    #[inline]
    pub fn has_successor(&self, target: BlockId) -> bool {
        match self {
            Terminator::Branch {
                target: successor, ..
            } => *successor == target,
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => *then_block == target || *else_block == target,
            Terminator::Switch { cases, default, .. }
            | Terminator::StateDispatch { cases, default, .. } => {
                cases.iter().any(|(_, successor, _)| *successor == target) || *default == target
            }
            Terminator::Return { .. } | Terminator::Unreachable => false,
        }
    }

    /// Return the values forwarded by the first edge to `target`.
    ///
    /// Callers that need to process every duplicate-target edge must use
    /// [`Self::for_each_edge`] instead.
    #[inline]
    pub fn first_edge_args_to(&self, target: BlockId) -> Option<&[ValueId]> {
        match self {
            Terminator::Branch {
                target: edge_target,
                args,
            } => (*edge_target == target).then_some(args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == target {
                    Some(then_args)
                } else if *else_block == target {
                    Some(else_args)
                } else {
                    None
                }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => cases
                .iter()
                .find_map(|(_, edge_target, args)| {
                    (*edge_target == target).then_some(args.as_slice())
                })
                .or_else(|| (*default == target).then_some(default_args.as_slice())),
            Terminator::Return { .. } | Terminator::Unreachable => None,
        }
    }
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

    #[test]
    fn terminator_edge_projection_covers_every_variant() {
        let variants = [
            (
                Terminator::Branch {
                    target: BlockId(1),
                    args: vec![ValueId(10)],
                },
                vec![BlockId(1)],
            ),
            (
                Terminator::CondBranch {
                    cond: ValueId(11),
                    then_block: BlockId(2),
                    then_args: vec![ValueId(12)],
                    else_block: BlockId(3),
                    else_args: vec![ValueId(13)],
                },
                vec![BlockId(2), BlockId(3)],
            ),
            (
                Terminator::Switch {
                    value: ValueId(14),
                    cases: vec![(0, BlockId(4), vec![ValueId(15)])],
                    default: BlockId(5),
                    default_args: vec![ValueId(16)],
                },
                vec![BlockId(4), BlockId(5)],
            ),
            (
                Terminator::StateDispatch {
                    cases: vec![(1, BlockId(6), vec![ValueId(17)])],
                    default: BlockId(7),
                    default_args: vec![ValueId(18)],
                },
                vec![BlockId(6), BlockId(7)],
            ),
            (
                Terminator::Return {
                    values: vec![ValueId(19)],
                },
                vec![],
            ),
            (Terminator::Unreachable, vec![]),
        ];

        for (terminator, expected) in variants {
            assert_eq!(terminator.successors(), expected);
            assert_eq!(terminator.successor_count(), expected.len());
            for target in expected {
                assert!(terminator.has_successor(target));
                assert!(terminator.first_edge_args_to(target).is_some());
            }
        }
    }

    #[test]
    fn terminator_value_and_mutable_edge_visitors_cover_every_variant() {
        let mut state_dispatch = Terminator::StateDispatch {
            cases: vec![(1, BlockId(4), vec![ValueId(20)])],
            default: BlockId(5),
            default_args: vec![ValueId(21)],
        };
        state_dispatch.for_each_edge_mut(|target, args| {
            target.0 += 10;
            args.push(ValueId(22));
        });
        assert_eq!(state_dispatch.successors(), vec![BlockId(14), BlockId(15)]);

        let variants = [
            Terminator::Branch {
                target: BlockId(1),
                args: vec![ValueId(1)],
            },
            Terminator::CondBranch {
                cond: ValueId(2),
                then_block: BlockId(2),
                then_args: vec![ValueId(3)],
                else_block: BlockId(3),
                else_args: vec![ValueId(4)],
            },
            Terminator::Switch {
                value: ValueId(5),
                cases: vec![(0, BlockId(4), vec![ValueId(6)])],
                default: BlockId(5),
                default_args: vec![ValueId(7)],
            },
            state_dispatch,
            Terminator::Return {
                values: vec![ValueId(8)],
            },
            Terminator::Unreachable,
        ];
        let expected_direct = [
            vec![],
            vec![ValueId(2)],
            vec![ValueId(5)],
            vec![],
            vec![ValueId(8)],
            vec![],
        ];

        for (terminator, expected) in variants.iter().zip(expected_direct) {
            let mut direct = Vec::new();
            terminator.for_each_direct_value(|value| direct.push(value));
            assert_eq!(direct, expected);
        }
    }
}
