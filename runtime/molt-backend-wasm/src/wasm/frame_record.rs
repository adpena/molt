//! WASM spelling for the shared frame-record generators
//! (`molt_codegen_abi::frame_record`).
//!
//! As with the RC transitions (`refcount.rs`), the generated decision is
//! written once per module: one internal push body, one pop body per clear
//! order and one for runtime-linked frames. A synchronous function's entry and exit call them through the
//! attached call emitter and pass their own home count, so no function
//! repeats the sequence and none crosses the import boundary on the fast
//! path. The bodies call the runtime only where the generator's slow paths
//! do.
use crate::wasm::WasmBackend;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use molt_codegen_abi::frame_record::{
    FrameBuilder, FrameShape, HomeCount, generate_frame_pop, generate_frame_push,
    generate_linked_frame_pop,
};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::{WordBuilder, WordComparison};
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

/// Body parameters after the attached context: the push takes the code slot,
/// the pop the record base; both take the home count and the invocation's RC
/// admission snapshot.
const SUBJECT_PARAM: u32 = 1;
const HOMES_PARAM: u32 = 2;
const ADMISSION_PARAM: u32 = 3;
const PHYSICAL_PARAMS: u32 = 4;
const WORDS: u32 = 96;
const PREDICATES: u32 = 24;
const FIRST_WORD: u32 = PHYSICAL_PARAMS;

/// Function indices of one module's generated frame bodies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WasmFrameBodies {
    push: u32,
    pop_ascending: u32,
    pop_descending: u32,
    pop_linked: u32,
}

impl WasmFrameBodies {
    pub(crate) fn push(self) -> u32 {
        self.push
    }

    /// The pop body of a frame: a synchronous frame's by its clear order,
    /// otherwise the runtime-linked frame's.
    pub(crate) fn pop(self, shape: Option<FrameShape>) -> u32 {
        match shape {
            Some(shape) if shape.descending => self.pop_descending,
            Some(_) => self.pop_ascending,
            None => self.pop_linked,
        }
    }
}

enum Body {
    Push,
    Pop { descending: bool },
    PopLinked,
}

/// Declare and emit the module's frame bodies. They call the RC transition
/// bodies, so they follow them, and precede every defined function whose
/// index is derived from `func_count`.
pub(in crate::wasm) fn emit_frame_bodies(backend: &mut WasmBackend, reloc: bool) {
    let logical = [ValType::I64; 3];
    let physical = [ValType::I64; PHYSICAL_PARAMS as usize];
    let push_type = backend.types.function(&physical, &[ValType::I64]);
    let pop_type = backend.types.function(&physical, &[]);
    let mut index_of = |body: Body| {
        let index = backend.func_count;
        std::rc::Rc::make_mut(&mut backend.execution_call_abis).register_attached(index, &logical);
        backend.funcs.function(match body {
            Body::Push => push_type,
            Body::Pop { .. } | Body::PopLinked => pop_type,
        });
        let mut func = Function::new_with_locals_types(WasmBodyLayout::scratch_locals());
        func.attach_context(
            PHYSICAL_PARAMS as usize,
            backend.execution_call_abis.clone(),
        );
        let mut b = WasmFrameBuilder::new(
            &mut func,
            reloc,
            &backend.import_ids,
            WasmBodyLayout {
                admission: ADMISSION_PARAM,
                first_word: FIRST_WORD,
            },
        );
        crate::wasm::pending_work::emit_thread_record(b.func);
        let thread = b.word();
        match body {
            Body::Push => {
                let base = generate_frame_push(
                    &mut b,
                    thread,
                    SUBJECT_PARAM,
                    HomeCount::Dynamic(HOMES_PARAM),
                );
                b.push_get(base);
            }
            Body::Pop { descending } => generate_frame_pop(
                &mut b,
                thread,
                SUBJECT_PARAM,
                HomeCount::Dynamic(HOMES_PARAM),
                descending,
            ),
            Body::PopLinked => generate_linked_frame_pop(&mut b, SUBJECT_PARAM),
        }
        assert!(b.closed(), "a frame body left a control label open");
        func.instruction(&Instruction::End);
        backend.codes.function(&func);
        backend.func_count += 1;
        index
    };
    let bodies = WasmFrameBodies {
        push: index_of(Body::Push),
        pop_ascending: index_of(Body::Pop { descending: false }),
        pop_descending: index_of(Body::Pop { descending: true }),
        pop_linked: index_of(Body::PopLinked),
    };
    backend.import_ids.set_frame_bodies(bodies);
}

/// Where a generated body keeps its RC admission parameter and its scratch:
/// `WORDS` i64 locals from `first_word`, then `PREDICATES` i32 locals.
#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmBodyLayout {
    pub(in crate::wasm) admission: u32,
    pub(in crate::wasm) first_word: u32,
}

impl WasmBodyLayout {
    /// The scratch locals a body with this layout declares after its
    /// parameters.
    pub(in crate::wasm) fn scratch_locals() -> Vec<ValType> {
        let mut locals = vec![ValType::I64; WORDS as usize];
        locals.extend(std::iter::repeat_n(ValType::I32, PREDICATES as usize));
        locals
    }
}

/// WASM's spelling of the shared control and word primitives in a generated
/// body. The method site generators reuse it (`method_site.rs`).
pub(in crate::wasm) struct WasmFrameBuilder<'a> {
    pub(in crate::wasm) func: &'a mut Function,
    pub(in crate::wasm) reloc: bool,
    pub(in crate::wasm) imports: &'a TrackedImportIds,
    layout: WasmBodyLayout,
    word_count: u32,
    predicate_count: u32,
    /// Control labels open at the emission point.
    open: u32,
    /// For each open `attempt`, `open` just inside its fast-path block.
    attempts: Vec<u32>,
    /// The function type of a fixed-arity compiled entry, by its number of
    /// Python arguments, for a method site's call body.
    pub(in crate::wasm) entry_types: std::collections::BTreeMap<usize, u32>,
}

impl<'a> WasmFrameBuilder<'a> {
    pub(in crate::wasm) fn new(
        func: &'a mut Function,
        reloc: bool,
        imports: &'a TrackedImportIds,
        layout: WasmBodyLayout,
    ) -> Self {
        Self {
            func,
            reloc,
            imports,
            layout,
            word_count: 0,
            predicate_count: 0,
            open: 0,
            attempts: Vec::new(),
            entry_types: std::collections::BTreeMap::new(),
        }
    }

    /// Whether every control label the body opened is closed.
    pub(in crate::wasm) fn closed(&self) -> bool {
        self.open == 0
    }
}

impl WasmFrameBuilder<'_> {
    pub(in crate::wasm) fn push(&mut self, instruction: Instruction<'static>) {
        self.func.instruction(&instruction);
    }
    fn fresh_word(&mut self) -> u32 {
        assert!(
            self.word_count < WORDS,
            "shared frame generator exhausted its word scratch"
        );
        let slot = self.layout.first_word + self.word_count;
        self.word_count += 1;
        slot
    }
    /// Pop the i64 on the operand stack into a fresh scratch word.
    pub(in crate::wasm) fn word(&mut self) -> u32 {
        let slot = self.fresh_word();
        self.push(Instruction::LocalSet(slot));
        slot
    }
    fn predicate(&mut self) -> u32 {
        assert!(
            self.predicate_count < PREDICATES,
            "shared frame generator exhausted its predicate scratch"
        );
        let slot = self.layout.first_word + WORDS + self.predicate_count;
        self.predicate_count += 1;
        self.push(Instruction::LocalSet(slot));
        slot
    }
    pub(in crate::wasm) fn push_get(&mut self, slot: u32) {
        self.push(Instruction::LocalGet(slot));
    }
    /// Push the i32 linear-memory address `pointer + offset`.
    fn address(&mut self, pointer: u32, offset: i32) {
        self.push_get(pointer);
        self.push(Instruction::I64Const(i64::from(offset)));
        self.push(Instruction::I64Add);
        self.push(Instruction::I32WrapI64);
    }
    fn refcount(&mut self, bits: u32, operation: RefcountOperation) {
        self.push_get(bits);
        self.push_get(self.layout.admission);
        emit_call(self.func, self.reloc, self.imports.refcount_body(operation));
    }
}

const WORD_MEMORY: MemArg = MemArg {
    align: 3,
    offset: 0,
    memory_index: 0,
};

impl WordBuilder for WasmFrameBuilder<'_> {
    type Word = u32;
    type Predicate = u32;
    fn constant(&mut self, value: u64) -> u32 {
        self.push(Instruction::I64Const(value as i64));
        self.word()
    }
    fn and(&mut self, value: u32, mask: u64) -> u32 {
        self.push_get(value);
        self.push(Instruction::I64Const(mask as i64));
        self.push(Instruction::I64And);
        self.word()
    }
    fn or(&mut self, value: u32, bits: u64) -> u32 {
        self.push_get(value);
        self.push(Instruction::I64Const(bits as i64));
        self.push(Instruction::I64Or);
        self.word()
    }
    fn shift_left(&mut self, value: u32, amount: u32) -> u32 {
        self.push_get(value);
        self.push(Instruction::I64Const(i64::from(amount)));
        self.push(Instruction::I64Shl);
        self.word()
    }
    fn shift_right_arithmetic(&mut self, value: u32, amount: u32) -> u32 {
        self.push_get(value);
        self.push(Instruction::I64Const(i64::from(amount)));
        self.push(Instruction::I64ShrS);
        self.word()
    }
    fn add_immediate(&mut self, value: u32, delta: i64) -> u32 {
        self.push_get(value);
        self.push(Instruction::I64Const(delta));
        self.push(Instruction::I64Add);
        self.word()
    }
    fn compare(&mut self, comparison: WordComparison, left: u32, right: u32) -> u32 {
        self.push_get(left);
        self.push_get(right);
        self.push(match comparison {
            WordComparison::Equal => Instruction::I64Eq,
            WordComparison::NotEqual => Instruction::I64Ne,
            WordComparison::UnsignedLessThan => Instruction::I64LtU,
        });
        self.predicate()
    }
    fn both(&mut self, left: u32, right: u32) -> u32 {
        self.push_get(left);
        self.push_get(right);
        self.push(Instruction::I32And);
        self.predicate()
    }
}

impl FrameBuilder for WasmFrameBuilder<'_> {
    type Local = u32;

    fn local(&mut self) -> u32 {
        self.fresh_word()
    }
    fn set(&mut self, local: u32, value: u32) {
        self.push_get(value);
        self.push(Instruction::LocalSet(local));
    }
    fn get(&mut self, local: u32) -> u32 {
        self.push_get(local);
        self.word()
    }
    fn load_word(&mut self, address: u32, offset: i32) -> u32 {
        self.address(address, offset);
        self.push(Instruction::I64Load(WORD_MEMORY));
        self.word()
    }
    fn store_word(&mut self, address: u32, offset: i32, value: u32) {
        self.address(address, offset);
        self.push_get(value);
        self.push(Instruction::I64Store(WORD_MEMORY));
    }
    fn load_header_u32(&mut self, pointer: u32, offset: i32) -> u32 {
        self.address(pointer, offset);
        self.push(Instruction::I64Load32U(MemArg {
            align: 2,
            offset: 0,
            memory_index: 0,
        }));
        self.word()
    }
    fn add(&mut self, left: u32, right: u32) -> u32 {
        self.push_get(left);
        self.push_get(right);
        self.push(Instruction::I64Add);
        self.word()
    }
    fn retain(&mut self, bits: u32) {
        self.refcount(bits, RefcountOperation::Retain);
    }
    fn release(&mut self, bits: u32) {
        self.refcount(bits, RefcountOperation::Release);
    }
    fn enter_slow(&mut self, code_slot: u32) -> u32 {
        self.push_get(code_slot);
        emit_call(
            self.func,
            self.reloc,
            self.imports[WasmRuntimeImport::FrameRecordEnter],
        );
        self.word()
    }
    fn exit_slow(&mut self, base: u32) {
        self.push_get(base);
        emit_call(
            self.func,
            self.reloc,
            self.imports[WasmRuntimeImport::FrameRecordExit],
        );
        self.push(Instruction::Drop);
    }
    fn attempt(&mut self, fast: impl FnOnce(&mut Self), slow: impl FnOnce(&mut Self)) {
        // A failed `require` leaves the inner block and falls into `slow`; a
        // finished fast path leaves the outer block around it.
        self.push(Instruction::Block(BlockType::Empty));
        self.push(Instruction::Block(BlockType::Empty));
        self.open += 2;
        self.attempts.push(self.open);
        fast(self);
        let inner = self.attempts.pop().expect("open attempt");
        self.push(Instruction::Br(self.open - inner + 1));
        self.push(Instruction::End);
        self.open -= 1;
        slow(self);
        self.push(Instruction::End);
        self.open -= 1;
    }
    fn require(&mut self, condition: u32) {
        let inner = *self
            .attempts
            .last()
            .expect("require outside a frame attempt");
        self.push_get(condition);
        self.push(Instruction::I32Eqz);
        self.push(Instruction::BrIf(self.open - inner));
    }
    fn if_then(&mut self, condition: u32, then: impl FnOnce(&mut Self)) {
        self.push_get(condition);
        self.push(Instruction::If(BlockType::Empty));
        self.open += 1;
        then(self);
        self.push(Instruction::End);
        self.open -= 1;
    }
    fn counted_loop(&mut self, count: u32, descending: bool, mut body: impl FnMut(&mut Self, u32)) {
        // Ascending: the next index. Descending: how many indices remain.
        let cursor = self.fresh_word();
        if descending {
            self.push_get(count);
        } else {
            self.push(Instruction::I64Const(0));
        }
        self.push(Instruction::LocalSet(cursor));
        self.push(Instruction::Block(BlockType::Empty));
        self.push(Instruction::Loop(BlockType::Empty));
        self.open += 2;
        self.push_get(cursor);
        if descending {
            self.push(Instruction::I64Eqz);
        } else {
            self.push_get(count);
            self.push(Instruction::I64Eq);
        }
        self.push(Instruction::BrIf(1));
        let index = self.fresh_word();
        self.push_get(cursor);
        if descending {
            self.push(Instruction::I64Const(1));
            self.push(Instruction::I64Sub);
            self.push(Instruction::LocalTee(cursor));
            self.push(Instruction::LocalSet(index));
        } else {
            self.push(Instruction::LocalTee(index));
            self.push(Instruction::I64Const(1));
            self.push(Instruction::I64Add);
            self.push(Instruction::LocalSet(cursor));
        }
        body(self, index);
        self.push(Instruction::Br(0));
        self.push(Instruction::End);
        self.push(Instruction::End);
        self.open -= 2;
    }
}
