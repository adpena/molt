//! LLVM's spelling of the shared frame-record generators
//! (`molt_codegen_abi::frame_record`), in always-inlined LLVM bodies. As with
//! the RC transitions (`refcount.rs`), keeping the generated blocks in a
//! separate IR function leaves the caller's TIR/phi CFG intact; LLVM inlines
//! the body into the one entry and the one exit that call it.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use inkwell::values::{IntValue, PointerValue};
use molt_codegen_abi::frame_record::{
    FrameBuilder, FrameShape, HomeCount, generate_frame_pop, generate_frame_push,
    generate_linked_frame_pop,
};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> FunctionLowering<'ctx, '_> {
    /// The thread record as a word, for the frame helpers' address arithmetic.
    fn thread_record_word(&mut self) -> IntValue<'ctx> {
        let record = self.thread_record();
        self.backend
            .builder
            .build_ptr_to_int(
                record,
                self.backend.context.i64_type(),
                "thread_record_word",
            )
            .unwrap()
    }

    /// Enter this invocation's frame for code slot `code_slot` and return its
    /// record base: zero when nothing was linked. A synchronous frame
    /// (`shape`) pushes its record inline; any other frame is runtime-linked.
    pub(super) fn emit_frame_record_push(
        &mut self,
        code_slot: IntValue<'ctx>,
        shape: Option<FrameShape>,
    ) -> IntValue<'ctx> {
        let Some(shape) = shape else {
            let enter = self.ensure_runtime_import(RuntimeFn::FrameRecordEnter);
            return self
                .backend
                .builder
                .build_execution_call(enter, &[code_slot.into()], "frame_record_enter")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value();
        };
        let thread = self.thread_record_word();
        let admission = self
            .ensure_refcount_site(RefcountOperation::Retain)
            .admission;
        let helper = self.ensure_frame_helper(FrameHelper::Push { homes: shape.homes });
        self.backend
            .builder
            .build_execution_call(
                helper,
                &[thread.into(), code_slot.into(), admission.into()],
                "frame_base",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    /// Pop the frame `base` names, if this invocation entered one.
    pub(super) fn emit_frame_record_pop(
        &mut self,
        base: IntValue<'ctx>,
        shape: Option<FrameShape>,
    ) {
        // The runtime exits a frame it linked: that body reads neither the
        // thread record nor the RC admission.
        let (thread, admission) = match shape {
            Some(_) => (
                self.thread_record_word(),
                self.ensure_refcount_site(RefcountOperation::Release)
                    .admission,
            ),
            None => {
                let zero = self.backend.context.i64_type().const_zero();
                (zero, zero)
            }
        };
        let helper = self.ensure_frame_helper(match shape {
            Some(shape) => FrameHelper::Pop(shape),
            None => FrameHelper::PopRuntimeLinked,
        });
        self.backend
            .builder
            .build_execution_call(helper, &[thread.into(), base.into(), admission.into()], "")
            .unwrap();
    }

    fn ensure_frame_helper(&self, helper: FrameHelper) -> FunctionValue<'ctx> {
        let name = match helper {
            FrameHelper::Push { homes } => format!("__molt_frame_push_{homes}"),
            FrameHelper::Pop(shape) => format!(
                "__molt_frame_pop_{}_{}",
                shape.homes,
                if shape.descending { "down" } else { "up" }
            ),
            FrameHelper::PopRuntimeLinked => "__molt_frame_pop_linked".to_string(),
        };
        let context = self.backend.context;
        let i64_type = context.i64_type();
        let params = [i64_type.into(), i64_type.into(), i64_type.into()];
        let logical_signature = match helper {
            FrameHelper::Push { .. } => i64_type.fn_type(&params, false),
            FrameHelper::Pop(_) | FrameHelper::PopRuntimeLinked => {
                context.void_type().fn_type(&params, false)
            }
        };
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.backend.module.get_compiled_function(&name) {
            return require_llvm_function_type(&name, function, signature);
        }
        let function = self.backend.module.add_compiled_function(
            &name,
            logical_signature,
            Some(inkwell::module::Linkage::Internal),
        );
        function.add_attribute(
            AttributeLoc::Function,
            context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("alwaysinline"),
                0,
            ),
        );
        let entry = context.append_basic_block(function, "entry");
        let body = context.append_basic_block(function, "body");
        let builder = context.create_builder();
        builder.position_at_end(body);
        let param = |index| function.get_nth_param(index).unwrap().into_int_value();
        let (thread, subject, admission) = (param(1), param(2), param(3));
        let mut frames = LlvmFrameBuilder {
            words: LlvmWords {
                context,
                builder: &builder,
            },
            function,
            entry,
            admission,
            retain: self.ensure_refcount_helper(RefcountOperation::Retain),
            release: self.ensure_refcount_helper(RefcountOperation::Release),
            enter: self.ensure_runtime_import(RuntimeFn::FrameRecordEnter),
            exit: self.ensure_runtime_import(RuntimeFn::FrameRecordExit),
            slow_blocks: Vec::new(),
        };
        match helper {
            FrameHelper::Push { homes } => {
                let base =
                    generate_frame_push(&mut frames, thread, subject, HomeCount::Constant(homes));
                let base = frames.get(base);
                builder.build_return(Some(&base)).unwrap();
            }
            FrameHelper::Pop(shape) => {
                generate_frame_pop(
                    &mut frames,
                    thread,
                    subject,
                    HomeCount::Constant(shape.homes),
                    shape.descending,
                );
                builder.build_return(None).unwrap();
            }
            FrameHelper::PopRuntimeLinked => {
                generate_linked_frame_pop(&mut frames, subject);
                builder.build_return(None).unwrap();
            }
        }
        // The locals were allocated in `entry` as the body asked for them.
        builder.position_at_end(entry);
        builder.build_unconditional_branch(body).unwrap();
        function
    }
}

#[derive(Clone, Copy)]
enum FrameHelper {
    Push { homes: u32 },
    Pop(FrameShape),
    PopRuntimeLinked,
}

struct LlvmFrameBuilder<'a, 'ctx> {
    words: LlvmWords<'a, 'ctx>,
    function: FunctionValue<'ctx>,
    /// Holds only the helper's local slots; it branches to the body last.
    entry: BasicBlock<'ctx>,
    admission: IntValue<'ctx>,
    retain: FunctionValue<'ctx>,
    release: FunctionValue<'ctx>,
    enter: FunctionValue<'ctx>,
    exit: FunctionValue<'ctx>,
    /// The cold block of each open `attempt`, innermost last.
    slow_blocks: Vec<BasicBlock<'ctx>>,
}

impl<'ctx> LlvmFrameBuilder<'_, 'ctx> {
    fn block(&self, name: &str) -> BasicBlock<'ctx> {
        self.words.context.append_basic_block(self.function, name)
    }

    fn address(&self, pointer: IntValue<'ctx>, offset: i32) -> PointerValue<'ctx> {
        let i64_type = self.words.context.i64_type();
        let address = self
            .words
            .builder
            .build_int_add(
                pointer,
                i64_type.const_int(offset as u64, true),
                "frame_address",
            )
            .unwrap();
        self.words
            .builder
            .build_int_to_ptr(
                address,
                self.words
                    .context
                    .ptr_type(inkwell::AddressSpace::default()),
                "frame_word",
            )
            .unwrap()
    }
}

impl<'a, 'ctx> WordBuilder for LlvmFrameBuilder<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> FrameBuilder for LlvmFrameBuilder<'_, 'ctx> {
    type Local = PointerValue<'ctx>;

    fn local(&mut self) -> Self::Local {
        let slots = self.words.context.create_builder();
        slots.position_at_end(self.entry);
        slots
            .build_alloca(self.words.context.i64_type(), "frame_local")
            .unwrap()
    }
    fn set(&mut self, local: Self::Local, value: Self::Word) {
        self.words.builder.build_store(local, value).unwrap();
    }
    fn get(&mut self, local: Self::Local) -> Self::Word {
        self.words
            .builder
            .build_load(self.words.context.i64_type(), local, "frame_local_value")
            .unwrap()
            .into_int_value()
    }
    fn load_word(&mut self, address: Self::Word, offset: i32) -> Self::Word {
        self.words
            .builder
            .build_load(
                self.words.context.i64_type(),
                self.address(address, offset),
                "frame_load",
            )
            .unwrap()
            .into_int_value()
    }
    fn store_word(&mut self, address: Self::Word, offset: i32, value: Self::Word) {
        self.words
            .builder
            .build_store(self.address(address, offset), value)
            .unwrap();
    }
    fn load_header_u32(&mut self, pointer: Self::Word, offset: i32) -> Self::Word {
        let value = self
            .words
            .builder
            .build_load(
                self.words.context.i32_type(),
                self.address(pointer, offset),
                "frame_header",
            )
            .unwrap()
            .into_int_value();
        self.words
            .builder
            .build_int_z_extend(value, self.words.context.i64_type(), "frame_header_word")
            .unwrap()
    }
    fn add(&mut self, left: Self::Word, right: Self::Word) -> Self::Word {
        self.words
            .builder
            .build_int_add(left, right, "frame_add")
            .unwrap()
    }
    fn retain(&mut self, bits: Self::Word) {
        self.words
            .builder
            .build_execution_call(self.retain, &[bits.into(), self.admission.into()], "")
            .unwrap();
    }
    fn release(&mut self, bits: Self::Word) {
        self.words
            .builder
            .build_execution_call(self.release, &[bits.into(), self.admission.into()], "")
            .unwrap();
    }
    fn enter_slow(&mut self, code_slot: Self::Word) -> Self::Word {
        self.words
            .builder
            .build_execution_call(self.enter, &[code_slot.into()], "frame_record_enter")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }
    fn exit_slow(&mut self, base: Self::Word) {
        self.words
            .builder
            .build_execution_call(self.exit, &[base.into()], "")
            .unwrap();
    }
    fn attempt(&mut self, fast: impl FnOnce(&mut Self), slow: impl FnOnce(&mut Self)) {
        let slow_block = self.block("frame_slow");
        let done = self.block("frame_done");
        self.slow_blocks.push(slow_block);
        fast(self);
        self.slow_blocks.pop();
        self.words.builder.build_unconditional_branch(done).unwrap();
        self.words.builder.position_at_end(slow_block);
        slow(self);
        self.words.builder.build_unconditional_branch(done).unwrap();
        self.words.builder.position_at_end(done);
    }
    fn require(&mut self, condition: Self::Predicate) {
        let slow_block = *self
            .slow_blocks
            .last()
            .expect("require outside a frame attempt");
        let next = self.block("frame_next");
        self.words
            .builder
            .build_conditional_branch(condition, next, slow_block)
            .unwrap();
        self.words.builder.position_at_end(next);
    }
    fn if_then(&mut self, condition: Self::Predicate, then: impl FnOnce(&mut Self)) {
        let then_block = self.block("frame_then");
        let done = self.block("frame_after");
        self.words
            .builder
            .build_conditional_branch(condition, then_block, done)
            .unwrap();
        self.words.builder.position_at_end(then_block);
        then(self);
        self.words.builder.build_unconditional_branch(done).unwrap();
        self.words.builder.position_at_end(done);
    }
    fn counted_loop(
        &mut self,
        count: Self::Word,
        descending: bool,
        mut body: impl FnMut(&mut Self, Self::Word),
    ) {
        let i64_type = self.words.context.i64_type();
        // Ascending: the next index. Descending: how many indices remain.
        let cursor = self.local();
        let start = if descending {
            count
        } else {
            i64_type.const_zero()
        };
        self.set(cursor, start);
        let header = self.block("frame_loop");
        let body_block = self.block("frame_loop_body");
        let done = self.block("frame_loop_done");
        self.words
            .builder
            .build_unconditional_branch(header)
            .unwrap();
        self.words.builder.position_at_end(header);
        let current = self.get(cursor);
        let finished = self
            .words
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                current,
                if descending {
                    i64_type.const_zero()
                } else {
                    count
                },
                "frame_loop_finished",
            )
            .unwrap();
        self.words
            .builder
            .build_conditional_branch(finished, done, body_block)
            .unwrap();
        self.words.builder.position_at_end(body_block);
        let index = if descending {
            let index = self.add_immediate(current, -1);
            self.set(cursor, index);
            index
        } else {
            let next = self.add_immediate(current, 1);
            self.set(cursor, next);
            current
        };
        body(self, index);
        self.words
            .builder
            .build_unconditional_branch(header)
            .unwrap();
        self.words.builder.position_at_end(done);
    }
}
