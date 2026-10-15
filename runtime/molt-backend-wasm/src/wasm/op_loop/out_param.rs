//! A runtime entry's second result through a linear-memory out-parameter.
//!
//! WASM imports return at most one value, so an entry with a second result
//! (a method load's `self_or_null`, an unboxed iterator step's value) takes
//! the address of a scratch word and writes it as its last action, after any
//! Python code it ran. The caller loads the word right after the call returns.
//! A shared scratch region is therefore safe: nested compiled code may reuse
//! it during the call, but the entry's final store follows every such use.

use wasm_encoder::{Function, Instruction, MemArg};

/// The scratch word an entry writes its out-parameter to.
#[derive(Clone, Copy, Debug)]
pub(super) struct WasmOutParam {
    address: u32,
}

impl WasmOutParam {
    pub(super) fn at(address: u32) -> Self {
        Self { address }
    }

    /// Push the word's address as the entry's `u64` out-pointer argument.
    pub(super) fn push_address(self, func: &mut Function) {
        func.instruction(&Instruction::I64Const(i64::from(self.address)));
    }

    /// Push the word the entry wrote.
    pub(super) fn load(self, func: &mut Function) {
        func.instruction(&Instruction::I32Const(self.address as i32));
        func.instruction(&Instruction::I64Load(MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
    }
}
