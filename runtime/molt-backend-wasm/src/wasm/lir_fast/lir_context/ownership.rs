//! Operation-local owners for physical materializations absent from SSA.
//!
//! A borrowed raw integer may need a heap box at a runtime ABI boundary. That
//! box is neither the raw SSA value nor the runtime call's result. Keep its
//! owner until the entire operation finishes, including lazy boolean arms and
//! aggregate construction. Failure skips dependent work, releases initialized
//! owners, and leaves exception transfer to the enclosing IR edge.

use super::LirLowerCtx;
use crate::wasm::body::WasmBodyOp;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_codegen_abi::box_none_bits;
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::values::ValueId;
use std::collections::HashMap;
use wasm_encoder::{BlockType, Ieee64, Instruction, ValType};

pub(super) struct LirOperationOwners {
    start: usize,
    locals: Vec<u32>,
    boxes: HashMap<ValueId, u32>,
    failure_branches: Vec<usize>,
}

impl LirLowerCtx<'_> {
    pub(in crate::wasm::lir_fast) fn begin_operation_owners(&mut self) {
        assert!(self.operation_owners.is_none(), "nested LIR owner scope");
        self.operation_owners = Some(LirOperationOwners {
            start: self.instructions.ops.len(),
            locals: Vec::new(),
            boxes: HashMap::new(),
            failure_branches: Vec::new(),
        });
    }

    pub(in crate::wasm::lir_fast) fn alloc_operation_owner(&mut self) -> u32 {
        let local = self.alloc_scratch_local(ValType::I64);
        self.operation_owners
            .as_mut()
            .expect("temporary heap owner outside LIR operation")
            .locals
            .push(local);
        local
    }

    pub(in crate::wasm::lir_fast) fn boxed_operand_local(&mut self, value: ValueId) -> u32 {
        if let Some(&local) = self.operation_owners.as_ref().unwrap().boxes.get(&value) {
            return local;
        }
        let local = self.alloc_operation_owner();
        self.operation_owners
            .as_mut()
            .unwrap()
            .boxes
            .insert(value, local);
        local
    }

    /// The owner has been consumed or transferred; cleanup must not drop it.
    pub(in crate::wasm::lir_fast) fn forget_operation_owner(&mut self, local: u32) {
        self.instructions
            .push(Instruction::I64Const(box_none_bits()));
        self.instructions.push(Instruction::LocalSet(local));
    }

    /// Consume an i32 failure predicate. Branch depths are resolved in one
    /// linear pass once the operation's nested control structure is complete.
    pub(in crate::wasm::lir_fast) fn branch_to_operation_cleanup_if(&mut self) {
        let at = self.instructions.ops.len();
        self.operation_owners
            .as_mut()
            .expect("allocation failure outside LIR operation")
            .failure_branches
            .push(at);
        self.instructions.push(Instruction::BrIf(0));
    }

    pub(in crate::wasm::lir_fast) fn guard_operation_exception(&mut self) {
        self.emit_runtime_call(LirRuntimeCall::ExceptionPending);
        self.instructions.push(Instruction::I64Const(0));
        self.instructions.push(Instruction::I64Ne);
        self.branch_to_operation_cleanup_if();
    }

    pub(in crate::wasm::lir_fast) fn finish_operation_owners(&mut self, op: &LirOp) {
        let owners = self
            .operation_owners
            .take()
            .expect("missing LIR owner scope");
        if owners.locals.is_empty() && owners.failure_branches.is_empty() {
            return;
        }
        let mut body = self.instructions.ops.split_off(owners.start);
        // Reset on each dynamic execution, not just function entry. A skipped
        // arm must never expose a stale owner from a previous loop iteration.
        for &local in &owners.locals {
            self.forget_operation_owner(local);
        }
        for result in &op.result_values {
            let zero = match result.repr {
                LirRepr::DynBox | LirRepr::Ref64 => Instruction::I64Const(box_none_bits()),
                LirRepr::I64 => Instruction::I64Const(0),
                LirRepr::Bool1 => Instruction::I32Const(0),
                LirRepr::F64 => Instruction::F64Const(Ieee64::from(0.0)),
            };
            self.instructions.push(zero);
            self.emit_set(result.id);
        }
        let mut branches = owners.failure_branches.into_iter().peekable();
        let mut depth = 0u32;
        for (index, instruction) in body.iter_mut().enumerate() {
            if branches.peek() == Some(&(owners.start + index)) {
                *instruction = WasmBodyOp::Instruction(Instruction::BrIf(depth));
                branches.next();
            }
            match instruction {
                WasmBodyOp::Instruction(
                    Instruction::Block(_) | Instruction::Loop(_) | Instruction::If(_),
                ) => depth += 1,
                WasmBodyOp::Instruction(Instruction::End) => {
                    depth = depth.checked_sub(1).expect("unbalanced LIR operation")
                }
                _ => {}
            }
        }
        assert_eq!(depth, 0, "unclosed LIR operation control structure");
        assert!(branches.next().is_none(), "unresolved LIR failure branch");
        self.instructions.push(Instruction::Block(BlockType::Empty));
        self.instructions.ops.extend(body);
        self.instructions.push(Instruction::End);
        for local in owners.locals.into_iter().rev() {
            self.instructions.push(Instruction::LocalGet(local));
            self.emit_runtime_call(LirRuntimeCall::DecRefObj);
        }
    }
}
