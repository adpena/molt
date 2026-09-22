//! Shared reachability helpers for TIR passes that remove blocks.

use std::collections::HashSet;

use crate::tir::blocks::BlockId;
use crate::tir::dominators;
use crate::tir::function::TirFunction;

/// Collect the blocks that must survive a block-removing pass.
///
/// This follows explicit/implicit executable edges plus non-executable label
/// custody (such as TryEnd). It seeds every structural loop key and endpoint:
/// lower_to_simple depends on those metadata-carrying blocks even when a local
/// branch fold makes part of the textual loop shape temporarily unreachable.
pub(super) fn metadata_preserving_reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    dominators::reachable_blocks_from_roots_with(
        func,
        func.structural_block_roots(),
        dominators::CfgEdgePolicy::Retention,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopBreakKind, LoopRole, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn fixture() -> TirFunction {
        let mut func = TirFunction::new(
            "retention_closure".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        for id in 0..=7 {
            func.blocks.insert(
                BlockId(id),
                TirBlock {
                    id: BlockId(id),
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }
        func.next_block = 8;
        // These endpoints have no loop-role entry and no executable incoming edge.
        func.loop_roles.insert(BlockId(1), LoopRole::LoopHeader);
        func.loop_pairs.insert(BlockId(1), BlockId(2));
        func.loop_cond_blocks.insert(BlockId(1), BlockId(3));
        func.loop_break_kinds
            .insert(BlockId(4), LoopBreakKind::BreakIfFalse);
        func.blocks.get_mut(&BlockId(3)).unwrap().ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryEnd,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::from([("value".into(), AttrValue::Int(50))]),
            source_span: None,
        });
        func.label_id_map.extend([(5, 50), (6, 60), (7, 70)]);
        func.blocks.get_mut(&BlockId(5)).unwrap().terminator = Terminator::Branch {
            target: BlockId(6),
            args: vec![],
        };
        func.blocks.get_mut(&BlockId(6)).unwrap().ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::from([("value".into(), AttrValue::Int(50))]),
            source_span: None,
        });
        func
    }

    #[test]
    fn structural_and_try_end_custody_does_not_become_executable_reachability() {
        let mut func = fixture();
        assert_eq!(
            dominators::executable_reachable_blocks(&func),
            HashSet::from([func.entry_block])
        );
        assert_eq!(
            metadata_preserving_reachable_blocks(&func),
            (0..=6).map(BlockId).collect()
        );
        // Rooting at the TryEnd owner still does not create an executable edge.
        assert_eq!(
            dominators::reachable_blocks_from_roots_with(
                &func,
                [BlockId(3)],
                dominators::CfgEdgePolicy::Full
            ),
            HashSet::from([BlockId(3)])
        );
        let retained = metadata_preserving_reachable_blocks(&func);
        assert_eq!(func.retain_blocks(&retained).unwrap(), 1);
        assert!(!func.label_id_map.contains_key(&7));
    }

    #[test]
    fn dce_and_sccp_keep_metadata_closure_and_retire_dead_projections() {
        for sccp in [false, true] {
            let mut func = fixture();
            // Force SCCP's branch-fold cleanup, while keeping all structural roots.
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstBool,
                operands: vec![],
                results: vec![ValueId(0)],
                attrs: AttrDict::from([("value".into(), AttrValue::Bool(true))]),
                source_span: None,
            });
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0),
                then_block: BlockId(6),
                then_args: vec![],
                else_block: BlockId(7),
                else_args: vec![],
            };
            func.next_value = 1;
            if sccp {
                super::super::sccp::run(&mut func);
                assert!(!func.blocks.contains_key(&BlockId(7)));
                assert!(!func.label_id_map.contains_key(&7));
            } else {
                func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
                    target: BlockId(6),
                    args: vec![],
                };
                super::super::dce::run(&mut func);
                assert!(!func.blocks.contains_key(&BlockId(7)));
                assert!(!func.label_id_map.contains_key(&7));
            }
            for id in 1..=6 {
                assert!(
                    func.blocks.contains_key(&BlockId(id)),
                    "lost retention block {id}"
                );
            }
        }
    }
}
