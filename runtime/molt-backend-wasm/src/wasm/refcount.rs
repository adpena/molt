//! WASM spelling for the shared checked RC generator.
//!
//! The generated decision is written once per module, as one internal
//! `(context: i64, word: i64, admission: i64)` body per operation; every dynamic site calls
//! its operation's body through the attached call emitter
//! (docs/design/foundation/inline_refcount.md, "WASM: one generated body per
//! transition"). The caller forwards its context, word and admission instead
//! of repeating the whole decision, which was 76-78% of compiled code.
use crate::wasm::WasmBackend;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use molt_codegen_abi::inline_refcount::{
    RefcountBuilder, RefcountOperation, RefcountStorage, generate_refcount,
};
use molt_codegen_abi::word::{WordBuilder, WordComparison};
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

/// Body parameters: the attached context, word, and invocation's admission snapshot.
const WORD_PARAM: u32 = 1;
const ADMISSION_PARAM: u32 = 2;
const WORDS: u32 = 40;
const PREDICATES: u32 = 16;
const FIRST_WORD: u32 = 3;
const FIRST_PREDICATE: u32 = FIRST_WORD + WORDS;

/// Function indices of the two generated transition bodies of one module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WasmRefcountBodies {
    retain: u32,
    release: u32,
}

impl WasmRefcountBodies {
    pub(crate) fn index(self, operation: RefcountOperation) -> u32 {
        match operation {
            RefcountOperation::Retain => self.retain,
            RefcountOperation::Release => self.release,
        }
    }
}

/// The one per-invocation admission local a function's RC sites pass by value.
#[derive(Clone, Copy, Debug)]
pub(in crate::wasm) struct WasmRefcountScratch {
    pub(in crate::wasm) admission: u32,
}

impl WasmRefcountScratch {
    pub(in crate::wasm) fn allocate(mut allocate: impl FnMut(ValType) -> u32) -> Self {
        Self {
            admission: allocate(ValType::I64),
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

/// Declare and emit the module's two transition bodies. They must precede every
/// other defined function whose index is derived from `func_count`, so callers
/// run this before the callable-table layout fixes user function indices.
pub(in crate::wasm) fn emit_refcount_bodies(backend: &mut WasmBackend, reloc: bool) {
    let type_idx = backend.types.function(&[ValType::I64; 3], &[]);
    let mut index_of = |operation| {
        let index = backend.func_count;
        std::rc::Rc::make_mut(&mut backend.execution_call_abis)
            .register_attached(index, &[ValType::I64; 2]);
        backend.funcs.function(type_idx);
        let mut locals = vec![ValType::I64; WORDS as usize];
        locals.extend(std::iter::repeat_n(ValType::I32, PREDICATES as usize));
        let mut func = Function::new_with_locals_types(locals);
        func.attach_context(3, backend.execution_call_abis.clone());
        generate_body(
            &mut BodySink {
                func: &mut func,
                reloc,
                imports: &backend.import_ids,
            },
            operation,
        );
        func.instruction(&Instruction::End);
        backend.codes.function(&func);
        backend.func_count += 1;
        index
    };
    let bodies = WasmRefcountBodies {
        retain: index_of(RefcountOperation::Retain),
        release: index_of(RefcountOperation::Release),
    };
    backend.import_ids.set_refcount_bodies(bodies);
}

/// A dynamic op-loop site: the word is on the operand stack.
pub(in crate::wasm) fn emit_refcount(
    func: &mut Function,
    reloc: bool,
    imports: &TrackedImportIds,
    scratch: WasmRefcountScratch,
    operation: RefcountOperation,
) {
    func.instruction(&Instruction::LocalGet(scratch.admission));
    emit_call(func, reloc, imports.refcount_body(operation));
}

struct BodySink<'a> {
    func: &'a mut Function,
    reloc: bool,
    imports: &'a TrackedImportIds,
}

impl BodySink<'_> {
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

fn generate_body(sink: &mut BodySink<'_>, operation: RefcountOperation) {
    // Every rejection exits the inner block to one unchanged runtime call.
    // A successful/no-op transition exits the outer block around that call.
    sink.push(Instruction::Block(BlockType::Empty));
    sink.push(Instruction::Block(BlockType::Empty));
    let mut b = WasmRefcountBuilder {
        sink,
        word_count: 0,
        predicate_count: 0,
        depth: 0,
    };
    generate_refcount(
        &mut b,
        WORD_PARAM,
        ADMISSION_PARAM,
        operation,
        RefcountStorage::for_build(false),
    );
    b.sink.push(Instruction::Br(1));
    b.sink.push(Instruction::End);
    b.sink.push(Instruction::LocalGet(WORD_PARAM));
    b.sink.slow(operation);
    b.sink.push(Instruction::End);
}

struct WasmRefcountBuilder<'s, 'a> {
    sink: &'s mut BodySink<'a>,
    word_count: u32,
    predicate_count: u32,
    depth: u32,
}

impl WasmRefcountBuilder<'_, '_> {
    fn word(&mut self) -> u32 {
        assert!(
            self.word_count < WORDS,
            "shared RC generator exhausted its word scratch"
        );
        let slot = FIRST_WORD + self.word_count;
        self.word_count += 1;
        self.sink.push(Instruction::LocalSet(slot));
        slot
    }
    fn predicate(&mut self) -> u32 {
        assert!(
            self.predicate_count < PREDICATES,
            "shared RC generator exhausted predicate scratch"
        );
        let slot = FIRST_PREDICATE + self.predicate_count;
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

impl WordBuilder for WasmRefcountBuilder<'_, '_> {
    type Word = u32;
    type Predicate = u32;
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
    fn or(&mut self, value: u32, bits: u64) -> u32 {
        self.get(value);
        self.sink.push(Instruction::I64Const(bits as i64));
        self.sink.push(Instruction::I64Or);
        self.word()
    }
    fn shift_left(&mut self, value: u32, amount: u32) -> u32 {
        self.get(value);
        self.sink.push(Instruction::I64Const(i64::from(amount)));
        self.sink.push(Instruction::I64Shl);
        self.word()
    }
    fn shift_right_arithmetic(&mut self, value: u32, amount: u32) -> u32 {
        self.get(value);
        self.sink.push(Instruction::I64Const(i64::from(amount)));
        self.sink.push(Instruction::I64ShrS);
        self.word()
    }
    fn add_immediate(&mut self, value: u32, delta: i64) -> u32 {
        self.get(value);
        self.sink.push(Instruction::I64Const(delta));
        self.sink.push(Instruction::I64Add);
        self.word()
    }
    fn compare(&mut self, comparison: WordComparison, left: u32, right: u32) -> u32 {
        self.get(left);
        self.get(right);
        self.sink.push(match comparison {
            WordComparison::Equal => Instruction::I64Eq,
            WordComparison::NotEqual => Instruction::I64Ne,
            WordComparison::UnsignedLessThan => Instruction::I64LtU,
        });
        self.predicate()
    }
    fn both(&mut self, left: u32, right: u32) -> u32 {
        self.get(left);
        self.get(right);
        self.sink.push(Instruction::I32And);
        self.predicate()
    }
}

impl RefcountBuilder for WasmRefcountBuilder<'_, '_> {
    fn pointer_width(&self) -> molt_codegen_abi::PointerWidth {
        molt_codegen_abi::PointerWidth::Bits32
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
        assert_eq!(bits, WORD_PARAM);
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
