//! WASM instruction selection for the shared module global slot read
//! (call_anatomy.md 4.5; `molt_codegen_abi::global_slot` owns the meaning).
//!
//! As the RC transitions do, the read is one generated body per slot index
//! a module reads, `(context, base, module, name, admission) -> value`; each
//! site pushes its operands and calls the body for its index.
//!
//! A cell-eligible module's static cells (call_anatomy.md 4.5a;
//! `molt_codegen_abi::static_cell`) are one mutable data segment per module in
//! linear memory, as method site records are. `static_cell_base` is the
//! segment's address boxed as a slot base; a store is one generated body per
//! cell index, `(context, base, value, unused, admission)`, so its admission
//! sits where a read body's does.
use std::collections::{BTreeMap, BTreeSet};

use crate::wasm::WasmBackend;
use crate::wasm::refcount::{BodySink, WasmRefcountBuilder};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::{Function, emit_call};
use crate::{OpIR, SimpleIR};
use molt_codegen_abi::global_slot::GLOBAL_SLOT_BASE_SHIFT;
use molt_codegen_abi::global_slot::{SlotBuilder, generate_global_slot_read};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::static_cell::{CellBuilder, generate_static_cell_store};
use molt_codegen_abi::word::BranchBuilder;
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

const BASE_PARAM: u32 = 1;
const MODULE_PARAM: u32 = 2;
const NAME_PARAM: u32 = 3;
/// A store body's value: the read body's module position.
const VALUE_PARAM: u32 = 2;
const ADMISSION_PARAM: u32 = 4;
const FIRST_WORD: u32 = 5;

/// The slot indices the module's `global_slot_get` sites read.
pub(in crate::wasm) fn read_indices(ir: &SimpleIR) -> BTreeSet<u32> {
    ir.functions
        .iter()
        .flat_map(|function| function.ops.iter())
        .filter(|op| op.kind == molt_ir::OpKind::GlobalSlotGet)
        .map(slot_index)
        .collect()
}

/// The cell indices the module's `static_cell_set` sites store.
pub(in crate::wasm) fn store_indices(ir: &SimpleIR) -> BTreeSet<u32> {
    ir.functions
        .iter()
        .flat_map(|function| function.ops.iter())
        .filter(|op| op.kind == molt_ir::OpKind::StaticCellSet)
        .map(slot_index)
        .collect()
}

/// One mutable segment per cell-eligible module, every word unbound.
pub(in crate::wasm) fn prepare_static_cell_blocks(backend: &mut WasmBackend, reloc: bool) {
    let blocks: Vec<(String, u32)> = backend
        .module_registry
        .iter()
        .flat_map(|registry| registry.static_cell_blocks())
        .map(|(module, count)| (module.to_string(), count))
        .collect();
    for (module, count) in blocks {
        let bytes = molt_codegen_abi::NAMESPACE_SLOT_UNBOUND
            .to_le_bytes()
            .repeat(count as usize);
        let block = backend.add_data_segment_mutable(reloc, &bytes);
        backend.static_cell_blocks.insert(module, block);
    }
}

/// Declare and emit one store body per cell index a module stores.
pub(in crate::wasm) fn emit_static_cell_store_bodies(
    backend: &mut WasmBackend,
    reloc: bool,
    indices: &BTreeSet<u32>,
) {
    let type_idx = backend.types.function(&[ValType::I64; 5], &[]);
    let mut bodies = BTreeMap::new();
    for &index in indices {
        let function_index = backend.func_count;
        std::rc::Rc::make_mut(&mut backend.execution_call_abis)
            .register_attached(function_index, &[ValType::I64; 4]);
        backend.funcs.function(type_idx);
        let mut func = Function::new_with_locals_types(WasmRefcountBuilder::scratch_locals());
        func.attach_context(FIRST_WORD as usize, backend.execution_call_abis.clone());
        let mut sink = BodySink {
            func: &mut func,
            reloc,
            imports: &backend.import_ids,
        };
        let mut b = WasmRefcountBuilder::new(&mut sink, FIRST_WORD);
        // linear memory has one writer per isolate: WASM is never free-threaded.
        generate_static_cell_store(&mut b, BASE_PARAM, index, VALUE_PARAM, false);
        func.instruction(&Instruction::End);
        backend.codes.function(&func);
        backend.func_count += 1;
        bodies.insert(index, function_index);
    }
    backend.import_ids.set_static_cell_store_bodies(bodies);
}

/// A `static_cell_base` op: the module's segment address as a slot base.
pub(in crate::wasm) fn emit_static_cell_base(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &crate::wasm::WasmFrameLocals,
    func_index: u32,
    reloc: bool,
) {
    let module = op
        .s_value
        .as_deref()
        .expect("admitted static_cell_base owner");
    let block = *backend
        .static_cell_blocks
        .get(module)
        .unwrap_or_else(|| panic!("module {module} has no static cell block"));
    backend.emit_data_ptr(reloc, func_index, func, block);
    func.instruction(&Instruction::I64Const(i64::from(GLOBAL_SLOT_BASE_SHIFT)));
    func.instruction(&Instruction::I64ShrU);
    func.instruction(&Instruction::I64Const(molt_codegen_abi::INT_MASK as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const(
        (molt_codegen_abi::QNAN | molt_codegen_abi::TAG_INT) as i64,
    ));
    func.instruction(&Instruction::I64Or);
    let out = locals
        .bound_op_result_slot(op)
        .expect("admitted static_cell_base result");
    func.instruction(&Instruction::LocalSet(out));
}

/// A `static_cell_set` site: [base, value] are locals.
pub(in crate::wasm) fn emit_static_cell_set(
    func: &mut Function,
    op: &OpIR,
    locals: &crate::wasm::WasmFrameLocals,
    imports: &crate::wasm_import_tracking::TrackedImportIds,
    reloc: bool,
) {
    let args = op.args.as_ref().expect("admitted static_cell_set operands");
    for name in args {
        func.instruction(&Instruction::LocalGet(locals[name]));
    }
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::LocalGet(locals.refcount().admission));
    emit_call(func, reloc, imports.static_cell_store_body(slot_index(op)));
}

fn slot_index(op: &OpIR) -> u32 {
    op.global_slot
        .as_ref()
        .expect("admitted global_slot_get carries its slot")
        .index
}

/// Declare and emit one read body per index, after the RC bodies they call
/// and before the callable-table layout fixes user function indices.
pub(in crate::wasm) fn emit_global_slot_bodies(
    backend: &mut WasmBackend,
    reloc: bool,
    indices: &BTreeSet<u32>,
) {
    let type_idx = backend.types.function(&[ValType::I64; 5], &[ValType::I64]);
    let mut bodies = BTreeMap::new();
    for &index in indices {
        let function_index = backend.func_count;
        std::rc::Rc::make_mut(&mut backend.execution_call_abis)
            .register_attached(function_index, &[ValType::I64; 4]);
        backend.funcs.function(type_idx);
        let mut func = Function::new_with_locals_types(WasmRefcountBuilder::scratch_locals());
        func.attach_context(FIRST_WORD as usize, backend.execution_call_abis.clone());
        let mut sink = BodySink {
            func: &mut func,
            reloc,
            imports: &backend.import_ids,
        };
        let mut b = WasmRefcountBuilder::new(&mut sink, FIRST_WORD);
        let value = generate_global_slot_read(&mut b, BASE_PARAM, index, |b| {
            b.get(MODULE_PARAM);
            b.get(NAME_PARAM);
            let import = b.sink.imports[WasmRuntimeImport::ModuleGetGlobal];
            emit_call(b.sink.func, b.sink.reloc, import);
            b.word()
        });
        b.get(value);
        func.instruction(&Instruction::End);
        backend.codes.function(&func);
        backend.func_count += 1;
        bodies.insert(index, function_index);
    }
    backend.import_ids.set_global_slot_bodies(bodies);
}

/// A `global_slot_get` site: [base, module, name] are locals; the owned
/// value lands in the op's result local.
pub(in crate::wasm) fn emit_global_slot_get(
    func: &mut Function,
    op: &OpIR,
    locals: &crate::wasm::WasmFrameLocals,
    imports: &crate::wasm_import_tracking::TrackedImportIds,
    reloc: bool,
) {
    let args = op.args.as_ref().expect("admitted global_slot_get operands");
    for name in args {
        func.instruction(&Instruction::LocalGet(locals[name]));
    }
    func.instruction(&Instruction::LocalGet(locals.refcount().admission));
    emit_call(func, reloc, imports.global_slot_body(slot_index(op)));
    let out = locals
        .bound_op_result_slot(op)
        .expect("admitted global_slot_get result");
    func.instruction(&Instruction::LocalSet(out));
}

impl BranchBuilder for WasmRefcountBuilder<'_, '_> {
    fn choose(
        &mut self,
        condition: u32,
        yes: impl FnOnce(&mut Self) -> u32,
        no: impl FnOnce(&mut Self) -> u32,
    ) -> u32 {
        self.get(condition);
        self.sink
            .push(Instruction::If(BlockType::Result(ValType::I64)));
        let taken = yes(self);
        self.get(taken);
        self.sink.push(Instruction::Else);
        let fallback = no(self);
        self.get(fallback);
        self.sink.push(Instruction::End);
        self.word()
    }
}

impl CellBuilder for WasmRefcountBuilder<'_, '_> {
    fn swap_word(&mut self, address: u32, offset: i32, value: u32, atomic: bool) -> u32 {
        assert!(!atomic, "WASM has no free-threaded runtime");
        let memory = MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        };
        self.address(address, offset);
        self.sink.push(Instruction::I64Load(memory));
        let old = self.word();
        self.address(address, offset);
        self.get(value);
        self.sink.push(Instruction::I64Store(memory));
        old
    }

    fn release(&mut self, word: u32) {
        self.get(word);
        self.get(ADMISSION_PARAM);
        let body = self.sink.imports.refcount_body(RefcountOperation::Release);
        emit_call(self.sink.func, self.sink.reloc, body);
    }
}

impl SlotBuilder for WasmRefcountBuilder<'_, '_> {
    fn load_word(&mut self, address: u32, offset: i32) -> u32 {
        self.address(address, offset);
        self.sink.push(Instruction::I64Load(MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
        self.word()
    }

    fn retain(&mut self, word: u32) {
        self.get(word);
        self.get(ADMISSION_PARAM);
        let body = self.sink.imports.refcount_body(RefcountOperation::Retain);
        emit_call(self.sink.func, self.sink.reloc, body);
    }
}
