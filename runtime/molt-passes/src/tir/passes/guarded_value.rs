//! Keep the original continuation and its result when selecting a guarded operation.
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::TirOp;
use crate::tir::types::TirType;
use crate::tir::values::TirValue;

pub(super) fn move_loop_terminator(
    func: &mut TirFunction,
    block: crate::tir::blocks::BlockId,
    observe: crate::tir::blocks::BlockId,
) {
    use crate::tir::blocks::LoopRole;
    for boundary in func
        .loop_cond_blocks
        .values_mut()
        .chain(func.loop_pairs.values_mut())
    {
        if *boundary == block {
            *boundary = observe;
        }
    }
    if func.loop_roles.get(&block) == Some(&LoopRole::LoopEnd) {
        func.loop_roles.remove(&block);
        func.loop_roles.insert(observe, LoopRole::LoopEnd);
    }
}

/// Each predicate's false edge executes the original operation. Both arms
/// transfer one result to the pre-existing continuation.
pub(super) fn select_guarded_result<const N: usize>(
    func: &mut TirFunction,
    block: BlockId,
    index: usize,
    guards: [TirOp; N],
    generic: TirOp,
    ty: TirType,
) -> BlockId {
    let yes = func.fresh_block();
    let no = func.fresh_block();
    let join = func.fresh_block();
    // Build predicates backwards. Every failure takes the same generic arm.
    let mut success = yes;
    let mut predicates = guards.into_iter().enumerate().rev();
    let source_terminator = loop {
        let (index, guard) = predicates
            .next()
            .expect("a guarded operation needs a predicate");
        let cond = guard.results[0];
        func.value_types.insert(cond, TirType::Bool);
        let branch = Terminator::CondBranch {
            cond,
            then_block: success,
            then_args: vec![],
            else_block: no,
            else_args: vec![],
        };
        if index == 0 {
            break (guard, branch);
        }
        let at = func.fresh_block();
        func.blocks.insert(
            at,
            TirBlock {
                id: at,
                args: vec![],
                ops: vec![guard],
                terminator: branch,
            },
        );
        success = at;
    };
    let source = func
        .blocks
        .get_mut(&block)
        .expect("guarded operation block");
    let tail = source.ops.split_off(index + 1);
    let direct = source.ops.pop().expect("guarded operation");
    let result = direct.results[0];
    let (first, branch) = source_terminator;
    source.ops.push(first);
    let terminator = std::mem::replace(&mut source.terminator, branch);
    // The result of the original call names the join, never the true arm.
    for (id, mut call) in [(yes, direct), (no, generic)] {
        let returned = func.fresh_value();
        call.results = vec![returned];
        func.value_types.insert(returned, ty.clone());
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![call],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![returned],
                },
            },
        );
    }
    func.value_types.insert(result, ty.clone());
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue { id: result, ty }],
            ops: tail,
            terminator,
        },
    );
    move_loop_terminator(func, block, join);
    yes
}
