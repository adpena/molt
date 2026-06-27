use crate::wasm::body::{WasmBodyOp, WasmBodyOps};
use wasm_encoder::Instruction;

// ---------------------------------------------------------------------------
// Peephole: local.set X; local.get X → local.tee X
// ---------------------------------------------------------------------------
//
// The SSA→stack-machine lowering emits an explicit local.set after every op
// result and a local.get before every operand read. This creates abundant
// `local.set X; local.get X` pairs where the value is stored AND immediately
// reloaded. WASM's `local.tee` instruction does both in one shot: it stores
// the value in the local AND leaves a copy on the stack, eliminating the
// redundant get.
//
// This is a single linear pass over the instruction vector: O(N) time, O(N)
// space (new vec). No control-flow analysis needed because the pattern is
// purely sequential and the semantics are identical.
//
// Additionally, when the tee'd value is never read again after the
// immediately following instruction, the entire set can sometimes be
// eliminated — but that requires liveness analysis beyond this peephole's
// scope. wasm-opt handles that downstream.

pub(super) fn peephole_set_get_to_tee(instructions: WasmBodyOps) -> WasmBodyOps {
    let instructions = instructions.into_vec();
    if instructions.len() < 2 {
        return WasmBodyOps { ops: instructions };
    }
    let mut out = WasmBodyOps::default();
    let mut i = 0;
    while i < instructions.len() {
        // Pattern 1: local.set X; local.get X -> local.tee X
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::LocalSet(set_idx)),
                WasmBodyOp::Instruction(Instruction::LocalGet(get_idx)),
            ) = (&instructions[i], &instructions[i + 1])
            && set_idx == get_idx
        {
            out.push(Instruction::LocalTee(*set_idx));
            i += 2;
            continue;
        }
        // Pattern 2: i64.const 0; i64.eq -> i64.eqz (test for zero)
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::I64Const(0)),
                WasmBodyOp::Instruction(Instruction::I64Eq),
            ) = (&instructions[i], &instructions[i + 1])
        {
            out.push(Instruction::I64Eqz);
            i += 2;
            continue;
        }
        // Pattern 3: i32.const 0; i32.eq -> i32.eqz
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::I32Const(0)),
                WasmBodyOp::Instruction(Instruction::I32Eq),
            ) = (&instructions[i], &instructions[i + 1])
        {
            out.push(Instruction::I32Eqz);
            i += 2;
            continue;
        }
        // Pattern 4: i64.const 1; i64.mul -> (eliminated, multiply by 1 is identity)
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::I64Const(1)),
                WasmBodyOp::Instruction(Instruction::I64Mul),
            ) = (&instructions[i], &instructions[i + 1])
        {
            // Value already on stack; skip the const+mul.
            i += 2;
            continue;
        }
        // Pattern 5: i64.const 0; i64.add -> (eliminated, add 0 is identity)
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::I64Const(0)),
                WasmBodyOp::Instruction(Instruction::I64Add),
            ) = (&instructions[i], &instructions[i + 1])
        {
            i += 2;
            continue;
        }
        // Pattern 6: i64.const 0; i64.sub -> (eliminated, sub 0 is identity)
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::I64Const(0)),
                WasmBodyOp::Instruction(Instruction::I64Sub),
            ) = (&instructions[i], &instructions[i + 1])
        {
            i += 2;
            continue;
        }
        // Pattern 7: i64.const -1; i64.xor -> (equivalent to bit_not, but keep xor)
        // Not folded: -1 xor is the canonical bit_not, no simpler form exists.

        // Pattern 8: f64.const 0.0; f64.add -> (eliminated, add 0 is identity)
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::F64Const(z)),
                WasmBodyOp::Instruction(Instruction::F64Add),
            ) = (&instructions[i], &instructions[i + 1])
            && f64::from(*z) == 0.0
        {
            i += 2;
            continue;
        }
        // Pattern 9: f64.const 1.0; f64.mul -> (eliminated, multiply by 1 is identity)
        if i + 1 < instructions.len()
            && let (
                WasmBodyOp::Instruction(Instruction::F64Const(one)),
                WasmBodyOp::Instruction(Instruction::F64Mul),
            ) = (&instructions[i], &instructions[i + 1])
            && f64::from(*one) == 1.0
        {
            i += 2;
            continue;
        }
        match &instructions[i] {
            WasmBodyOp::Instruction(instruction) => out.push(instruction.clone()),
            WasmBodyOp::Call(call) => out.ops.push(WasmBodyOp::Call(*call)),
        }
        i += 1;
    }
    out
}
