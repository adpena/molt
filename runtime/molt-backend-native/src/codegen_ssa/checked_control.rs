//! A checked producer can emit its continuations directly when the flag is
//! consumed by the terminator and the successful edge owns every raw use.

use molt_ir::deterministic_hash::HashMap;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{compute_dominators_in_order, exception_transfer_target};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

#[derive(Clone, Copy)]
pub(super) struct CheckedBranch {
    pub(super) hit: BlockId,
    pub(super) miss: BlockId,
    pub(super) raw: Option<ValueId>,
}

#[derive(Clone, Copy)]
enum UseScope {
    BranchFlag,
    Success(BlockId),
}

impl CheckedBranch {
    /// Index just the checked producers that can consume an existing pair of
    /// edges. A single use census validates them after rewrites and includes
    /// operands on exceptional edges.
    pub(super) fn packed<'cfg>(
        func: &TirFunction,
        order: &[BlockId],
        exception_targets: &HashMap<i64, BlockId>,
        predecessors: impl FnOnce() -> &'cfg HashMap<BlockId, Vec<BlockId>>,
    ) -> HashMap<BlockId, Self> {
        let mut candidates: HashMap<_, _> = order
            .iter()
            .filter_map(|&source| {
                let block = &func.blocks[&source];
                let op = block.ops.last()?;
                let Terminator::CondBranch {
                    cond,
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                } = &block.terminator
                else {
                    return None;
                };
                let (&flag, values) = op.results.split_last()?;
                let raw = if op.opcode == OpCode::TryBuffer2dLoadI64
                    && let [value] = values
                {
                    Some(*value)
                } else if op.opcode == OpCode::TryBuffer2dStoreI64 && values.is_empty() {
                    None
                } else {
                    return None;
                };
                (flag == *cond
                    && then_args.is_empty()
                    && else_args.is_empty()
                    && then_block != else_block
                    && *then_block != func.entry_block
                    && [then_block, else_block].into_iter().all(|target| {
                        func.blocks
                            .get(target)
                            .is_some_and(|block| block.args.is_empty())
                    }))
                .then_some((
                    source,
                    (
                        Self {
                            hit: *then_block,
                            miss: *else_block,
                            raw,
                        },
                        flag,
                    ),
                ))
            })
            .collect();
        if candidates.is_empty() {
            return HashMap::default();
        }

        let predecessors = predecessors();
        // The predecessor map deduplicates edges. A transfer from an earlier
        // exception observer in the same block would bypass the load as well.
        candidates.retain(|source, (branch, _)| {
            predecessors[&branch.hit].as_slice() == [*source]
                && func.blocks[source]
                    .ops
                    .iter()
                    .all(|op| exception_transfer_target(op, exception_targets) != Some(branch.hit))
        });
        if candidates.is_empty() {
            return HashMap::default();
        }

        // Stores and loads consumed wholly in the hit block need no tree.
        let dominators =
            std::cell::LazyCell::new(|| compute_dominators_in_order(order.to_vec(), predecessors));
        let uses: HashMap<ValueId, _> = candidates
            .iter()
            .flat_map(|(&source, &(branch, flag))| {
                std::iter::once((flag, (source, UseScope::BranchFlag))).chain(
                    branch
                        .raw
                        .map(|raw| (raw, (source, UseScope::Success(branch.hit)))),
                )
            })
            .collect();
        for &reader in order {
            let block = &func.blocks[&reader];
            let mut check = |value, terminal| {
                if let Some((source, scope)) = uses.get(&value) {
                    let allowed = match *scope {
                        UseScope::BranchFlag => terminal && *source == reader,
                        UseScope::Success(hit) => {
                            hit == reader || dominators.dominates(hit, reader)
                        }
                    };
                    if !allowed {
                        candidates.remove(source);
                    }
                }
            };
            for op in &block.ops {
                for &value in &op.operands {
                    check(value, false);
                }
            }
            block.terminator.for_each_value(|value| check(value, true));
        }
        candidates
            .into_iter()
            .map(|(source, (branch, _))| (source, branch))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::dominators::{
        build_pred_map, exception_label_to_block, executable_reverse_postorder,
    };
    use crate::tir::ops::{AttrValue, TirOp};
    use crate::tir::types::TirType;

    #[test]
    fn packed_branch_requires_an_exclusive_flag_and_successful_raw_uses() {
        let mut func = TirFunction::new(
            "checked".into(),
            vec![TirType::DynBox; 3],
            TirType::I64,
            molt_ir::ir::FunctionReturnAbi::Value,
        );
        let source = func.entry_block;
        let hit = func.fresh_block();
        let miss = func.fresh_block();
        let raw = func.fresh_value();
        let flag = func.fresh_value();
        let entry = func.blocks.get_mut(&source).unwrap();
        let inputs = entry.args.iter().map(|a| a.id).collect();
        entry.ops.push(TirOp::new(
            OpCode::TryBuffer2dLoadI64,
            inputs,
            vec![raw, flag],
        ));
        entry.terminator = Terminator::CondBranch {
            cond: flag,
            then_block: hit,
            then_args: vec![],
            else_block: miss,
            else_args: vec![],
        };
        func.blocks.insert(
            hit,
            TirBlock {
                id: hit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![raw] },
            },
        );
        func.blocks.insert(
            miss,
            TirBlock {
                id: miss,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );

        let admitted = |func: &TirFunction| {
            let predecessors = std::cell::LazyCell::new(|| build_pred_map(func));
            CheckedBranch::packed(
                func,
                &executable_reverse_postorder(func),
                &exception_label_to_block(func),
                || &predecessors,
            )
            .contains_key(&source)
        };
        assert!(admitted(&func));

        for value in [raw, flag] {
            func.blocks.get_mut(&miss).unwrap().terminator = Terminator::Return {
                values: vec![value],
            };
            assert!(!admitted(&func));
        }
        func.blocks.get_mut(&miss).unwrap().terminator = Terminator::Unreachable;
        let mut bypass = TirOp::new(OpCode::CheckException, vec![], vec![]);
        bypass.attrs.insert("value".into(), AttrValue::Int(42));
        func.label_id_map.insert(hit.0, 42);
        func.blocks.get_mut(&source).unwrap().ops.insert(0, bypass);
        assert!(!admitted(&func));
    }
}
