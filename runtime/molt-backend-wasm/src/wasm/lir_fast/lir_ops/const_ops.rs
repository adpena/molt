use super::super::lir_context::LirLowerCtx;
use crate::wasm::const_materialization::{WasmConstMaterializationScratch, WasmConstOpPolicy};
use crate::wasm_abi_generated::{WasmConstLirFastPolicy, WasmConstScalarValue};
use molt_codegen_abi::box_none_bits;
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::OpCode;
use wasm_encoder::{Ieee64, Instruction, ValType};

pub(super) fn emit_lir_const(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    match tir_op.opcode {
        OpCode::ConstInt => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            let val = match policy.required_tir_scalar_value(tir_op) {
                WasmConstScalarValue::Int(value) => value,
                other => panic!(
                    "generated WASM const policy produced {other:?} for {:?}",
                    tir_op.opcode
                ),
            };
            if let Some(result) = op.result_values.first() {
                match result.repr {
                    LirRepr::F64 => ctx
                        .instructions
                        .push(Instruction::F64Const(Ieee64::from(val as f64))),
                    _ => ctx.instructions.push(Instruction::I64Const(val)),
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstFloat => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            let val = match policy.required_tir_scalar_value(tir_op) {
                WasmConstScalarValue::Float(value) => value,
                other => panic!(
                    "generated WASM const policy produced {other:?} for {:?}",
                    tir_op.opcode
                ),
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::F64Const(Ieee64::from(val)));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstBool => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            let val = match policy.required_tir_scalar_value(tir_op) {
                WasmConstScalarValue::Bool(value) => value,
                other => panic!(
                    "generated WASM const policy produced {other:?} for {:?}",
                    tir_op.opcode
                ),
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::I32Const(if val { 1 } else { 0 }));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstNone => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            assert_eq!(
                policy.required_tir_scalar_value(tir_op),
                WasmConstScalarValue::NoneValue,
                "generated WASM const policy must classify ConstNone as NoneValue"
            );
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::I64Const(box_none_bits()));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            match const_policy_for_opcode(tir_op.opcode).lir_fast_policy() {
                WasmConstLirFastPolicy::Materialize => emit_const_materialization(ctx, op),
                WasmConstLirFastPolicy::Lower => {
                    panic!(
                        "generated WASM const policy requires direct LIR lowering for {:?}",
                        tir_op.opcode
                    );
                }
            }
        }
        OpCode::ConstBigInt => match const_policy_for_opcode(tir_op.opcode).lir_fast_policy() {
            WasmConstLirFastPolicy::Materialize => emit_const_materialization(ctx, op),
            WasmConstLirFastPolicy::Lower => {
                panic!(
                    "generated WASM const policy requires direct LIR lowering for {:?}",
                    tir_op.opcode
                );
            }
        },
        other => panic!("opcode {other:?} is not a WASM LIR const opcode"),
    }
}

fn const_policy_for_opcode(opcode: OpCode) -> WasmConstOpPolicy {
    WasmConstOpPolicy::for_tir_opcode(opcode)
        .unwrap_or_else(|| panic!("opcode {opcode:?} is not a WASM const policy opcode"))
}

fn assert_const_lir_fast_policy(
    opcode: OpCode,
    expected: WasmConstLirFastPolicy,
) -> WasmConstOpPolicy {
    let policy = const_policy_for_opcode(opcode);
    assert_eq!(
        policy.lir_fast_policy(),
        expected,
        "generated WASM const LIR-fast policy drifted for {opcode:?}"
    );
    policy
}

fn emit_const_materialization(ctx: &mut LirLowerCtx, op: &LirOp) {
    let policy =
        assert_const_lir_fast_policy(op.tir_op.opcode, WasmConstLirFastPolicy::Materialize);
    let result = op.result_values.first().unwrap_or_else(|| {
        panic!(
            "generated WASM const policy requires a result for {:?}",
            op.tir_op.opcode
        )
    });
    let scratch = policy.needs_literal_scratch().then(|| {
        WasmConstMaterializationScratch::new(
            ctx.alloc_scratch_local(ValType::I64),
            ctx.alloc_scratch_local(ValType::I64),
        )
    });
    let out_local = ctx.get_local(result.id);
    ctx.emit_const_materialization(policy.tir_materialization(&op.tir_op, out_local, scratch));
}
