use super::*;
use crate::codegen_ssa::custody::{self, Operands};
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use inkwell::values::BasicValue;

/// A runtime-call argument on LLVM: the walker's, over LLVM values.
pub(super) type RuntimeArg<'ctx> = custody::RuntimeArg<BasicValueEnum<'ctx>>;

/// Operation-local custody of borrowed boxed operands on LLVM: the walker's
/// (`codegen_ssa::custody`), over LLVM words, entry slots and blocks.
pub(super) type BorrowedOperands<'ctx> = custody::BorrowedOperands<
    inkwell::values::IntValue<'ctx>,
    inkwell::values::PointerValue<'ctx>,
    BasicBlock<'ctx>,
>;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Match native/WASM invocation ownership: entry can fail before a frame
    /// exists, and every return (including suspension) closes only its own.
    pub(super) fn initialize_owned_execution_frame(&mut self) {
        if self.func.execution_context != crate::ir::ExecutionContextPolicy::Local {
            return;
        }
        assert!(self.owned_frame_entered.is_none());
        let builder = super::codegen_builder::entry_builder(self.backend.context, self.llvm_fn);
        let slot = builder
            .build_alloca(self.backend.context.i64_type(), "owned_frame_base")
            .unwrap();
        builder
            .build_store(slot, self.backend.context.i64_type().const_zero())
            .unwrap();
        self.owned_frame_entered = Some(crate::codegen_ssa::returns::OwnedFrame {
            slot,
            kind: self.frame_entry_kind(),
        });
    }

    pub(super) fn program_slot_id(
        &self,
        kind: molt_ir::code_slots::ProgramSlotKind,
        op: &TirOp,
    ) -> inkwell::values::IntValue<'ctx> {
        let owner = match op.attrs.get("s_value") {
            Some(AttrValue::Str(owner)) => owner.as_str(),
            _ => panic!("admitted program-slot owner"),
        };
        self.load_program_slot(kind, owner)
    }

    /// The program slot of the class statement a layout op names (HF-220).
    pub(super) fn class_definition_slot(&self, op: &TirOp) -> inkwell::values::IntValue<'ctx> {
        let owner = match op.attrs.get("class_definition") {
            Some(AttrValue::Str(owner)) => owner.as_str(),
            _ => panic!("admitted class statement symbol"),
        };
        self.load_program_slot(molt_ir::code_slots::ProgramSlotKind::Code, owner)
    }

    fn load_program_slot(
        &self,
        kind: molt_ir::code_slots::ProgramSlotKind,
        owner: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        self.codegen().program_slot(kind, owner)
    }

    /// What the walker's custody reads from this function.
    pub(super) fn operands(&self) -> Operands<'_, LlvmCodegen<'_, 'ctx>> {
        Operands {
            function: &self.func.name,
            values: &self.values,
            plan: &self.repr_facts,
            diagnostics: &self.diagnostics,
        }
    }

    /// Open custody over every operand the operation may borrow
    /// (`Operands::begin`). It must precede the operation's first failure
    /// edge.
    pub(super) fn begin_borrowed_operands(
        &mut self,
        operands: &[ValueId],
        label: &str,
    ) -> BorrowedOperands<'ctx> {
        self.operands().begin(&mut self.codegen(), operands, label)
    }

    /// The operand's boxed word, boxed at its first request.
    pub(super) fn borrowed_operand(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
        operand: ValueId,
    ) -> inkwell::values::IntValue<'ctx> {
        self.operands()
            .operand(&mut self.codegen(), custody, operand)
    }

    /// Rejoin the committed path with the failure block and release the
    /// custody's owners (`Operands::finish`). `on_abort` runs on the failure
    /// path only.
    pub(super) fn finish_borrowed_operands(
        &mut self,
        custody: BorrowedOperands<'ctx>,
        committed: inkwell::values::IntValue<'ctx>,
        result_name: &str,
        on_abort: impl FnOnce(&Self),
    ) -> inkwell::values::IntValue<'ctx> {
        self.operands()
            .finish(&mut self.codegen(), custody, committed, result_name, |_| {
                on_abort(self)
            })
    }

    /// Emit a borrowed runtime call and bind or retire its word by `custody`.
    pub(super) fn emit_borrowed_runtime_call(
        &mut self,
        op: &TirOp,
        callee: FunctionValue<'ctx>,
        args: &[RuntimeArg<'ctx>],
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let (mut cx, mut b) = self.walker();
        let callee = crate::codegen_ssa::custody::CallTarget::Callee(callee);
        cx.runtime_call(&mut b, op, callee, args, custody, label, call_name);
    }

    /// `emit_borrowed_runtime_call` over every operand of `op`, in order.
    pub(super) fn emit_positional_runtime_call(
        &mut self,
        op: &TirOp,
        callee: FunctionValue<'ctx>,
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let (mut cx, mut b) = self.walker();
        cx.positional_runtime_call(&mut b, op, callee, custody, label, call_name);
    }

    /// Return from this function with no value of its own: void, or None in
    /// its linkage return carrier. The return of a body without a value, and
    /// the early exit whose caller reads the pending exception instead.
    pub(super) fn build_empty_return(&mut self) {
        self.emit_return(None);
    }

    /// Transfer one source reference into a storage home, without retaining an
    /// already-owned object. A raw scalar's materialization becomes the home's
    /// owner. Calls must instead use operation-local call custody, since their
    /// repeated positions and fallible preparation require shared identity and
    /// atomic transfer. A failed mint yields None with MemoryError pending.
    pub(super) fn adopted_storage_word(
        &mut self,
        operand: ValueId,
    ) -> inkwell::values::IntValue<'ctx> {
        self.materialize_dynbox_bits(operand)
    }

    // ── Representation authority ──

    /// Parameters and phis project the same producer facts as frozen linkage.
    pub(super) fn effective_block_arg_type(&self, id: ValueId, declared: &TirType) -> TirType {
        self.repr_facts.effective_value_type(id, declared)
    }

    /// Resolve the specialized `len` runtime function from the operand's
    /// refined TIR type. Container specialization is derived directly from TIR
    /// types, not from SimpleIR-name lookup.
    pub(super) fn container_len_fn(&self, operand_id: ValueId) -> RuntimeFn {
        // The container's exact type is a TIR fact; a boxed word carries none.
        match self.func.value_types.get(&operand_id) {
            Some(TirType::List(_)) => RuntimeFn::LenList,
            Some(TirType::Str) => RuntimeFn::LenStr,
            Some(TirType::Dict(_, _)) => RuntimeFn::LenDict,
            Some(TirType::Tuple(_)) => RuntimeFn::LenTuple,
            Some(TirType::Set(_)) => RuntimeFn::LenSet,
            _ => RuntimeFn::Len,
        }
    }

    // ── Box / Unbox ──

    /// The walker's context and builder for one op: they borrow disjoint
    /// parts of this function's state (the value table, and the builder with
    /// its block parameters and per-function state).
    pub(super) fn walker(
        &mut self,
    ) -> (
        crate::codegen_ssa::function::FunctionCx<'_, LlvmCodegen<'_, 'ctx>>,
        LlvmCodegen<'_, 'ctx>,
    ) {
        let codegen = LlvmCodegen {
            words: LlvmWords {
                context: self.backend.context,
                builder: &self.backend.builder,
            },
            backend: self.backend,
            params: &self.block_params,
            state: &self.state,
        };
        let cx = crate::codegen_ssa::function::FunctionCx {
            func: self.func,
            plan: &self.repr_facts,
            values: &mut self.values,
            diagnostics: &self.diagnostics,
            runtime: &self.backend.runtime_callable_symbols,
            linkage: &self.backend.function_linkage_abis,
            functions: &self.backend.function_table,
            call_sites: &self.call_site_counter,
        };
        (cx, codegen)
    }

    /// Run one walker family method on this function's walker context.
    pub(super) fn walker_op(
        &mut self,
        lower: impl for<'a> FnOnce(
            &mut crate::codegen_ssa::function::FunctionCx<'_, LlvmCodegen<'a, 'ctx>>,
            &mut LlvmCodegen<'a, 'ctx>,
        ),
    ) {
        let (mut cx, mut b) = self.walker();
        lower(&mut cx, &mut b);
    }

    /// `IncRef`/`DecRef` (`FunctionCx::refcount_op`).
    pub(super) fn emit_refcount(
        &mut self,
        op: &TirOp,
        operation: molt_codegen_abi::inline_refcount::RefcountOperation,
    ) {
        let (mut cx, mut b) = self.walker();
        cx.refcount_op(&mut b, op, operation);
    }

    /// Representation boxing of a lowered value (`Operands::boxed_word`). A
    /// full-width integer boxed here may mint a heap owner that nobody
    /// tracks, so this stays private to value materialization: borrowing
    /// consumers go through custody and storing or returning consumers
    /// through `owned_operand_word`.
    fn materialize_dynbox_bits(&self, operand: ValueId) -> inkwell::values::IntValue<'ctx> {
        self.operands().boxed_word(&mut self.codegen(), operand)
    }

    /// One static entry-block word for an operation's out-parameter or owner
    /// slot; allocating where the operation runs would grow the stack on each
    /// loop iteration.
    pub(super) fn build_entry_i64_alloca(&self, name: &str) -> inkwell::values::PointerValue<'ctx> {
        self.codegen().entry_slot(name)
    }

    /// A fixed word range for one operation (list, tuple, class-definition or
    /// unpack transport), in the entry block as `build_entry_i64_alloca`.
    pub(super) fn build_entry_i64_array_alloca(
        &self,
        len: u64,
        name: &str,
    ) -> inkwell::values::PointerValue<'ctx> {
        let len = u32::try_from(len).expect("an operation's word range fits u32");
        self.codegen().entry_slot_array(len, name)
    }

    // ── Terminators ──

    /// The walker's view of this function's control flow.
    fn edges(&self) -> crate::codegen_ssa::control::Edges<'_, LlvmCodegen<'_, 'ctx>> {
        crate::codegen_ssa::control::Edges {
            func: self.func,
            plan: &self.repr_facts,
            values: &self.values,
            blocks: &self.block_map,
        }
    }

    fn record_fatals(&self, diagnostics: Vec<String>) {
        for diagnostic in diagnostics {
            self.record_fatal(diagnostic);
        }
    }

    /// End the current block with a branch from `source` to `target` that
    /// passes `args`.
    pub(super) fn jump_with_args(
        &mut self,
        source: BlockId,
        edge: &str,
        target: BlockId,
        args: &[ValueId],
    ) {
        let mut diagnostics = Vec::new();
        let mut codegen = self.codegen();
        let edges = self.edges();
        let passed = edges.arguments(&mut codegen, source, edge, target, args, &mut diagnostics);
        codegen.jump(self.block_map[&target], &passed);
        self.record_fatals(diagnostics);
    }

    /// End the current block with a branch on `condition`: to `target`,
    /// passing `args`, when it holds, else to `otherwise` with none.
    pub(super) fn branch_with_args_if(
        &mut self,
        condition: inkwell::values::IntValue<'ctx>,
        source: BlockId,
        edge: &str,
        target: BasicBlock<'ctx>,
        target_block: BlockId,
        args: &[ValueId],
        otherwise: BasicBlock<'ctx>,
    ) {
        let mut diagnostics = Vec::new();
        let mut codegen = self.codegen();
        let edges = self.edges();
        let passed = edges.arguments(
            &mut codegen,
            source,
            edge,
            target_block,
            args,
            &mut diagnostics,
        );
        codegen.branch_if(condition, (target, &passed), (otherwise, &[]));
        self.record_fatals(diagnostics);
    }

    pub(super) fn lower_terminator(&mut self, source_block: BlockId, term: &Terminator) {
        let mut diagnostics = Vec::new();
        let lowered = {
            let mut codegen = self.codegen();
            self.edges()
                .terminator(&mut codegen, source_block, term, &mut diagnostics)
        };
        self.record_fatals(diagnostics);
        if lowered {
            return;
        }
        match term {
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                let frame = self.poll_frame_address();
                let mut diagnostics = Vec::new();
                self.edges().dispatch_state(
                    &mut self.codegen(),
                    source_block,
                    frame,
                    cases,
                    (*default, default_args),
                    &mut diagnostics,
                );
                self.record_fatals(diagnostics);
            }
            Terminator::Return { values } => {
                self.emit_return(values.first().copied());
            }
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::Unreachable => unreachable!("the walker lowers {term:?}"),
        }
    }

    // ── Helpers ──

    /// The machine-level type of a lowered value: `I64`, `F64` or `Bool` when
    /// it is held raw, `DynBox` when it is a boxed word. It names a
    /// representation for lowering helpers, never a proven Python type; that
    /// is `TirFunction::value_types`.
    pub(super) fn repr_ty(&self, id: ValueId) -> TirType {
        self.values.repr(id).map_or(
            TirType::DynBox,
            crate::codegen_ssa::HeldCarrier::machine_type,
        )
    }

    /// Resolve a ValueId to its LLVM value.
    ///
    /// If the value was never defined, record a fatal diagnostic. The fallback
    /// value only keeps diagnostic collection moving; checked lowering refuses
    /// to expose the resulting function.
    pub(super) fn resolve(&self, id: ValueId) -> BasicValueEnum<'ctx> {
        if let Some(val) = self.values.value(id) {
            val
        } else {
            self.record_fatal(format!(
                "ValueId %{} was used before being defined during LLVM lowering",
                id.0
            ));
            self.backend.context.i64_type().get_undef().into()
        }
    }

    /// Ensure a value is i64 (for NaN-boxed runtime calls).
    /// If it's already i64, return as-is. Otherwise, cast/extend.
    pub(super) fn ensure_i64(&self, val: BasicValueEnum<'ctx>) -> inkwell::values::IntValue<'ctx> {
        crate::codegen_ssa::scalar::word_of(&mut self.codegen(), val)
    }

    /// This invocation's thread record (invariant J1, `call_anatomy.md` 4.1):
    /// one load from the execution context every compiled function takes,
    /// in the actual LLVM entry, including a CFG trampoline. The execution
    /// lease keeps the record alive for the invocation.
    pub(super) fn thread_record(&mut self) -> inkwell::values::PointerValue<'ctx> {
        self.codegen().thread_record()
    }

    /// The pending-work word the thread record lends, loaded once in the
    /// entry right after the record (the entry builder inserts before the
    /// block's first instruction, which may precede the record).
    pub(super) fn pending_work_ptr(&mut self) -> inkwell::values::PointerValue<'ctx> {
        if let Some(word) = self.pending_work_ptr {
            return word;
        }
        let record = self.thread_record();
        let context = self.backend.context;
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let builder = context.create_builder();
        let defined = record
            .as_instruction()
            .expect("the thread record is an entry-block instruction");
        match defined.get_next_instruction() {
            Some(next) => builder.position_before(&next),
            None => builder
                .position_at_end(defined.get_parent().expect("the thread record has a block")),
        }
        let field = unsafe {
            builder
                .build_gep(
                    context.i8_type(),
                    record,
                    &[context.i64_type().const_int(
                        molt_codegen_abi::THREAD_RECORD_PENDING_WORK_PTR_OFFSET as u64,
                        false,
                    )],
                    "pending_work_field",
                )
                .unwrap()
        };
        let word = builder
            .build_load(context.i64_type(), field, "pending_work_address")
            .unwrap()
            .into_int_value();
        let word = builder
            .build_int_to_ptr(word, ptr_ty, "pending_work_ptr")
            .unwrap();
        self.pending_work_ptr = Some(word);
        word
    }

    /// Read the thread's pending-work word and test it: an i1 that is true
    /// when work waits for a poll. The load is volatile and atomic, so no
    /// LLVM pass merges two reads or hoists one out of a loop.
    pub(super) fn emit_async_work_pending(&mut self) -> inkwell::values::IntValue<'ctx> {
        let word_ptr = self.pending_work_ptr();
        let context = self.backend.context;
        let builder = &self.backend.builder;
        let word = builder
            .build_load(context.i32_type(), word_ptr, "pending_work")
            .unwrap();
        let load = word.as_instruction_value().unwrap();
        load.set_alignment(4).unwrap();
        load.set_atomic_ordering(inkwell::AtomicOrdering::Monotonic)
            .unwrap();
        load.set_volatile(true).unwrap();
        builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                word.into_int_value(),
                context.i32_type().const_zero(),
                "has_async_work",
            )
            .unwrap()
    }

    pub(super) fn emit_async_work_poll(
        &mut self,
        payload: bool,
        cold_location: Option<i64>,
    ) -> BasicValueEnum<'ctx> {
        let has_work = self.emit_async_work_pending();
        let record = self.thread_record();
        let context = self.backend.context;
        let cold = context.append_basic_block(self.llvm_fn, "async_poll_cold");
        let fast = context.append_basic_block(self.llvm_fn, "async_poll_fast");
        let done = context.append_basic_block(self.llvm_fn, "async_poll_done");
        self.backend
            .builder
            .build_conditional_branch(has_work, cold, fast)
            .unwrap();
        self.backend.builder.position_at_end(cold);
        if let Some(location) = cold_location {
            // Serviced work is Python code that can read this frame.
            self.store_frame_location(location);
        }
        let cold_fn = self.ensure_runtime_import(if payload {
            RuntimeFn::AsyncWorkPollAndExceptionLastPending
        } else {
            RuntimeFn::AsyncWorkPollAndExceptionPending
        });
        let cold_call = self
            .backend
            .builder
            .build_execution_call(cold_fn, &[], "serviced_async_work")
            .unwrap();
        cold_call.add_attribute(
            inkwell::attributes::AttributeLoc::Function,
            context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("cold"),
                0,
            ),
        );
        let cold_value = cold_call.try_as_basic_value().unwrap_basic();
        self.backend
            .builder
            .build_unconditional_branch(done)
            .unwrap();
        self.backend.builder.position_at_end(fast);
        let byte = self
            .backend
            .builder
            .build_load(context.i8_type(), record, "exception_pending")
            .unwrap()
            .into_int_value();
        let mut no_exception = None;
        let fast_value = if payload {
            let capture = context.append_basic_block(self.llvm_fn, "async_poll_capture_exception");
            let pending = self
                .backend
                .builder
                .build_int_compare(
                    inkwell::IntPredicate::NE,
                    byte,
                    context.i8_type().const_zero(),
                    "has_pending_exception",
                )
                .unwrap();
            self.backend
                .builder
                .build_conditional_branch(pending, capture, done)
                .unwrap();
            no_exception = Some(
                context
                    .i64_type()
                    .const_int(molt_codegen_abi::QNAN_TAG_NONE_I64 as u64, false),
            );
            self.backend.builder.position_at_end(capture);
            let observer = self.ensure_runtime_import(RuntimeFn::ExceptionLastPending);
            self.backend
                .builder
                .build_execution_call(observer, &[], "pending_exception_payload")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
        } else {
            self.backend
                .builder
                .build_int_z_extend(byte, context.i64_type(), "pending_exception_word")
                .unwrap()
                .into()
        };
        let fast_exit = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_unconditional_branch(done)
            .unwrap();
        self.backend.builder.position_at_end(done);
        let result = self
            .backend
            .builder
            .build_phi(context.i64_type(), "async_poll_result")
            .unwrap();
        result.add_incoming(&[(&cold_value, cold), (&fast_value, fast_exit)]);
        if let Some(none) = no_exception {
            result.add_incoming(&[(&none, fast)]);
        }
        result.as_basic_value()
    }

    /// `molt_exception_pending()` as an i64 0 or 1 (invariant J1). It tests
    /// the thread record's pending byte inline and calls the runtime reader
    /// only when the byte is set. The reader answers 0 at once for a clear
    /// byte, and for a set byte it confirms it, repairing a stale task slot;
    /// the inline test therefore returns exactly what the call returned. The
    /// record costs one entry-block load in any function, less than one call.
    pub(super) fn emit_exception_pending_word(
        &mut self,
        name: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let thread = self.thread_record_word();
        let mut b = self.codegen();
        let result = crate::codegen_ssa::pending::exception_pending_word(&mut b, thread);
        b.set_name(result.into(), &format!("{name}_word"));
        result
    }

    /// The module's declaration of runtime entry `function` (HF-821): its
    /// linker symbol, type and attributes come from the generated record.
    pub(super) fn ensure_runtime_import(&self, function: RuntimeFn) -> FunctionValue<'ctx> {
        self.backend.runtime_import(function)
    }

    pub(super) fn unbox_ptr_bits(
        &self,
        bits: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        self.backend
            .builder
            .build_and(
                bits,
                i64_ty.const_int(nanbox::POINTER_MASK, false),
                "ptr_address",
            )
            .unwrap()
    }
}
