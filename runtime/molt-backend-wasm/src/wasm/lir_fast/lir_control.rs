use super::lir_context::LirLowerCtx;
use super::lir_scalar::{emit_box_none, emit_lir_truthiness_i32, emit_return_boxed_i64};
use molt_tir::tir::blocks::BlockId;
#[cfg(any(test, feature = "test-util"))]
use molt_tir::tir::lir::LirRepr;
use molt_tir::tir::lir::LirTerminator;
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Instruction};

#[derive(Clone, Copy)]
pub(super) enum LirReturnAbi {
    #[cfg(any(test, feature = "test-util"))]
    Native(Option<LirRepr>),
    BoxedI64,
}

/// Labels represent semantic destinations, not block indices or guessed depths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LirControlLabel {
    ForwardExit(BlockId),
    LoopContinue(BlockId),
    Selection,
}

pub(super) fn emit_lir_terminator(
    ctx: &mut LirLowerCtx,
    term: &LirTerminator,
    return_abi: LirReturnAbi,
    labels: &mut Vec<LirControlLabel>,
) {
    match term {
        LirTerminator::Return { values } => emit_lir_return(ctx, values, return_abi),
        LirTerminator::Unreachable => ctx.instructions.push(Instruction::Unreachable),
        LirTerminator::Branch { target, args } => emit_edge(ctx, *target, args, labels),
        LirTerminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            emit_lir_truthiness_i32(ctx, *cond);
            ctx.instructions.push(Instruction::If(BlockType::Empty));
            labels.push(LirControlLabel::Selection);
            emit_edge(ctx, *then_block, then_args, labels);
            ctx.instructions.push(Instruction::Else);
            emit_edge(ctx, *else_block, else_args, labels);
            ctx.instructions.push(Instruction::End);
            labels.pop();
        }
        LirTerminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            for (case_value, target, args) in cases {
                ctx.emit_get(*value);
                ctx.instructions.push(Instruction::I64Const(*case_value));
                ctx.instructions.push(Instruction::I64Eq);
                ctx.instructions.push(Instruction::If(BlockType::Empty));
                labels.push(LirControlLabel::Selection);
                emit_edge(ctx, *target, args, labels);
                ctx.instructions.push(Instruction::End);
                labels.pop();
            }
            emit_edge(ctx, *default, default_args, labels);
        }
        LirTerminator::StateDispatch { .. } => {
            panic!(
                "StateDispatch terminator reached the LIR→WASM fast lane in '{}'; \
                 generator/coroutine _poll bodies must lower via the SimpleIR relooper",
                ctx.func.name
            );
        }
    }
}

fn emit_lir_return(ctx: &mut LirLowerCtx, values: &[ValueId], return_abi: LirReturnAbi) {
    match return_abi {
        #[cfg(any(test, feature = "test-util"))]
        LirReturnAbi::Native(result_repr) => {
            if let Some(&value) = values.first() {
                ctx.emit_get(value);
            } else if let Some(result_repr) = result_repr {
                // The test-only native ABI uses raw carriers for scalars;
                // boxed/reference carriers retain the semantic None tag.
                match result_repr {
                    LirRepr::DynBox | LirRepr::Ref64 => emit_box_none(ctx),
                    LirRepr::I64 => ctx.instructions.push(Instruction::I64Const(0)),
                    LirRepr::F64 => ctx.instructions.push(Instruction::F64Const(0.0.into())),
                    LirRepr::Bool1 => ctx.instructions.push(Instruction::I32Const(0)),
                }
            }
        }
        LirReturnAbi::BoxedI64 => {
            if let Some(&value) = values.first() {
                emit_return_boxed_i64(ctx, value);
            } else {
                emit_box_none(ctx);
            }
        }
    }
    ctx.instructions.push(Instruction::Return);
}

fn emit_edge(ctx: &mut LirLowerCtx, target: BlockId, args: &[ValueId], labels: &[LirControlLabel]) {
    let depth = labels
        .iter()
        .rev()
        .position(|label| {
            matches!(label,
        LirControlLabel::ForwardExit(id) | LirControlLabel::LoopContinue(id) if *id == target)
        })
        .unwrap_or_else(|| {
            panic!(
                "LIR WASM CFG in '{}': no active scope for target {target:?}",
                ctx.func.name
            )
        });
    store_lir_block_args(ctx, target, args);
    ctx.instructions.push(Instruction::Br(depth as u32));
}

fn store_lir_block_args(ctx: &mut LirLowerCtx, target: BlockId, args: &[ValueId]) {
    let block = ctx.func.blocks.get(&target).expect("validated edge target");
    assert_eq!(args.len(), block.args.len(), "validated edge arity changed");
    let destinations: Vec<_> = block
        .args
        .iter()
        .map(|value| (value.id, value.repr))
        .collect();
    // Snapshot all sources before writing any destination. The WASM operand
    // stack is already a typed parallel-copy temporary; no RC or boxing occurs.
    for (&source, &(_, repr)) in args.iter().zip(&destinations) {
        assert_eq!(
            ctx.value_reprs[&source], repr,
            "validated edge representation changed"
        );
        ctx.emit_get(source);
    }
    for &(destination, _) in destinations.iter().rev() {
        ctx.emit_set(destination);
    }
}
