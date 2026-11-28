//! LLVM's spelling of the shared frame-record generators
//! (`molt_codegen_abi::frame_record`), in always-inlined LLVM bodies. As with
//! the RC transitions (`refcount.rs`), keeping the generated blocks in a
//! separate IR function leaves the caller's TIR/phi CFG intact; LLVM inlines
//! the body into the one entry and the one exit that call it.
use super::*;
use crate::codegen_ssa::frame::{self, FrameBody, FramePlacement};
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use inkwell::values::{BasicValue, IntValue, PointerValue};
use molt_codegen_abi::frame_record::{FrameBuilder, FrameProbe, FrameReservation};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::thread_record::RecordWordBuilder;
use molt_codegen_abi::word::WordBuilder;
use molt_ir::frame_entry::FrameEntryKind;

impl<'ctx> LlvmBackend<'ctx> {
    pub(super) fn frame_probe_helper(&self, probe: FrameProbe) -> FunctionValue<'ctx> {
        let name = match probe {
            FrameProbe::Entry(FrameReservation {
                arena_bytes,
                recursion_depth,
                virtual_exception_level,
                callable_admitted,
            }) => format!(
                "__molt_frame_entry_probe_{arena_bytes}_{recursion_depth}_{}_{}",
                u8::from(virtual_exception_level),
                u8::from(callable_admitted),
            ),
            FrameProbe::Region {
                arena_bytes,
                recursion_depth,
                virtual_exception_level,
            } => format!(
                "__molt_frame_region_probe_{arena_bytes}_{recursion_depth}_{}",
                u8::from(virtual_exception_level),
            ),
        };
        self.site_helper(&name, 4, |b, words| {
            let &[thread, function, code] = words else {
                unreachable!("frame entry probe arguments")
            };
            molt_codegen_abi::frame_record::generate_frame_probe(b, thread, function, code, probe)
        })
    }

    fn ensure_frame_helper(&self, helper: FrameBody) -> FunctionValue<'ctx> {
        let name = match helper {
            FrameBody::Push { homes } => format!("__molt_frame_push_{homes}"),
            FrameBody::Pop(shape) => format!(
                "__molt_frame_pop_{}_{}",
                shape.homes,
                if shape.descending { "down" } else { "up" }
            ),
            FrameBody::PopRuntimeLinked => "__molt_frame_pop_linked".to_string(),
            FrameBody::StatefulLink => "__molt_frame_link_stateful".to_string(),
            FrameBody::StatefulUnlink => "__molt_frame_unlink_stateful".to_string(),
        };
        let context = self.context;
        let i64_type = context.i64_type();
        let params = [i64_type.into(), i64_type.into(), i64_type.into()];
        let logical_signature = match helper {
            FrameBody::Push { .. } | FrameBody::StatefulLink => i64_type.fn_type(&params, false),
            FrameBody::Pop(_) | FrameBody::PopRuntimeLinked | FrameBody::StatefulUnlink => {
                context.void_type().fn_type(&params, false)
            }
        };
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.module.get_compiled_function(&name) {
            return require_llvm_function_type(&name, function, signature);
        }
        let function = self.module.add_compiled_function(
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
        // A stateful link's third parameter is its code slot; the stateful
        // helpers retain and release nothing.
        let (thread, subject, admission) = (param(1), param(2), param(3));
        let mut frames = LlvmFrameBuilder {
            backend: self,
            words: LlvmWords {
                context,
                builder: &builder,
            },
            function,
            entry,
            admission,
            retain: self.ensure_refcount_helper(RefcountOperation::Retain),
            release: self.ensure_refcount_helper(RefcountOperation::Release),
            enter: self.runtime_import(RuntimeFn::FrameRecordEnter),
            exit: self.runtime_import(RuntimeFn::FrameRecordExit),
            slow_blocks: Vec::new(),
            task_imports: None,
        };
        let result = helper.generate(&mut frames, thread, subject, admission);
        builder
            .build_return(result.as_ref().map(|word| word as &dyn BasicValue<'ctx>))
            .unwrap();
        // The locals were allocated in `entry` as the body asked for them.
        builder.position_at_end(entry);
        builder.build_unconditional_branch(body).unwrap();
        function
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_probe_helpers_key_callable_admission() {
        let reservation = FrameReservation::from_counts(2, 1).unwrap();
        let admitted = FrameProbe::Entry(FrameReservation {
            callable_admitted: true,
            ..reservation
        });
        let checked = FrameProbe::Entry(reservation);
        for [first, second] in [[admitted, checked], [checked, admitted]] {
            let context = inkwell::context::Context::create();
            let backend = LlvmBackend::new(&context, "frame_probe_admission");
            let first_helper = backend.frame_probe_helper(first);
            let second_helper = backend.frame_probe_helper(second);
            assert_ne!(first_helper, second_helper);
            assert_eq!(first_helper, backend.frame_probe_helper(first));
            assert_eq!(second_helper, backend.frame_probe_helper(second));
            backend.module.verify().unwrap();
        }
    }
}

impl<'ctx> frame::RecordPlacement for LlvmCodegen<'_, 'ctx> {
    fn push_frame(
        &mut self,
        code: IntValue<'ctx>,
        kind: Option<FrameEntryKind>,
        payload: Option<IntValue<'ctx>>,
    ) -> IntValue<'ctx> {
        frame::enter(self, code, kind, |_| {
            payload.expect("stateful frame payload")
        })
    }
}

impl<'ctx> FramePlacement for LlvmCodegen<'_, 'ctx> {
    type Word = IntValue<'ctx>;

    fn thread(&mut self) -> Self::Word {
        self.thread_record_word()
    }
    fn admission(&mut self, op: RefcountOperation) -> Self::Word {
        self.llvm_refcount_site(op).admission
    }
    fn zero(&mut self) -> Self::Word {
        self.backend.context.i64_type().const_zero()
    }
    fn enter_runtime(&mut self, code: Self::Word) -> Self::Word {
        let callee = self.backend.runtime_import(RuntimeFn::FrameRecordEnter);
        self.backend
            .builder
            .build_execution_call(callee, &[code.into()], "frame_record_enter")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }
    fn place(&mut self, body: FrameBody, args: [Self::Word; 3]) -> Option<Self::Word> {
        let callee = self.backend.ensure_frame_helper(body);
        let name = if callee.get_type().get_return_type().is_some() {
            "frame_base"
        } else {
            ""
        };
        self.backend
            .builder
            .build_execution_call(callee, &args.map(Into::into), name)
            .unwrap()
            .try_as_basic_value()
            .basic()
            .map(|value| value.into_int_value())
    }
}

/// LLVM's spelling of the shared control and word primitives, in an
/// always-inlined helper body. The method site generators reuse it
/// (`method_site.rs`).
pub(super) struct LlvmFrameBuilder<'a, 'ctx> {
    pub(super) backend: &'a LlvmBackend<'ctx>,
    pub(super) words: LlvmWords<'a, 'ctx>,
    pub(super) function: FunctionValue<'ctx>,
    /// Holds only the helper's local slots; it branches to the body last.
    pub(super) entry: BasicBlock<'ctx>,
    pub(super) admission: IntValue<'ctx>,
    pub(super) retain: FunctionValue<'ctx>,
    pub(super) release: FunctionValue<'ctx>,
    pub(super) enter: FunctionValue<'ctx>,
    pub(super) exit: FunctionValue<'ctx>,
    /// The cold block of each open `attempt`, innermost last.
    pub(super) slow_blocks: Vec<BasicBlock<'ctx>>,
    /// Set only in a task helper (`task_site.rs`).
    pub(super) task_imports: Option<super::task_site::LlvmTaskImports<'ctx>>,
}

impl<'ctx> LlvmFrameBuilder<'_, 'ctx> {
    fn block(&self, name: &str) -> BasicBlock<'ctx> {
        self.words.context.append_basic_block(self.function, name)
    }

    fn address(&self, pointer: IntValue<'ctx>, offset: i32) -> PointerValue<'ctx> {
        self.words.address(pointer, offset)
    }
}

impl<'ctx> LlvmWords<'_, 'ctx> {
    pub(super) fn address(&self, pointer: IntValue<'ctx>, offset: i32) -> PointerValue<'ctx> {
        let i64_type = self.context.i64_type();
        let address = self
            .builder
            .build_int_add(
                pointer,
                i64_type.const_int(offset as u64, true),
                "record_address",
            )
            .unwrap();
        self.builder
            .build_int_to_ptr(
                address,
                self.context.ptr_type(inkwell::AddressSpace::default()),
                "record_word",
            )
            .unwrap()
    }
}

impl<'ctx> RecordWordBuilder for LlvmWords<'_, 'ctx> {
    fn load_word(&mut self, address: Self::Word, offset: i32) -> Self::Word {
        self.builder
            .build_load(
                self.context.i64_type(),
                self.address(address, offset),
                "record_load",
            )
            .unwrap()
            .into_int_value()
    }
    fn store_word(&mut self, address: Self::Word, offset: i32, value: Self::Word) {
        self.builder
            .build_store(self.address(address, offset), value)
            .unwrap();
    }
    /// A monotonic atomic load: LLVM gives a racing plain load no meaning.
    fn load_shared_word(&mut self, address: Self::Word, offset: i32) -> Self::Word {
        let load = self
            .builder
            .build_load(
                self.context.i64_type(),
                self.address(address, offset),
                "record_shared_load",
            )
            .unwrap();
        let instruction = load
            .as_instruction_value()
            .expect("a load is an instruction");
        instruction.set_alignment(8).unwrap();
        instruction
            .set_atomic_ordering(inkwell::AtomicOrdering::Monotonic)
            .unwrap();
        load.into_int_value()
    }
}

impl<'a, 'ctx> WordBuilder for LlvmFrameBuilder<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> RecordWordBuilder for LlvmFrameBuilder<'_, 'ctx> {
    fn load_word(&mut self, address: Self::Word, offset: i32) -> Self::Word {
        self.words.load_word(address, offset)
    }
    fn store_word(&mut self, address: Self::Word, offset: i32, value: Self::Word) {
        self.words.store_word(address, offset, value)
    }
    fn load_shared_word(&mut self, address: Self::Word, offset: i32) -> Self::Word {
        self.words.load_shared_word(address, offset)
    }
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
    fn frame_owner_refcount(
        &mut self,
        bits: Self::Word,
        operation: RefcountOperation,
        owner: molt_codegen_abi::frame_record::FrameOwner,
    ) {
        let callee = self
            .backend
            .ensure_owner_refcount_helper(operation, Some(owner));
        self.words
            .builder
            .build_execution_call(callee, &[bits.into(), self.admission.into()], "")
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

impl<'ctx> molt_codegen_abi::field_slot::FieldSlotBuilder for LlvmFrameBuilder<'_, 'ctx> {
    fn or_header_flag(&mut self, pointer: Self::Word, flag: u32) {
        let i32_type = self.words.context.i32_type();
        let flags = self.address(pointer, molt_codegen_abi::HEADER_FLAGS_OFFSET);
        self.words
            .builder
            .build_atomicrmw(
                inkwell::AtomicRMWBinOp::Or,
                flags,
                i32_type.const_int(u64::from(flag), false),
                inkwell::AtomicOrdering::Monotonic,
            )
            .unwrap();
    }
}
