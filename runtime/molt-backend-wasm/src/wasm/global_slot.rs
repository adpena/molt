//! WASM instruction selection for the shared module global slot read
//! (call_anatomy.md 4.5; `molt_codegen_abi::global_slot` owns the meaning).
//!
//! As the RC transitions do, the read is one generated body per slot index
//! a module reads, `(context, base, module, name, admission) -> value`; each
//! site pushes its operands and calls the body for its index.
use std::collections::{BTreeMap, BTreeSet};

use crate::wasm::WasmBackend;
use crate::wasm::refcount::{BodySink, WasmRefcountBuilder};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::{Function, emit_call};
use crate::{OpIR, SimpleIR};
use molt_codegen_abi::global_slot::{SlotBuilder, generate_global_slot_read};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::BranchBuilder;
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

const BASE_PARAM: u32 = 1;
const MODULE_PARAM: u32 = 2;
const NAME_PARAM: u32 = 3;
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
