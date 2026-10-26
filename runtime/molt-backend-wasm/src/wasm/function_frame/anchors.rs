//! Invocation-owned literal anchors in one accounted scratch allocation.
//!
//! This changes physical storage, not lifetime: every slot owns one reference
//! until reverse exit cleanup, independently of each constant result owner.

use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::emit_boxed_none;
use wasm_encoder::{BlockType, Function, Instruction, MemArg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) struct WasmConstAnchorSlot(usize);

impl WasmConstAnchorSlot {
    pub(in crate::wasm) fn new(index: usize) -> Self {
        Self(index)
    }
}

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmConstAnchorRegion {
    base: u32,
    temporary: u32,
    count: usize,
    bytes: i64,
}

impl WasmConstAnchorRegion {
    pub(in crate::wasm) fn new(base: u32, temporary: u32, count: usize) -> Self {
        assert!(count != 0, "empty anchors require no storage");
        let bytes = count
            .checked_mul(8)
            .and_then(|bytes| i64::try_from(bytes).ok())
            .expect("literal anchor region exceeds the sized scratch ABI");
        Self {
            base,
            temporary,
            count,
            bytes,
        }
    }

    pub(in crate::wasm) fn base(self) -> u32 {
        self.base
    }
    pub(in crate::wasm) fn temporary(self) -> u32 {
        self.temporary
    }

    fn slot_memarg(self, slot: WasmConstAnchorSlot) -> MemArg {
        assert!(
            slot.0 < self.count,
            "literal anchor slot outside its region"
        );
        MemArg {
            offset: (slot.0 as u64) * 8,
            ..word_memarg()
        }
    }

    pub(in crate::wasm) fn emit_allocate(
        self,
        func: &mut Function,
        imports: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        func.instruction(&Instruction::I64Const(self.bytes));
        emit_call(
            func,
            reloc_enabled,
            imports[WasmRuntimeImport::ScratchAlloc],
        );
        func.instruction(&Instruction::LocalSet(self.base));
    }

    /// The caller branches to its existing return scope on a null allocation.
    /// Initialize every hole before any fallible constructor can run. Zero is
    /// not boxed None, and memory.fill cannot express its repeated word value.
    pub(in crate::wasm) fn emit_initialize(self, func: &mut Function) {
        self.emit_reverse_loop_start(func);
        self.emit_cursor_address(func);
        emit_boxed_none(func);
        func.instruction(&Instruction::I64Store(word_memarg()));
        self.emit_reverse_loop_end(func);
    }

    pub(in crate::wasm) fn emit_store_temporary(
        self,
        func: &mut Function,
        slot: WasmConstAnchorSlot,
    ) {
        func.instruction(&Instruction::LocalGet(self.base));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(self.temporary));
        func.instruction(&Instruction::I64Store(self.slot_memarg(slot)));
    }

    pub(in crate::wasm) fn emit_load(self, func: &mut Function, slot: WasmConstAnchorSlot) {
        func.instruction(&Instruction::LocalGet(self.base));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::I64Load(self.slot_memarg(slot)));
    }

    pub(in crate::wasm) fn emit_release(
        self,
        func: &mut Function,
        imports: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        func.instruction(&Instruction::LocalGet(self.base));
        func.instruction(&Instruction::I64Eqz);
        func.instruction(&Instruction::If(BlockType::Empty));
        // No allocation, or already retired by normal cleanup before a later
        // frame-exit unwind: neither case may read memory through zero.
        func.instruction(&Instruction::Else);
        self.emit_reverse_loop_start(func);
        self.emit_cursor_address(func);
        func.instruction(&Instruction::I64Load(word_memarg()));
        // Keep the old owner on the operand stack while publishing None.
        // Reentry sees a private region; unwind cleanup sees cleared holes.
        self.emit_cursor_address(func);
        emit_boxed_none(func);
        func.instruction(&Instruction::I64Store(word_memarg()));
        emit_call(func, reloc_enabled, imports[WasmRuntimeImport::DecRefObj]);
        self.emit_reverse_loop_end(func);
        // ScratchFree has no Python callback. Transfer allocation custody
        // before freeing; a later TraceExit unwind must not free it again.
        func.instruction(&Instruction::LocalGet(self.base));
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::LocalSet(self.base));
        func.instruction(&Instruction::I64Const(self.bytes));
        emit_call(func, reloc_enabled, imports[WasmRuntimeImport::ScratchFree]);
        func.instruction(&Instruction::End);
    }

    fn emit_reverse_loop_start(self, func: &mut Function) {
        func.instruction(&Instruction::I64Const(self.bytes / 8));
        func.instruction(&Instruction::LocalSet(self.temporary));
        func.instruction(&Instruction::Block(BlockType::Empty));
        func.instruction(&Instruction::Loop(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(self.temporary));
        func.instruction(&Instruction::I64Eqz);
        func.instruction(&Instruction::BrIf(1));
        func.instruction(&Instruction::LocalGet(self.temporary));
        func.instruction(&Instruction::I64Const(1));
        func.instruction(&Instruction::I64Sub);
        func.instruction(&Instruction::LocalSet(self.temporary));
    }

    fn emit_cursor_address(self, func: &mut Function) {
        func.instruction(&Instruction::LocalGet(self.base));
        func.instruction(&Instruction::LocalGet(self.temporary));
        func.instruction(&Instruction::I64Const(3));
        func.instruction(&Instruction::I64Shl);
        func.instruction(&Instruction::I64Add);
        func.instruction(&Instruction::I32WrapI64);
    }

    fn emit_reverse_loop_end(self, func: &mut Function) {
        func.instruction(&Instruction::Br(0));
        func.instruction(&Instruction::End);
        func.instruction(&Instruction::End);
    }
}

fn word_memarg() -> MemArg {
    MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }
}
