use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use inkwell::values::BasicValue;
use molt_codegen_abi::task_site::TaskConstructorLayout;
use std::collections::BTreeMap;

/// A runtime-call argument: a borrowed operand of the lowered operation, or a
/// raw machine word the runtime ABI fixes (an immediate, address or length).
#[derive(Clone, Copy)]
pub(super) enum RuntimeArg<'ctx> {
    Operand(ValueId),
    Word(inkwell::values::BasicMetadataValueEnum<'ctx>),
}

/// Custody of a runtime call's return word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuntimeResultCustody {
    /// The canonical boxed return (a generated boxed-ABI row or the manifest
    /// return contract): an owned or poll word is bound or released, a
    /// borrowed word is retained only when bound, and a void callee binds None.
    Boxed(RuntimeBoxedReturn),
    /// An immediate or immortal word (None, a boolean) with no owner: bound
    /// as-is when requested and never released.
    Unowned,
    /// The call reports only through the exception state: its word is not
    /// adopted and a requested result is None.
    SideEffect,
}

/// Operation-local custody of borrowed boxed operands.
///
/// A lowered operation opens one custody, before its first failure edge, with
/// every operand it may borrow. Only a raw integer carrier without an
/// inline-safe proof can mint a heap owner (`materialization_mints_owner`).
/// Such an operand gets a static entry-block slot, reset to None when custody
/// opens, so a failure taken before the operand is requested releases nothing
/// and a loop never releases a previous iteration's owner. Operands are
/// materialized lazily, at their first request, and a later request of the same
/// value reuses its word: one operation transports one identity per value. A
/// failed mint and every consumer step routed through
/// `borrowed_operands_continue_if` join one failure block, which skips all
/// later work and keeps the first exception pending. Blocks are numbered only
/// when custody creates one, so an operation with nothing that can fail leaves
/// the CFG unchanged.
pub(super) struct BorrowedOperands<'ctx> {
    label: String,
    suffix: Option<usize>,
    requests: usize,
    owners: BTreeMap<ValueId, inkwell::values::PointerValue<'ctx>>,
    words: BTreeMap<ValueId, inkwell::values::IntValue<'ctx>>,
    adopted_objects: Vec<inkwell::values::IntValue<'ctx>>,
    adopted_callable: Option<inkwell::values::IntValue<'ctx>>,
    abort: Option<BasicBlock<'ctx>>,
}

impl BorrowedOperands<'_> {
    /// Whether a failure edge joins this custody, making its result a rejoined
    /// value rather than the committed word itself.
    pub(super) fn can_fail(&self) -> bool {
        self.abort.is_some()
    }
}

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Match native/WASM invocation ownership: entry can fail before a frame
    /// exists, and every return (including suspension) closes only its own.
    pub(super) fn initialize_owned_execution_frame(&mut self) {
        if self.func.execution_context != crate::ir::ExecutionContextPolicy::Local {
            return;
        }
        assert!(self.owned_frame_entered.is_none());
        let builder = self.backend.context.create_builder();
        let entry = self
            .llvm_fn
            .get_first_basic_block()
            .expect("function entry");
        if let Some(first) = entry.get_first_instruction() {
            builder.position_before(&first);
        } else {
            builder.position_at_end(entry);
        }
        let slot = builder
            .build_alloca(self.backend.context.i64_type(), "owned_frame_base")
            .unwrap();
        builder
            .build_store(slot, self.backend.context.i64_type().const_zero())
            .unwrap();
        self.owned_frame_entered = Some(slot);
    }

    fn exit_owned_execution_frame(&mut self) {
        let Some(slot) = self.owned_frame_entered else {
            return;
        };
        let i64_ty = self.backend.context.i64_type();
        let base = self
            .backend
            .builder
            .build_load(i64_ty, slot, "frame_base")
            .unwrap()
            .into_int_value();
        // Retire the custody before releasing homes can call Python again.
        self.backend
            .builder
            .build_store(slot, i64_ty.const_zero())
            .unwrap();
        // The pop body tests the base itself: a failed entry pops nothing.
        let kind = self.frame_entry_kind();
        self.emit_frame_record_pop(base, kind);
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
        let binding = self.backend.program_slot_binding(kind, owner);
        self.backend
            .builder
            .build_load(
                self.backend.context.i64_type(),
                binding.as_pointer_value(),
                "program_slot",
            )
            .expect("load immutable program-slot binding")
            .into_int_value()
    }

    pub(super) fn none_word(&self) -> inkwell::values::IntValue<'ctx> {
        self.backend
            .context
            .i64_type()
            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
    }

    /// Open custody over every operand the operation may borrow; see
    /// [`BorrowedOperands`]. It must precede the operation's first failure edge.
    pub(super) fn begin_borrowed_operands(
        &mut self,
        operands: &[ValueId],
        label: &str,
    ) -> BorrowedOperands<'ctx> {
        let none = self.none_word();
        let mut owners = BTreeMap::new();
        for &operand in operands {
            if !owners.contains_key(&operand) && self.materialization_mints_owner(operand) {
                let slot = self.build_entry_i64_alloca(&format!("{label}_owner"));
                self.backend.builder.build_store(slot, none).unwrap();
                owners.insert(operand, slot);
            }
        }
        BorrowedOperands {
            label: label.to_owned(),
            suffix: None,
            requests: 0,
            owners,
            words: BTreeMap::new(),
            adopted_objects: Vec::new(),
            adopted_callable: None,
            abort: None,
        }
    }

    /// Open call custody before any materialization can fail. The instruction
    /// already owns one reference per adopted object position; snapshot their
    /// words now so abort cleanup dominates every later allocation failure.
    /// Raw values have no preexisting owner and use the same operation-local
    /// materialization as borrowed operands.
    pub(super) fn begin_call_operands(
        &mut self,
        operands: &[ValueId],
        adopted: &[ValueId],
        callable: Option<ValueId>,
        label: &str,
    ) -> BorrowedOperands<'ctx> {
        let adopted_objects = adopted
            .iter()
            .filter(|&&id| Self::tir_type_is_dynbox_like(&self.repr_ty(id)))
            .map(|&id| self.ensure_i64(self.resolve(id)))
            .collect();
        let adopted_callable = callable
            .filter(|id| Self::tir_type_is_dynbox_like(&self.repr_ty(*id)))
            .map(|id| self.ensure_i64(self.resolve(id)));
        let mut custody = self.begin_borrowed_operands(operands, label);
        custody.adopted_objects = adopted_objects;
        custody.adopted_callable = adopted_callable;
        custody
    }

    fn borrowed_operands_suffix(&mut self, custody: &mut BorrowedOperands<'ctx>) -> usize {
        if let Some(suffix) = custody.suffix {
            return suffix;
        }
        let suffix = self.synthetic_block_counter;
        self.synthetic_block_counter += 1;
        custody.suffix = Some(suffix);
        suffix
    }

    fn borrowed_operands_abort(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
    ) -> BasicBlock<'ctx> {
        if let Some(abort) = custody.abort {
            return abort;
        }
        let suffix = self.borrowed_operands_suffix(custody);
        let abort = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("{}_abort{suffix}", custody.label));
        custody.abort = Some(abort);
        abort
    }

    /// The operand's boxed word, materialized at its first request.
    pub(super) fn borrowed_operand(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
        operand: ValueId,
    ) -> inkwell::values::IntValue<'ctx> {
        let position = custody.requests;
        custody.requests += 1;
        if let Some(&word) = custody.words.get(&operand) {
            return word;
        }
        let word = match custody.owners.get(&operand).copied() {
            Some(slot) => self.mint_borrowed_integer(custody, operand, slot, position),
            None => {
                assert!(
                    !self.materialization_mints_owner(operand),
                    "operand %{} can mint an owner but was not declared when `{}` custody opened",
                    operand.0,
                    custody.label
                );
                self.materialize_dynbox_bits(operand)
            }
        };
        custody.words.insert(operand, word);
        word
    }

    /// Box a raw full-width integer for a borrowing consumer. Only the heap
    /// branch can allocate or fail, and `molt_int_from_i64` returns None exactly
    /// when it raised, so that branch records the owner and joins the failure
    /// block on None. The inline branch needs neither: no exception-state poll
    /// runs on the hot path.
    fn mint_borrowed_integer(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
        operand: ValueId,
        slot: inkwell::values::PointerValue<'ctx>,
        position: usize,
    ) -> inkwell::values::IntValue<'ctx> {
        let raw = self.ensure_i64(self.resolve(operand));
        let context = self.backend.context;
        let fits = nanbox::boxed_word::generate_inline_int_fits(&mut self.words(), raw);
        let suffix = self.borrowed_operands_suffix(custody);
        let inline_bb = context.append_basic_block(self.llvm_fn, "box_int_inline");
        let heap_bb = context.append_basic_block(self.llvm_fn, "box_int_bigint");
        let boxed_bb = context.append_basic_block(
            self.llvm_fn,
            &format!("{}_operand{suffix}_{position}", custody.label),
        );
        self.backend
            .builder
            .build_conditional_branch(fits, inline_bb, heap_bb)
            .unwrap();
        self.backend.builder.position_at_end(inline_bb);
        let inline = nanbox::boxed_word::generate_box_inline_int(&mut self.words(), raw);
        self.backend
            .builder
            .build_unconditional_branch(boxed_bb)
            .unwrap();
        self.backend.builder.position_at_end(heap_bb);
        let heap = crate::codegen_ssa::scalar::box_int_by_runtime(&mut self.codegen(), raw);
        self.backend.builder.build_store(slot, heap).unwrap();
        let failed = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                heap,
                self.none_word(),
                &format!("{}_box_failed", custody.label),
            )
            .unwrap();
        let abort = self.borrowed_operands_abort(custody);
        self.backend
            .builder
            .build_conditional_branch(failed, abort, boxed_bb)
            .unwrap();
        self.backend.builder.position_at_end(boxed_bb);
        let boxed = self
            .backend
            .builder
            .build_phi(context.i64_type(), "boxed_int")
            .unwrap();
        boxed.add_incoming(&[(&inline, inline_bb), (&heap, heap_bb)]);
        boxed.as_basic_value().into_int_value()
    }

    /// Continue past a consumer step only when `ok` holds; otherwise join the
    /// custody's failure block. The continuation is named `{label}_{step}N`.
    pub(super) fn borrowed_operands_continue_if(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
        ok: inkwell::values::IntValue<'ctx>,
        step: &str,
    ) {
        let abort = self.borrowed_operands_abort(custody);
        let suffix = self.borrowed_operands_suffix(custody);
        let next = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("{}_{step}{suffix}", custody.label));
        self.backend
            .builder
            .build_conditional_branch(ok, next, abort)
            .unwrap();
        self.backend.builder.position_at_end(next);
    }

    /// Continue past a consumer step that reports failure only through the
    /// exception state.
    pub(super) fn borrowed_operands_continue_if_clear(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
        step: &str,
    ) {
        let pending = self.emit_exception_pending_word(&format!("{}_pending", custody.label));
        let clear = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                pending,
                self.backend.context.i64_type().const_zero(),
                &format!("{}_no_exception", custody.label),
            )
            .unwrap();
        self.borrowed_operands_continue_if(custody, clear, step);
    }

    /// Rejoin the committed path with the failure block, then release every
    /// initialized owner once, after the consumer has retained what it keeps.
    /// The result is `committed`, or None with the first exception pending.
    /// `on_abort` runs on the failure path only, for consumer-owned cleanup such
    /// as releasing a partly built aggregate or publishing None targets.
    pub(super) fn finish_borrowed_operands(
        &mut self,
        mut custody: BorrowedOperands<'ctx>,
        committed: inkwell::values::IntValue<'ctx>,
        result_name: &str,
        on_abort: impl FnOnce(&mut Self),
    ) -> inkwell::values::IntValue<'ctx> {
        let result = match custody.abort {
            None => committed,
            Some(abort) => {
                let suffix = self.borrowed_operands_suffix(&mut custody);
                let committed_bb = self.backend.builder.get_insert_block().unwrap();
                let merge = self
                    .backend
                    .context
                    .append_basic_block(self.llvm_fn, &format!("{}_merge{suffix}", custody.label));
                self.backend
                    .builder
                    .build_unconditional_branch(merge)
                    .unwrap();
                self.backend.builder.position_at_end(abort);
                on_abort(&mut *self);
                self.release_call_inputs(
                    &self.backend.builder,
                    custody.adopted_callable,
                    &custody.adopted_objects,
                );
                let abort_end = self.backend.builder.get_insert_block().unwrap();
                self.backend
                    .builder
                    .build_unconditional_branch(merge)
                    .unwrap();
                self.backend.builder.position_at_end(merge);
                let none = self.none_word();
                let result = self
                    .backend
                    .builder
                    .build_phi(self.backend.context.i64_type(), result_name)
                    .unwrap();
                result.add_incoming(&[(&committed, committed_bb), (&none, abort_end)]);
                result.as_basic_value().into_int_value()
            }
        };
        self.release_borrowed_owners(&custody);
        result
    }

    /// Release every initialized owner here, in descending value-id order.
    /// `finish_borrowed_operands` does this on the rejoined path; an operation
    /// that leaves the function early (a suspension) calls it before that exit.
    pub(super) fn release_borrowed_owners(&self, custody: &BorrowedOperands<'ctx>) {
        if custody.owners.is_empty() {
            return;
        }
        let i64_ty = self.backend.context.i64_type();
        let release = self
            .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Release);
        for &slot in custody.owners.values().rev() {
            let bits = self
                .backend
                .builder
                .build_load(i64_ty, slot, &format!("{}_owner_bits", custody.label))
                .unwrap();
            release.emit(&self.backend.builder, bits);
        }
    }

    /// Eager custody for a consumer that needs every operand word up front:
    /// fixed constructors and positional runtime calls. Operands are requested
    /// in order; `construct` receives one word per position and the name its
    /// value should carry.
    pub(super) fn with_borrowed_boxed_operands(
        &mut self,
        operands: &[ValueId],
        label: &str,
        construct: impl FnOnce(
            &mut Self,
            &[inkwell::values::IntValue<'ctx>],
            &str,
        ) -> inkwell::values::IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let mut custody = self.begin_borrowed_operands(operands, label);
        let mut words = Vec::with_capacity(operands.len());
        for &operand in operands {
            words.push(self.borrowed_operand(&mut custody, operand));
        }
        let name = if custody.can_fail() {
            format!("{label}_constructed")
        } else {
            format!("{label}_result")
        };
        let constructed = construct(&mut *self, words.as_slice(), name.as_str());
        self.finish_borrowed_operands(custody, constructed, &format!("{label}_result"), |_| {})
            .into()
    }

    /// One runtime call whose object arguments are borrowed operands of the
    /// lowered operation and whose raw words pass unchanged. The value is the
    /// callee's word, or None with the first exception pending when an argument
    /// could not be materialized. With `retain_result`, a borrowed word the
    /// caller keeps acquires its own reference on the committed path, before the
    /// argument owners are released (it may alias one of them).
    pub(super) fn borrowed_runtime_call_value(
        &mut self,
        callee: FunctionValue<'ctx>,
        args: &[RuntimeArg<'ctx>],
        retain_result: bool,
        label: &str,
        call_name: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let operands: Vec<ValueId> = args
            .iter()
            .filter_map(|arg| match *arg {
                RuntimeArg::Operand(operand) => Some(operand),
                RuntimeArg::Word(_) => None,
            })
            .collect();
        let mut custody = self.begin_borrowed_operands(&operands, label);
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(args.len());
        for arg in args {
            call_args.push(match *arg {
                RuntimeArg::Operand(operand) => self.borrowed_operand(&mut custody, operand).into(),
                RuntimeArg::Word(word) => word,
            });
        }
        let returns_word = callee.get_type().get_return_type().is_some();
        let call = self
            .backend
            .builder
            .build_execution_call(
                callee,
                &call_args,
                if returns_word { call_name } else { "" },
            )
            .unwrap();
        let value = match call.try_as_basic_value().basic() {
            Some(word) => word.into_int_value(),
            None => self.none_word(),
        };
        if retain_result && returns_word {
            let retain = self
                .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Retain);
            retain.emit(&self.backend.builder, value.into());
        }
        self.finish_borrowed_operands(custody, value, &format!("{label}_result"), |_| {})
    }

    /// Emit a borrowed runtime call and bind or retire its word by `custody`.
    pub(super) fn emit_borrowed_runtime_call(
        &mut self,
        op: &TirOp,
        callee: FunctionValue<'ctx>,
        args: &[RuntimeArg<'ctx>],
        custody: RuntimeResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let bound = !op.results.is_empty();
        let retain =
            bound && custody == RuntimeResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue);
        let value = self.borrowed_runtime_call_value(callee, args, retain, label, call_name);
        match custody {
            RuntimeResultCustody::Boxed(
                RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue,
            ) => self.bind_owned_runtime_result(op, value.into()),
            RuntimeResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue)
            | RuntimeResultCustody::Unowned => {
                if bound {
                    self.bind_owned_runtime_result(op, value.into());
                }
            }
            RuntimeResultCustody::Boxed(RuntimeBoxedReturn::Void)
            | RuntimeResultCustody::SideEffect => {
                let none: BasicValueEnum<'ctx> = self.none_word().into();
                for &result in &op.results {
                    self.values.set_value(result, none);
                    self.values.set_repr(
                        result,
                        crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
                    );
                }
            }
        }
    }

    /// `emit_borrowed_runtime_call` over every operand of `op`, in order.
    pub(super) fn emit_positional_runtime_call(
        &mut self,
        op: &TirOp,
        callee: FunctionValue<'ctx>,
        custody: RuntimeResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let args: Vec<RuntimeArg<'ctx>> = op
            .operands
            .iter()
            .map(|&operand| RuntimeArg::Operand(operand))
            .collect();
        self.emit_borrowed_runtime_call(op, callee, &args, custody, label, call_name);
    }

    /// Return from this function with no value of its own: void, or None in
    /// its linkage return carrier. The return of a body without a value, and
    /// the early exit whose caller reads the pending exception instead.
    pub(super) fn build_empty_return(&mut self) {
        let linkage_abi = require_function_linkage_abi(self.func, self.backend);
        match linkage_abi.return_type.clone() {
            None => {
                self.exit_owned_execution_frame();
                self.backend.builder.build_return(None).unwrap();
            }
            Some(return_type) => {
                let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                let ret_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(none_bits, false)
                    .into();
                let ret_val = self.coerce_value(
                    ret_val,
                    crate::codegen_ssa::HeldCarrier::Boxed,
                    None,
                    &return_type,
                );
                self.exit_owned_execution_frame();
                self.backend.builder.build_return(Some(&ret_val)).unwrap();
            }
        }
    }

    /// The generated boxed-ABI return custody of a dedicated call whose symbol
    /// has a canonical row; a missing row is generator drift, not a fallback.
    pub(super) fn canonical_boxed_return(
        function: RuntimeFn,
        arity: usize,
    ) -> RuntimeResultCustody {
        let abi = function
            .boxed_abi()
            .filter(|abi| abi.arity() == arity)
            .unwrap_or_else(|| {
                panic!(
                    "{}/{arity} must have a generated boxed ABI row",
                    function.symbol()
                )
            });
        RuntimeResultCustody::Boxed(abi.result)
    }

    /// A +1 owned word for a value the consumer stores or returns rather than
    /// borrows. An object carrier is retained; a scalar carrier is boxed, and a
    /// minted heap integer already is the new owner, so it is not retained
    /// again. A failed mint yields None with MemoryError pending for the
    /// enclosing operation's exception check.
    pub(super) fn owned_operand_word(
        &mut self,
        operand: ValueId,
        _retain_name: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let ty = self.repr_ty(operand);
        let word = self.materialize_dynbox_bits(operand);
        if Self::tir_type_is_dynbox_like(&ty) && !self.repr_facts.boxed_refcount_is_noop(operand) {
            let retain = self
                .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Retain);
            retain.emit(&self.backend.builder, word.into());
        }
        word
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

    /// Commit one reference per adopted position while preserving one boxed
    /// identity per value. Repeated transfers need additional references. If
    /// the consumer also borrows that value, custody keeps its original owner
    /// until the call returns; otherwise the first transfer takes that owner.
    /// Call only after every fallible input preparation, just before entry.
    pub(super) fn surrender_borrowed_owners(
        &self,
        custody: &BorrowedOperands<'ctx>,
        operands: &[ValueId],
        borrowed: &[ValueId],
    ) {
        let none = self.none_word();
        let mut transfers = BTreeMap::<ValueId, usize>::new();
        for &operand in operands {
            if custody.owners.contains_key(&operand) {
                *transfers.entry(operand).or_default() += 1;
            }
        }
        for (operand, count) in transfers {
            let slot = custody.owners[&operand];
            let word = custody.words[&operand];
            let keep_owner = borrowed.contains(&operand);
            let retains = count - usize::from(!keep_owner);
            if retains > 0 {
                let retain = self.ensure_refcount_site(
                    molt_codegen_abi::inline_refcount::RefcountOperation::Retain,
                );
                for _ in 0..retains {
                    retain.emit(&self.backend.builder, word.into());
                }
            }
            if !keep_owner {
                self.backend.builder.build_store(slot, none).unwrap();
            }
        }
    }

    pub(super) fn release_call_inputs(
        &self,
        builder: &inkwell::builder::Builder<'ctx>,
        callable: Option<inkwell::values::IntValue<'ctx>>,
        owners: &[inkwell::values::IntValue<'ctx>],
    ) {
        if owners.is_empty() && callable.is_none() {
            return;
        }
        let (args_ptr, nargs) = self.spill_call_words_with_builder(builder, owners, "call_retired");
        let release = self.ensure_runtime_import(RuntimeFn::CallInputsRelease);
        let callable = callable.unwrap_or_else(|| self.backend.context.i64_type().const_zero());
        builder
            .build_execution_call(
                release,
                &[callable.into(), args_ptr.into(), nargs.into()],
                "",
            )
            .unwrap();
    }

    /// Spill call argument words into this operation's static entry-block
    /// array: `(args_ptr, nargs)` words for an owned runtime entry, which
    /// copies them out before any Python code can run.
    pub(super) fn spill_call_words(
        &self,
        words: &[inkwell::values::IntValue<'ctx>],
        name: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        self.spill_call_words_with_builder(&self.backend.builder, words, name)
    }

    /// Calls and their generated trampolines share the same static argument
    /// transport; the supplied builder owns both stores and the entry alloca.
    fn spill_call_words_with_builder(
        &self,
        builder: &inkwell::builder::Builder<'ctx>,
        words: &[inkwell::values::IntValue<'ctx>],
        name: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let array = self
            .entry_block_builder(builder)
            .build_array_alloca(
                i64_ty,
                i64_ty.const_int(words.len().max(1) as u64, false),
                name,
            )
            .unwrap();
        for (index, &word) in words.iter().enumerate() {
            let slot = unsafe {
                builder
                    .build_gep(
                        i64_ty,
                        array,
                        &[i64_ty.const_int(index as u64, false)],
                        &format!("{name}_{index}"),
                    )
                    .unwrap()
            };
            builder.build_store(slot, word).unwrap();
        }
        let args_ptr = builder
            .build_ptr_to_int(array, i64_ty, &format!("{name}_ptr"))
            .unwrap();
        (args_ptr, i64_ty.const_int(words.len() as u64, false))
    }

    /// The boxed word of a compile-time integer inside the inline payload window
    /// (IC site ids and similar); it owns nothing and can never fail.
    pub(super) fn inline_int_constant(&self, value: i64) -> inkwell::values::IntValue<'ctx> {
        assert!(
            (nanbox::INT_MIN_INLINE..=nanbox::INT_MAX_INLINE).contains(&value),
            "compile-time integer word {value} does not fit the inline payload"
        );
        self.backend
            .context
            .i64_type()
            .const_int(nanbox::box_int_bits(value) as u64, false)
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

    /// RC consumes the lowered physical carrier, never an annotation or a
    /// freshly boxed view. In particular, extracting a full-width raw integer
    /// must not feed its payload bits to the object reference-count ABI.
    pub(super) fn emit_refcount(
        &mut self,
        op: &TirOp,
        operation: molt_codegen_abi::inline_refcount::RefcountOperation,
    ) {
        let operand = op.operands[0];
        let value = self.resolve(operand);
        let ty = self.repr_ty(operand);
        if Self::tir_type_is_dynbox_like(&ty) && !self.repr_facts.carrier_refcount_is_noop(operand)
        {
            let runtime = self.ensure_refcount_site(operation);
            let bits = self.ensure_i64(value);
            runtime.emit(&self.backend.builder, bits.into());
        }
        if let Some(&result) = op.results.first() {
            if operation == molt_codegen_abi::inline_refcount::RefcountOperation::Retain {
                self.values.set_value(result, value);
                self.values
                    .set_repr(result, crate::codegen_ssa::HeldCarrier::of(&ty));
            } else {
                let none = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
                self.values.set_value(result, none.into());
                self.values
                    .set_repr(result, crate::codegen_ssa::HeldCarrier::of(&TirType::None));
            }
        }
    }

    /// Representation boxing of a lowered value. A full-width integer boxed here
    /// may mint a heap owner that nobody tracks, so this stays private to the
    /// authority: borrowing consumers go through operation-local custody
    /// (`borrowed_operand`) and storing or returning consumers through
    /// `owned_operand_word`.
    fn materialize_dynbox_bits(&self, operand: ValueId) -> inkwell::values::IntValue<'ctx> {
        let value = self.resolve(operand);
        let ty = &self.repr_ty(operand);
        if *ty == TirType::I64 && self.repr_facts.is_inline_safe_int(operand) {
            return nanbox::boxed_word::generate_box_inline_int(
                &mut self.words(),
                self.ensure_i64(value),
            );
        }
        crate::codegen_ssa::scalar::box_value(&mut self.codegen(), value, ty)
    }

    /// Ownership authority for materialization: a raw full-width integer can
    /// allocate a heap BigInt, while inline-safe raw integers and already-boxed
    /// values transfer no owner. It is known before materializing, so custody
    /// can initialize its owner slots before the operation's first failure edge.
    fn materialization_mints_owner(&self, operand_id: ValueId) -> bool {
        self.values.repr(operand_id) == Some(crate::codegen_ssa::HeldCarrier::RawI64)
            && !self.repr_facts.is_inline_safe_int(operand_id)
    }

    /// One static entry-block word for an operation's out-parameter or owner
    /// slot; allocating where the operation runs would grow the stack on each
    /// loop iteration.
    pub(super) fn build_entry_i64_alloca(&self, name: &str) -> inkwell::values::PointerValue<'ctx> {
        self.entry_block_builder(&self.backend.builder)
            .build_alloca(self.backend.context.i64_type(), name)
            .unwrap()
    }

    /// A fixed word range for one operation (list, tuple,
    /// class-definition or unpack transport). Every execution of the operation
    /// reuses this static entry-block slot; allocating where the operation runs
    /// would grow the stack on each loop iteration.
    pub(super) fn build_entry_i64_array_alloca(
        &self,
        len: u64,
        name: &str,
    ) -> inkwell::values::PointerValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        self.entry_block_builder(&self.backend.builder)
            .build_array_alloca(i64_ty, i64_ty.const_int(len, false), name)
            .unwrap()
    }

    pub(super) fn entry_block_builder(
        &self,
        position: &inkwell::builder::Builder<'ctx>,
    ) -> inkwell::builder::Builder<'ctx> {
        let builder = self.backend.context.create_builder();
        let current_fn = position
            .get_insert_block()
            .and_then(|bb| bb.get_parent())
            .expect("llvm function missing while allocating an entry-block slot");
        let entry = current_fn
            .get_first_basic_block()
            .expect("llvm function missing entry block");
        if let Some(first_instr) = entry.get_first_instruction() {
            builder.position_before(&first_instr);
        } else {
            builder.position_at_end(entry);
        }
        builder
    }

    pub(super) fn emit_box(&mut self, op: &crate::tir::ops::TirOp) {
        let operand_id = op.operands[0];
        let Some(&result_id) = op.results.first() else {
            if self.materialization_mints_owner(operand_id) {
                // Boxing can allocate even without a result binding. Preserve
                // that failure, then retire only the newly materialized owner.
                let boxed = self.materialize_dynbox_bits(operand_id);
                let release = self.ensure_refcount_site(
                    molt_codegen_abi::inline_refcount::RefcountOperation::Release,
                );
                release.emit(&self.backend.builder, boxed.into());
            }
            return;
        };

        // BoxVal's borrowed operand never donates its owner. Scalar boxing
        // creates the result owner; an already-boxed carrier must retain it.
        let boxed: BasicValueEnum<'ctx> = self
            .owned_operand_word(operand_id, "box_result_retain")
            .into();

        self.values.set_value(result_id, boxed);
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    pub(super) fn emit_unbox(&mut self, op: &crate::tir::ops::TirOp) {
        let Some(&result_id) = op.results.first() else {
            return;
        };
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);

        // Determine target type from attrs or result type hint.
        let target_ty = if let Some(AttrValue::Str(ty_name)) = op.attrs.get("type") {
            match ty_name.as_str() {
                "i64" => TirType::I64,
                "f64" => TirType::F64,
                "bool" => TirType::Bool,
                _ => TirType::DynBox,
            }
        } else {
            // The boxed operand's inner type is a TIR fact; read it from TIR,
            // where a boxed transport cannot have erased it.
            self.values
                .repr(result_id)
                .map(crate::codegen_ssa::HeldCarrier::machine_type)
                .or_else(|| match self.func.value_types.get(&operand_id) {
                    Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
                    _ => None,
                })
                .unwrap_or(TirType::I64)
        };

        let target_ty = self.repr_facts.effective_value_type(result_id, &target_ty);
        let unboxed =
            crate::codegen_ssa::scalar::unbox_value(&mut self.codegen(), operand, &target_ty);
        if Self::tir_type_is_dynbox_like(&target_ty)
            && !self.repr_facts.carrier_refcount_is_noop(result_id)
        {
            let retain = self
                .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Retain);
            retain.emit(&self.backend.builder, unboxed);
        }

        self.values.set_value(result_id, unboxed);
        self.values
            .set_repr(result_id, crate::codegen_ssa::HeldCarrier::of(&target_ty));
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

    /// `value`, held as `source`, converted for a destination of type
    /// `target` at the current position (a return, a direct call's raw
    /// parameter).
    pub(super) fn coerce_value(
        &self,
        value: BasicValueEnum<'ctx>,
        source: crate::codegen_ssa::HeldCarrier,
        source_id: Option<ValueId>,
        target: &TirType,
    ) -> BasicValueEnum<'ctx> {
        let mut codegen = self.codegen();
        crate::codegen_ssa::control::coerce(
            &mut codegen,
            &self.repr_facts,
            value,
            source,
            source_id,
            target,
        )
        .unwrap_or_else(|| {
            self.record_fatal(format!(
                "no conversion of a value held as {source:?} to {target:?}"
            ));
            codegen.placeholder(crate::codegen_ssa::control::carrier_kind(
                crate::codegen_ssa::HeldCarrier::of(target),
            ))
        })
    }

    pub(super) fn held(&self, id: ValueId) -> crate::codegen_ssa::HeldCarrier {
        self.values
            .repr(id)
            .unwrap_or(crate::codegen_ssa::HeldCarrier::Boxed)
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
                // Generator/coroutine `_poll` dispatch. The runtime restores
                // the saved resume state across the suspend boundary, so the
                // dispatch reads it from the frame header (no SSA value):
                // `molt_obj_get_state(self)`. State 0 (initial entry) takes
                // the `default` edge; every saved resume state takes the
                // matching suspend op's resume continuation, whose block
                // parameters the edge's arguments fill.
                let self_bits = self.poll_frame_address();
                let get_state_fn = self.ensure_runtime_import(RuntimeFn::ObjGetState);
                let state_val = self
                    .backend
                    .builder
                    .build_execution_call(get_state_fn, &[self_bits.into()], "state_dispatch_state")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let mut diagnostics = Vec::new();
                {
                    let mut codegen = self.codegen();
                    self.edges().switch(
                        &mut codegen,
                        source_block,
                        state_val,
                        cases,
                        (*default, default_args),
                        "state-dispatch",
                        &mut diagnostics,
                    );
                }
                self.record_fatals(diagnostics);
            }
            Terminator::Return { values } => {
                let linkage_abi = require_function_linkage_abi(self.func, self.backend);
                let Some(&value) = values.first() else {
                    self.build_empty_return();
                    return;
                };
                // A multi-value return returns its first value.
                let return_type = linkage_abi.return_type.as_ref().unwrap_or_else(|| {
                    panic!(
                        "LLVM function `{}` returns a value through a native-void linkage ABI",
                        self.func.name
                    )
                });
                let ret_val = self.coerce_value(
                    self.resolve(value),
                    self.held(value),
                    Some(value),
                    return_type,
                );
                self.exit_owned_execution_frame();
                self.backend.builder.build_return(Some(&ret_val)).unwrap();
            }
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::Unreachable => unreachable!("the walker lowers {term:?}"),
        }
    }

    pub(super) fn tir_type_is_dynbox_like(ty: &TirType) -> bool {
        !matches!(
            ty,
            TirType::I64 | TirType::F64 | TirType::Bool | TirType::Never
        )
    }

    // ── Helpers ──

    /// The truth of `operand` as an i1: one spelling for a `Bool` op and a
    /// conditional branch. A raw bool, int or float is tested inline; a boxed
    /// word reaches here only where the late lowering stage could not expand
    /// it (its shared truthiness expansion tests bool and int words inline).
    pub(super) fn truth_bit(&mut self, operand_id: ValueId) -> inkwell::values::IntValue<'ctx> {
        let operand = self.resolve(operand_id);
        let carrier = self
            .values
            .repr(operand_id)
            .unwrap_or(crate::codegen_ssa::HeldCarrier::Boxed);
        crate::codegen_ssa::scalar::truth(&mut self.codegen(), operand, carrier)
    }

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
        if let Some(record) = self.thread_record {
            return record;
        }
        let builder = self.entry_block_builder(&self.backend.builder);
        let context = self.backend.context;
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let token = self.llvm_fn.get_first_param().unwrap().into_int_value();
        let bits = super::refcount::thread_record_bits(&builder, context, token);
        let record = builder
            .build_int_to_ptr(bits, ptr_ty, "thread_record_ptr")
            .unwrap();
        self.thread_record = Some(record);
        record
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
        let pending_fn = self.ensure_runtime_import(RuntimeFn::ExceptionPending);
        let record = self.thread_record();
        let context = self.backend.context;
        let byte_ptr = unsafe {
            self.backend
                .builder
                .build_gep(
                    context.i8_type(),
                    record,
                    &[context.i64_type().const_int(
                        molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as u64,
                        false,
                    )],
                    &format!("{name}_byte_ptr"),
                )
                .unwrap()
        };
        let byte = self
            .backend
            .builder
            .build_load(context.i8_type(), byte_ptr, &format!("{name}_byte"))
            .unwrap()
            .into_int_value();
        let set = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                byte,
                context.i8_type().const_zero(),
                &format!("{name}_byte_set"),
            )
            .unwrap();
        let confirm = context.append_basic_block(self.llvm_fn, "exception_pending_confirm");
        let join = context.append_basic_block(self.llvm_fn, "exception_pending_join");
        let from = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(set, confirm, join)
            .unwrap();
        self.backend.builder.position_at_end(confirm);
        let confirm_call = self
            .backend
            .builder
            .build_execution_call(pending_fn, &[], name)
            .unwrap();
        confirm_call.add_attribute(
            inkwell::attributes::AttributeLoc::Function,
            context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("cold"),
                0,
            ),
        );
        let confirmed = confirm_call
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let confirm_exit = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_unconditional_branch(join)
            .unwrap();
        self.backend.builder.position_at_end(join);
        let pending = self
            .backend
            .builder
            .build_phi(context.i64_type(), &format!("{name}_word"))
            .unwrap();
        pending.add_incoming(&[
            (&confirmed, confirm_exit),
            (&context.i64_type().const_zero(), from),
        ]);
        pending.as_basic_value().into_int_value()
    }

    /// The module's declaration of runtime entry `function` (HF-821): its
    /// linker symbol, type and attributes come from the generated record.
    pub(super) fn ensure_runtime_import(&self, function: RuntimeFn) -> FunctionValue<'ctx> {
        self.backend
            .runtime_imports
            .get(self.backend.context, &self.backend.module, function)
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

    /// Construct a task whose payload is `payload_operands`, with
    /// `function` as its frame context (a compiled `alloc_task`) or none (an
    /// async call). Each slot owns one reference; the task does every payload
    /// step only once it exists (`molt_codegen_abi::task_site`).
    pub(super) fn emit_task_new_with_payload(
        &mut self,
        poll_addr: inkwell::values::IntValue<'ctx>,
        closure_size: i64,
        frame_bytes: inkwell::values::IntValue<'ctx>,
        layout: TaskConstructorLayout,
        function: Option<ValueId>,
        payload_operands: &[ValueId],
        call_name: &str,
    ) -> BasicValueEnum<'ctx> {
        use molt_codegen_abi::task_site::PayloadCustody;
        layout.validate_closure_size(
            closure_size,
            payload_operands.len(),
            false,
            crate::GENERATOR_CONTROL_BYTES,
        );
        // A value repeated in the payload keeps one identity (`SameAs` its first
        // slot). A full-range raw int is boxed by the task once it exists, so a
        // failed allocation mints nothing; any other scalar boxes by identity.
        let mut first_slot: BTreeMap<ValueId, usize> = BTreeMap::new();
        let mut payload: Vec<(inkwell::values::IntValue<'ctx>, PayloadCustody)> =
            Vec::with_capacity(payload_operands.len());
        for (slot, &operand) in payload_operands.iter().enumerate() {
            if let Some(&first) = first_slot.get(&operand) {
                payload.push((payload[first].0, PayloadCustody::SameAs(first)));
                continue;
            }
            first_slot.insert(operand, slot);
            let ty = self.repr_ty(operand);
            if ty == TirType::I64 && !self.repr_facts.is_inline_safe_int(operand) {
                let raw = self.ensure_i64(self.resolve(operand));
                payload.push((raw, PayloadCustody::RawInt));
                continue;
            }
            let word = self.materialize_dynbox_bits(operand);
            let custody = if Self::tir_type_is_dynbox_like(&ty)
                && !self.repr_facts.boxed_refcount_is_noop(operand)
            {
                PayloadCustody::Borrowed
            } else {
                PayloadCustody::Unowned
            };
            payload.push((word, custody));
        }
        let (source, context) = match function {
            Some(function) => (
                super::task_site::TaskContextSource::Function,
                Some(self.materialize_dynbox_bits(function)),
            ),
            None => (super::task_site::TaskContextSource::NoFrame, None),
        };
        self.emit_task_construction(
            layout,
            poll_addr,
            frame_bytes,
            source,
            context,
            molt_codegen_abi::task_site::TaskPayload::Words(&payload),
            call_name,
        )
        .into()
    }
}
