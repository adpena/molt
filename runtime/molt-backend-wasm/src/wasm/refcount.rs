//! WASM instruction spelling for the shared checked RC generator.
use crate::wasm::body::WasmBodyOps;
use crate::wasm::lir_fast::LirRuntimeCall;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use molt_codegen_abi::inline_refcount::{
    RefcountBuilder, RefcountComparison, RefcountOperation, RefcountStorage, generate_refcount,
};
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

const WORDS: u32 = 40;
const PREDICATES: u32 = 16;

#[derive(Clone, Copy, Debug)]
pub(in crate::wasm) struct WasmRefcountScratch {
    pub(in crate::wasm) admission: u32,
    words: u32,
    predicates: u32,
}

impl WasmRefcountScratch {
    pub(in crate::wasm) fn allocate(mut allocate: impl FnMut(ValType) -> u32) -> Self {
        let admission = allocate(ValType::I64);
        let words = allocate(ValType::I64);
        for index in 1..WORDS {
            assert_eq!(allocate(ValType::I64), words + index);
        }
        let predicates = allocate(ValType::I32);
        for index in 1..PREDICATES {
            assert_eq!(allocate(ValType::I32), predicates + index);
        }
        Self {
            admission,
            words,
            predicates,
        }
    }

    pub(in crate::wasm) fn initialize(
        self,
        func: &mut Function,
        imports: &TrackedImportIds,
        reloc: bool,
    ) {
        emit_call(func, reloc, imports[WasmRuntimeImport::RefcountAdmission]);
        func.instruction(&Instruction::LocalSet(self.admission));
    }
}

pub(in crate::wasm) fn emit_refcount(
    func: &mut Function,
    reloc: bool,
    imports: &TrackedImportIds,
    scratch: WasmRefcountScratch,
    operation: RefcountOperation,
) {
    generate(
        &mut FunctionSink {
            func,
            reloc,
            imports,
        },
        scratch,
        operation,
    );
}

pub(in crate::wasm) fn emit_lir_refcount(
    body: &mut WasmBodyOps,
    scratch: WasmRefcountScratch,
    operation: RefcountOperation,
) {
    generate(body, scratch, operation);
}

trait Sink {
    fn push(&mut self, instruction: Instruction<'static>);
    fn slow(&mut self, operation: RefcountOperation);
}

struct FunctionSink<'a> {
    func: &'a mut Function,
    reloc: bool,
    imports: &'a TrackedImportIds,
}

impl Sink for FunctionSink<'_> {
    fn push(&mut self, instruction: Instruction<'static>) {
        self.func.instruction(&instruction);
    }
    fn slow(&mut self, operation: RefcountOperation) {
        let import = match operation {
            RefcountOperation::Retain => WasmRuntimeImport::IncRefObj,
            RefcountOperation::Release => WasmRuntimeImport::DecRefObj,
        };
        emit_call(self.func, self.reloc, self.imports[import]);
    }
}

impl Sink for WasmBodyOps {
    fn push(&mut self, instruction: Instruction<'static>) {
        WasmBodyOps::push(self, instruction);
    }
    fn slow(&mut self, operation: RefcountOperation) {
        self.push_runtime_import_call(match operation {
            RefcountOperation::Retain => LirRuntimeCall::IncRefObj,
            RefcountOperation::Release => LirRuntimeCall::DecRefObj,
        });
    }
}

fn generate(sink: &mut impl Sink, scratch: WasmRefcountScratch, operation: RefcountOperation) {
    sink.push(Instruction::LocalSet(scratch.words));
    // Every rejection exits the inner block to one unchanged runtime call.
    // A successful/no-op transition exits the outer block around that call.
    sink.push(Instruction::Block(BlockType::Empty));
    sink.push(Instruction::Block(BlockType::Empty));
    let mut b = WasmRefcountBuilder {
        sink,
        scratch,
        word_count: 1,
        predicate_count: 0,
        depth: 0,
    };
    generate_refcount(
        &mut b,
        scratch.words,
        scratch.admission,
        operation,
        RefcountStorage::for_build(false),
    );
    b.sink.push(Instruction::Br(1));
    b.sink.push(Instruction::End);
    b.sink.push(Instruction::LocalGet(scratch.words));
    b.sink.slow(operation);
    b.sink.push(Instruction::End);
}

struct WasmRefcountBuilder<'a, S> {
    sink: &'a mut S,
    scratch: WasmRefcountScratch,
    word_count: u32,
    predicate_count: u32,
    depth: u32,
}

impl<S: Sink> WasmRefcountBuilder<'_, S> {
    fn word(&mut self) -> u32 {
        assert!(
            self.word_count < WORDS,
            "shared RC generator exhausted its word scratch"
        );
        let slot = self.scratch.words + self.word_count;
        self.word_count += 1;
        self.sink.push(Instruction::LocalSet(slot));
        slot
    }
    fn predicate(&mut self) -> u32 {
        assert!(
            self.predicate_count < PREDICATES,
            "shared RC generator exhausted predicate scratch"
        );
        let slot = self.scratch.predicates + self.predicate_count;
        self.predicate_count += 1;
        self.sink.push(Instruction::LocalSet(slot));
        slot
    }
    fn get(&mut self, slot: u32) {
        self.sink.push(Instruction::LocalGet(slot));
    }
    fn address(&mut self, pointer: u32, offset: i32) {
        self.get(pointer);
        self.sink.push(Instruction::I64Const(offset as i64));
        self.sink.push(Instruction::I64Add);
        self.sink.push(Instruction::I32WrapI64);
    }
}

impl<S: Sink> RefcountBuilder for WasmRefcountBuilder<'_, S> {
    type Word = u32;
    type Predicate = u32;
    fn pointer_width(&self) -> molt_codegen_abi::PointerWidth {
        molt_codegen_abi::PointerWidth::Bits32
    }
    fn constant(&mut self, value: u64) -> u32 {
        self.sink.push(Instruction::I64Const(value as i64));
        self.word()
    }
    fn and(&mut self, value: u32, mask: u64) -> u32 {
        self.get(value);
        self.sink.push(Instruction::I64Const(mask as i64));
        self.sink.push(Instruction::I64And);
        self.word()
    }
    fn add(&mut self, value: u32, delta: i64) -> u32 {
        self.get(value);
        self.sink.push(Instruction::I64Const(delta));
        self.sink.push(Instruction::I64Add);
        self.word()
    }
    fn compare(&mut self, comparison: RefcountComparison, left: u32, right: u32) -> u32 {
        self.get(left);
        self.get(right);
        self.sink.push(match comparison {
            RefcountComparison::Equal => Instruction::I64Eq,
            RefcountComparison::NotEqual => Instruction::I64Ne,
            RefcountComparison::UnsignedLessThan => Instruction::I64LtU,
        });
        self.predicate()
    }
    fn both(&mut self, left: u32, right: u32) -> u32 {
        self.get(left);
        self.get(right);
        self.sink.push(Instruction::I32And);
        self.predicate()
    }
    fn load_header_u32(&mut self, pointer: u32, offset: i32) -> u32 {
        self.address(pointer, offset);
        self.sink.push(Instruction::I64Load32U(MemArg {
            align: 2,
            offset: 0,
            memory_index: 0,
        }));
        self.word()
    }
    fn store_header_u32(&mut self, pointer: u32, offset: i32, value: u32) {
        self.address(pointer, offset);
        self.get(value);
        self.sink.push(Instruction::I32WrapI64);
        self.sink.push(Instruction::I32Store(MemArg {
            align: 2,
            offset: 0,
            memory_index: 0,
        }));
    }
    fn slow_call(&mut self, _operation: RefcountOperation, bits: u32) {
        assert_eq!(bits, self.scratch.words);
        self.sink.push(Instruction::Br(self.depth));
    }
    fn if_else(&mut self, condition: u32, yes: impl FnOnce(&mut Self), no: impl FnOnce(&mut Self)) {
        self.get(condition);
        self.sink.push(Instruction::If(BlockType::Empty));
        self.depth += 1;
        yes(self);
        self.sink.push(Instruction::Else);
        no(self);
        self.sink.push(Instruction::End);
        self.depth -= 1;
    }
}
