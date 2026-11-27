//! Cranelift's spelling of the shared frame-record generators
//! (`molt_codegen_abi::frame_record`): a synchronous function pushes and pops
//! its own frame record inline.

use super::*;
use crate::codegen_ssa::frame::{self, FrameBody, FramePlacement};
use molt_codegen_abi::frame_record::{FrameBuilder, FrameProbe};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::thread_record::RecordWordBuilder;
use molt_codegen_abi::word::WordBuilder;
use molt_ir::frame_entry::FrameEntryKind;

/// What one function's frame transitions call off their inline paths.
#[derive(Clone, Copy)]
pub(crate) struct NativeFrameSites {
    pub(crate) retain: NativeRefcountSite,
    pub(crate) release: NativeRefcountSite,
    /// `molt_frame_record_enter`.
    pub(crate) enter: FuncRef,
    /// `molt_frame_record_exit`.
    pub(crate) exit: FuncRef,
}

/// Cranelift's spelling of the shared control and word primitives, inline in
/// the function being compiled. The method site generators reuse it
/// (`method_site.rs`).
pub(crate) struct NativeFrameBuilder<'a, 'b> {
    pub(crate) words: NativeWords<'a, 'b>,
    pub(crate) sites: NativeFrameSites,
    /// The cold block of each open `attempt`, innermost last.
    pub(crate) slow_blocks: Vec<Block>,
    /// Set only for a task construction (`task_site.rs`).
    pub(crate) tasks: Option<NativeTaskSites>,
}

impl<'a, 'b> WordBuilder for NativeFrameBuilder<'a, 'b> {
    molt_codegen_abi::forward_word_builder!(words: NativeWords<'a, 'b>);
}

impl RecordWordBuilder for NativeFrameBuilder<'_, '_> {
    fn load_word(&mut self, address: Value, offset: i32) -> Value {
        self.words.load_word(address, offset)
    }
    fn store_word(&mut self, address: Value, offset: i32, value: Value) {
        self.words.store_word(address, offset, value)
    }
    fn load_shared_word(&mut self, address: Value, offset: i32) -> Value {
        self.words.load_shared_word(address, offset)
    }
}

pub(crate) fn emit_frame_probe(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    thread: Value,
    function: Value,
    code: Value,
    probe: FrameProbe,
) -> Value {
    let mut b = NativeFrameBuilder {
        words: NativeWords { builder },
        sites,
        slow_blocks: Vec::new(),
        tasks: None,
    };
    molt_codegen_abi::frame_record::generate_frame_probe(&mut b, thread, function, code, probe)
}

impl NativeFrameBuilder<'_, '_> {
    fn enter_block(&mut self, block: Block) {
        // Every block here is private to the generator and entered once all
        // its predecessors exist.
        self.words.builder.switch_to_block(block);
        self.words.builder.seal_block(block);
    }
}

impl FrameBuilder for NativeFrameBuilder<'_, '_> {
    type Local = Variable;

    fn local(&mut self) -> Variable {
        self.words.builder.declare_var(types::I64)
    }
    fn set(&mut self, local: Variable, value: Value) {
        self.words.builder.def_var(local, value);
    }
    fn get(&mut self, local: Variable) -> Value {
        self.words.builder.use_var(local)
    }
    fn load_header_u32(&mut self, pointer: Value, offset: i32) -> Value {
        let value =
            self.words
                .builder
                .ins()
                .load(types::I32, MemFlagsData::trusted(), pointer, offset);
        self.words.builder.ins().uextend(types::I64, value)
    }
    fn add(&mut self, left: Value, right: Value) -> Value {
        self.words.builder.ins().iadd(left, right)
    }
    fn retain(&mut self, bits: Value) {
        emit_refcount(self.words.builder, bits, self.sites.retain);
    }
    fn release(&mut self, bits: Value) {
        emit_refcount(self.words.builder, bits, self.sites.release);
    }
    fn frame_owner_refcount(
        &mut self,
        bits: Value,
        operation: RefcountOperation,
        owner: molt_codegen_abi::frame_record::FrameOwner,
    ) {
        let site = match operation {
            RefcountOperation::Retain => self.sites.retain,
            RefcountOperation::Release => self.sites.release,
        };
        super::refcount::emit_refcount_for_owner(self.words.builder, bits, site, Some(owner));
    }
    fn enter_slow(&mut self, code_slot: Value) -> Value {
        let call = self
            .words
            .builder
            .ins()
            .call(self.sites.enter, &[code_slot]);
        self.words.builder.inst_results(call)[0]
    }
    fn exit_slow(&mut self, base: Value) {
        self.words.builder.ins().call(self.sites.exit, &[base]);
    }
    fn attempt(&mut self, fast: impl FnOnce(&mut Self), slow: impl FnOnce(&mut Self)) {
        let slow_block = self.words.builder.create_block();
        let done = self.words.builder.create_block();
        self.words.builder.set_cold_block(slow_block);
        self.slow_blocks.push(slow_block);
        fast(self);
        self.slow_blocks.pop();
        self.words.builder.ins().jump(done, &[]);
        // Every `require` of the fast path has branched here by now.
        self.enter_block(slow_block);
        slow(self);
        self.words.builder.ins().jump(done, &[]);
        self.enter_block(done);
    }
    fn require(&mut self, condition: Value) {
        let slow_block = *self
            .slow_blocks
            .last()
            .expect("require outside a frame attempt");
        let next = self.words.builder.create_block();
        self.words
            .builder
            .ins()
            .brif(condition, next, &[], slow_block, &[]);
        self.enter_block(next);
    }
    fn if_then(&mut self, condition: Value, then: impl FnOnce(&mut Self)) {
        let then_block = self.words.builder.create_block();
        let done = self.words.builder.create_block();
        self.words
            .builder
            .ins()
            .brif(condition, then_block, &[], done, &[]);
        self.enter_block(then_block);
        then(self);
        self.words.builder.ins().jump(done, &[]);
        self.enter_block(done);
    }
    fn counted_loop(
        &mut self,
        count: Value,
        descending: bool,
        mut body: impl FnMut(&mut Self, Value),
    ) {
        let builder = &mut *self.words.builder;
        // Ascending: the next index. Descending: how many indices remain.
        let cursor = builder.declare_var(types::I64);
        let start = if descending {
            count
        } else {
            builder.ins().iconst(types::I64, 0)
        };
        builder.def_var(cursor, start);
        let header = builder.create_block();
        let body_block = builder.create_block();
        let done = builder.create_block();
        builder.ins().jump(header, &[]);
        builder.switch_to_block(header);
        let current = builder.use_var(cursor);
        let finished = if descending {
            builder.ins().icmp_imm_s(IntCC::Equal, current, 0)
        } else {
            builder.ins().icmp(IntCC::Equal, current, count)
        };
        builder.ins().brif(finished, done, &[], body_block, &[]);
        self.enter_block(body_block);
        let builder = &mut *self.words.builder;
        let index = if descending {
            let index = builder.ins().iadd_imm_s(current, -1);
            builder.def_var(cursor, index);
            index
        } else {
            let next = builder.ins().iadd_imm_s(current, 1);
            builder.def_var(cursor, next);
            current
        };
        body(self, index);
        self.words.builder.ins().jump(header, &[]);
        // The back edge is in: the header has all its predecessors.
        self.words.builder.seal_block(header);
        self.enter_block(done);
    }
}

/// Enter a linked frame and return the record base or zero on failure.
pub(crate) fn emit_frame_record_push(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    record: Option<Value>,
    code: Value,
    kind: Option<FrameEntryKind>,
    payload: Option<Value>,
) -> Value {
    let mut placement = NativeFramePlacement::new(builder, sites, record);
    frame::enter(&mut placement, code, kind, |_| {
        payload.expect("stateful frame payload")
    })
}

/// Pop the frame `base` names, if this invocation entered one.
pub(crate) fn emit_frame_record_pop(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    thread_record: Option<Value>,
    base: Value,
    kind: Option<FrameEntryKind>,
) {
    let mut placement = NativeFramePlacement::new(builder, sites, thread_record);
    frame::leave(&mut placement, base, kind);
}

struct NativeFramePlacement<'a, 'b> {
    frames: NativeFrameBuilder<'a, 'b>,
    thread_record: Option<Value>,
}

impl<'a, 'b> NativeFramePlacement<'a, 'b> {
    fn new(
        builder: &'a mut FunctionBuilder<'b>,
        sites: NativeFrameSites,
        thread_record: Option<Value>,
    ) -> Self {
        Self {
            frames: NativeFrameBuilder {
                words: NativeWords { builder },
                sites,
                slow_blocks: Vec::new(),

                tasks: None,
            },
            thread_record,
        }
    }
}

impl FramePlacement for NativeFramePlacement<'_, '_> {
    type Word = Value;

    fn thread(&mut self) -> Value {
        self.thread_record
            .expect("inline frame links need the attached record")
    }
    fn admission(&mut self, op: RefcountOperation) -> Value {
        match op {
            RefcountOperation::Retain => self.frames.sites.retain.admission,
            RefcountOperation::Release => self.frames.sites.release.admission,
        }
    }
    fn zero(&mut self) -> Value {
        self.frames.constant(0)
    }
    fn enter_runtime(&mut self, code: Value) -> Value {
        self.frames.enter_slow(code)
    }
    fn place(&mut self, body: FrameBody, [thread, subject, third]: [Value; 3]) -> Option<Value> {
        body.generate(&mut self.frames, thread, subject, third)
    }
}

impl molt_codegen_abi::field_slot::FieldSlotBuilder for NativeFrameBuilder<'_, '_> {
    fn or_header_flag(&mut self, pointer: Value, flag: u32) {
        let builder = &mut *self.words.builder;
        let at = builder
            .ins()
            .iadd_imm_s(pointer, i64::from(molt_codegen_abi::HEADER_FLAGS_OFFSET));
        let flag = builder.ins().iconst(types::I32, i64::from(flag));
        builder.ins().atomic_rmw(
            types::I32,
            MemFlagsData::trusted(),
            cranelift_codegen::ir::AtomicRmwOp::Or,
            at,
            flag,
        );
    }
}
