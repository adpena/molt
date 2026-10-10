use super::*;
use crate::runtime_import_abi::{MOLT_CANCEL_TOKEN_GET_CURRENT, MOLT_TASK_REGISTER_EXECUTION};
use molt_tir::trampolines::{TaskCompletion, TaskConstructorLayout};
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
    fn none_word(&self) -> inkwell::values::IntValue<'ctx> {
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
            .filter(|&&id| {
                Self::tir_type_is_dynbox_like(self.value_types.get(&id).unwrap_or(&TirType::DynBox))
            })
            .map(|&id| self.ensure_i64(self.resolve(id)))
            .collect();
        let adopted_callable = callable
            .filter(|id| {
                Self::tir_type_is_dynbox_like(self.value_types.get(id).unwrap_or(&TirType::DynBox))
            })
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
        self.all_llvm_blocks.push(abort);
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
                let value = self.resolve(operand);
                let ty = self
                    .value_types
                    .get(&operand)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                self.materialize_dynbox_bits(value, &ty)
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
        let fits = inline_int_fits_with_builder(&self.backend.builder, context, raw);
        let suffix = self.borrowed_operands_suffix(custody);
        let inline_bb = context.append_basic_block(self.llvm_fn, "box_int_inline");
        let heap_bb = context.append_basic_block(self.llvm_fn, "box_int_bigint");
        let boxed_bb = context.append_basic_block(
            self.llvm_fn,
            &format!("{}_operand{suffix}_{position}", custody.label),
        );
        self.all_llvm_blocks.extend([inline_bb, heap_bb, boxed_bb]);
        let source = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(fits, inline_bb, heap_bb)
            .unwrap();
        self.record_llvm_edge(source, inline_bb);
        self.record_llvm_edge(source, heap_bb);

        self.backend.builder.position_at_end(inline_bb);
        let inline = inline_int_box_with_builder(&self.backend.builder, context, raw);
        self.backend
            .builder
            .build_unconditional_branch(boxed_bb)
            .unwrap();
        self.record_llvm_edge(inline_bb, boxed_bb);

        self.backend.builder.position_at_end(heap_bb);
        let heap =
            heap_int_box_with_builder(&self.backend.builder, context, &self.backend.module, raw);
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
        self.record_llvm_edge(heap_bb, abort);
        self.record_llvm_edge(heap_bb, boxed_bb);

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
        self.all_llvm_blocks.push(next);
        let source = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(ok, next, abort)
            .unwrap();
        self.record_llvm_edge(source, next);
        self.record_llvm_edge(source, abort);
        self.backend.builder.position_at_end(next);
    }

    /// Continue past a consumer step that reports failure only through the
    /// exception state.
    pub(super) fn borrowed_operands_continue_if_clear(
        &mut self,
        custody: &mut BorrowedOperands<'ctx>,
        step: &str,
    ) {
        let pending_fn = self.ensure_runtime_i64_fn("molt_exception_pending", 0);
        let pending = self
            .backend
            .builder
            .build_call(pending_fn, &[], &format!("{}_pending", custody.label))
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
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
                self.all_llvm_blocks.push(merge);
                self.backend
                    .builder
                    .build_unconditional_branch(merge)
                    .unwrap();
                self.record_llvm_edge(committed_bb, merge);
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
                self.record_llvm_edge(abort_end, merge);
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
        let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        for &slot in custody.owners.values().rev() {
            let bits = self
                .backend
                .builder
                .build_load(i64_ty, slot, &format!("{}_owner_bits", custody.label))
                .unwrap();
            self.backend
                .builder
                .build_call(release, &[bits.into()], "")
                .unwrap();
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
            .build_call(
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
            let retain = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
            self.backend
                .builder
                .build_call(retain, &[value.into()], "")
                .unwrap();
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
                    self.values.insert(result, none);
                    self.value_types.insert(result, TirType::DynBox);
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
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("return must be lowered inside a basic block");
                let ret_val =
                    self.coerce_to_tir_type(ret_val, &TirType::DynBox, &return_type, current_bb);
                let ret_val = self.coerce_to_type(
                    ret_val,
                    lower_type(self.backend.context, &return_type),
                    current_bb,
                );
                self.backend.builder.build_return(Some(&ret_val)).unwrap();
            }
        }
    }

    /// The generated boxed-ABI return custody of a dedicated call whose symbol
    /// has a canonical row; a missing row is generator drift, not a fallback.
    pub(super) fn canonical_boxed_return(symbol: &str, arity: usize) -> RuntimeResultCustody {
        let abi = runtime_boxed_abi(symbol, arity)
            .unwrap_or_else(|| panic!("{symbol}/{arity} must have a generated boxed ABI row"));
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
        retain_name: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let value = self.resolve(operand);
        let ty = self
            .value_types
            .get(&operand)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let word = self.materialize_dynbox_bits(value, &ty);
        if Self::tir_type_is_dynbox_like(&ty) {
            let retain = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
            self.backend
                .builder
                .build_call(retain, &[word.into()], retain_name)
                .unwrap();
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
        let value = self.resolve(operand);
        let ty = self
            .value_types
            .get(&operand)
            .cloned()
            .unwrap_or(TirType::DynBox);
        self.materialize_dynbox_bits(value, &ty)
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
                let retain = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                for _ in 0..retains {
                    self.backend
                        .builder
                        .build_call(retain, &[word.into()], "")
                        .unwrap();
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
        let release = self.ensure_runtime_void_fn("molt_call_inputs_release", 3);
        let callable = callable.unwrap_or_else(|| self.backend.context.i64_type().const_zero());
        builder
            .build_call(
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
        self.backend.context.i64_type().const_int(
            (value as u64 & nanbox::INT_MASK) | nanbox::QNAN | nanbox::TAG_INT,
            false,
        )
    }

    // ── Representation authority ──

    /// Effective semantic carrier type for a block argument (phi).
    ///
    /// The carrier is reconciled with the single `is_inline_safe_int`
    /// representation authority (`repr_by_value`'s `RawI64Safe` view), which is
    /// derived from the value-range proof shared with native/WASM. Two
    /// directions, both keyed on that same authority so `value_types` can never
    /// diverge from the `Repr` the raw-i64 lanes gate on:
    ///
    ///   * **Demotion** `I64 -> DynBox`: a `TirType::I64` phi the plan does NOT
    ///     prove overflow-safe is carried `DynBox` (NaN-boxed). `type_refine`
    ///     assigns `add(I64, I64) -> I64` with no overflow proof, so an unproven
    ///     i64 accumulator must stay boxed across the back-edge instead of
    ///     unboxing a runtime BigInt into a truncating 47-bit payload.
    ///   * **Promotion** `DynBox -> I64`: a `DynBox`-declared phi the plan DOES
    ///     prove overflow-safe is carried as a raw `I64`. This is the masked
    ///     back-edge accumulator (`s = (s << 1) & MASK`): the value-range phi
    ///     narrowing proves `s` fits the inline window, so `is_inline_safe_int`
    ///     mints `RawI64Safe` for it — but `type_refine` (which runs without that
    ///     value-range fact) left the phi `DynBox`. Without this promotion the
    ///     phi carries boxed, so the in-loop `<<`/`&` see a `DynBox` operand and
    ///     bail to the boxed `molt_lshift`/`molt_bit_and` runtime even though the
    ///     raw lane was proven legal — defeating the whole narrowing. The phi
    ///     incoming edges are reconciled by `coerce_to_tir_type`, which unboxes a
    ///     boxed incoming (`molt_int_from_i64` / a boxed back-edge value) into the
    ///     raw i64 the I64 phi slot expects. The promotion is sound because
    ///     `is_inline_safe_int` is granted ONLY for values a value-range proof
    ///     places entirely within the inline-int47 window (so a heap BigInt can
    ///     never reach the raw slot); it is restricted to a `DynBox` declared
    ///     type so a non-integer carrier (`Str`/`F64`/container) is never
    ///     reinterpreted as i64.
    pub(super) fn effective_block_arg_type(&self, id: ValueId, declared: &TirType) -> TirType {
        let inline_safe = self.repr_facts.is_inline_safe_int(id);
        match declared {
            TirType::I64 if !inline_safe => TirType::DynBox,
            TirType::DynBox if inline_safe => TirType::I64,
            _ => declared.clone(),
        }
    }

    /// Resolve the specialized `len` runtime function from the operand's
    /// refined TIR type. Container specialization is derived directly from TIR
    /// types, not from SimpleIR-name lookup.
    pub(super) fn container_len_fn(&self, operand_id: ValueId) -> &'static str {
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        match operand_ty {
            TirType::List(_) => "molt_len_list",
            TirType::Str => "molt_len_str",
            TirType::Dict(_, _) => "molt_len_dict",
            TirType::Tuple(_) => "molt_len_tuple",
            TirType::Set(_) => "molt_len_set",
            _ => "molt_len",
        }
    }

    // ── Box / Unbox ──

    /// RC consumes the lowered physical carrier, never an annotation or a
    /// freshly boxed view. In particular, extracting a full-width raw integer
    /// must not feed its payload bits to the object reference-count ABI.
    pub(super) fn emit_refcount(&mut self, op: &TirOp, signature: RuntimeImportSignature) {
        let operand = op.operands[0];
        let value = self.resolve(operand);
        let ty = self
            .value_types
            .get(&operand)
            .cloned()
            .unwrap_or(TirType::DynBox);
        if Self::tir_type_is_dynbox_like(&ty) {
            let runtime = self.ensure_runtime_import(signature);
            let bits = self.ensure_i64(value);
            self.backend
                .builder
                .build_call(runtime, &[bits.into()], "")
                .unwrap();
        }
        if let Some(&result) = op.results.first() {
            self.values.insert(result, value);
            self.value_types.insert(result, ty);
        }
    }

    /// Branchless overflow-safe integer box: a single `molt_int_from_i64` call
    /// that yields one SSA value and never alters control flow. Used where the
    /// boxed value must be a single value in a fixed block (phi-incoming
    /// materialization, function-return coercion). `molt_int_from_i64` returns
    /// the inline NaN-box for values that fit the 47-bit payload and a heap
    /// BigInt otherwise, matching the shared builder-parametric materializer.
    pub(super) fn box_i64_branchless(
        &self,
        raw: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let from_i64_fn = self.ensure_runtime_i64_fn("molt_int_from_i64", 1);
        self.backend
            .builder
            .build_call(from_i64_fn, &[raw.into()], "molt_int_from_i64")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    /// Representation boxing of a lowered value. A full-width integer boxed here
    /// may mint a heap owner that nobody tracks, so this stays private to the
    /// authority: borrowing consumers go through operation-local custody
    /// (`borrowed_operand`) and storing or returning consumers through
    /// `owned_operand_word`.
    fn materialize_dynbox_bits(
        &self,
        operand: BasicValueEnum<'ctx>,
        operand_ty: &TirType,
    ) -> inkwell::values::IntValue<'ctx> {
        materialize_dynbox_bits_with_builder(
            &self.backend.builder,
            self.backend.context,
            &self.backend.module,
            self.llvm_fn,
            operand,
            operand_ty,
        )
    }

    /// Ownership authority for materialization: a raw full-width integer can
    /// allocate a heap BigInt, while inline-safe raw integers and already-boxed
    /// values transfer no owner. It is known before materializing, so custody
    /// can initialize its owner slots before the operation's first failure edge.
    fn materialization_mints_owner(&self, operand_id: ValueId) -> bool {
        self.value_types.get(&operand_id) == Some(&TirType::I64)
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

    /// A fixed word range for one operation (list, tuple, dataclass,
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

    fn entry_block_builder(
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
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let Some(&result_id) = op.results.first() else {
            if operand_ty == TirType::I64 {
                // Boxing can allocate even without a result binding. Preserve
                // that failure, then retire only the newly materialized owner.
                let boxed = self.materialize_dynbox_bits(operand, &operand_ty);
                let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
                self.backend
                    .builder
                    .build_call(release, &[boxed.into()], "")
                    .unwrap();
            }
            return;
        };

        // BoxVal's borrowed operand never donates its owner. Scalar boxing
        // creates the result owner; an already-boxed carrier must retain it.
        let boxed: BasicValueEnum<'ctx> = self
            .owned_operand_word(operand_id, "box_result_retain")
            .into();

        self.values.insert(result_id, boxed);
        self.value_types.insert(result_id, TirType::DynBox);
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
            self.value_types
                .get(&result_id)
                .cloned()
                .or_else(|| match self.value_types.get(&operand_id) {
                    Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
                    _ => None,
                })
                .unwrap_or(TirType::I64)
        };

        let unboxed = self.unbox_from_dynbox(operand, &target_ty);
        if Self::tir_type_is_dynbox_like(&target_ty) {
            let retain = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
            self.backend
                .builder
                .build_call(retain, &[unboxed.into()], "unbox_result_retain")
                .unwrap();
        }

        self.values.insert(result_id, unboxed);
        self.value_types.insert(result_id, target_ty);
    }

    // ── Terminators ──

    pub(super) fn lower_terminator(&mut self, source_block: BlockId, term: &Terminator) {
        match term {
            Terminator::Branch { target, args } => {
                let target_bb = self.block_map[target];
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(source_block, current_bb, *target, "branch", args);
                self.record_llvm_edge(current_bb, target_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(target_bb)
                    .unwrap();
            }
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let cond_val = self.resolve(*cond);
                let cond_ty = self
                    .value_types
                    .get(cond)
                    .cloned()
                    .unwrap_or(TirType::DynBox);

                // Convert condition to i1.
                let cond_i1 = match &cond_ty {
                    TirType::Bool => cond_val.into_int_value(),
                    TirType::I64 => self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            cond_val.into_int_value(),
                            self.backend.context.i64_type().const_int(0, false),
                            "cond_i1",
                        )
                        .unwrap(),
                    _ => {
                        // DynBox: call molt_is_truthy
                        let cond_i64 = self.ensure_i64(cond_val);
                        let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                        let result = self
                            .backend
                            .builder
                            .build_call(truthy_fn, &[cond_i64.into()], "truthy")
                            .unwrap()
                            .try_as_basic_value()
                            .unwrap_basic();
                        self.backend
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::NE,
                                result.into_int_value(),
                                self.backend.context.i64_type().const_int(0, false),
                                "cond_i1",
                            )
                            .unwrap()
                    }
                };

                let then_bb = self.block_map[then_block];
                let else_bb = self.block_map[else_block];

                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *then_block,
                    "then-edge",
                    then_args,
                );
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *else_block,
                    "else-edge",
                    else_args,
                );
                self.record_llvm_edge(current_bb, then_bb);
                self.record_llvm_edge(current_bb, else_bb);

                let branch_inst = self
                    .backend
                    .builder
                    .build_conditional_branch(cond_i1, then_bb, else_bb)
                    .unwrap();

                // Attach PGO branch weight metadata when profile data is available.
                // The weights vector is consumed sequentially: each CondBranch
                // pops two values (true_weight, false_weight).
                if let Some(ref weights) = self.pgo_branch_weights {
                    let idx = self.pgo_weight_index;
                    if idx + 1 < weights.len() {
                        let true_weight = weights[idx];
                        let false_weight = weights[idx + 1];
                        self.pgo_weight_index = idx + 2;

                        // Build !prof metadata: !{!"branch_weights", i32 T, i32 F}
                        // inkwell exposes `set_metadata(MetadataValue, kind_id)` on
                        // InstructionValue, and `metadata_node` / `metadata_string`
                        // on Context. The "prof" metadata kind ID is obtained via
                        // `context.get_kind_id("prof")`.
                        //
                        // However, inkwell's `metadata_node` API expects
                        // `&[BasicMetadataValueEnum]` which cannot hold a
                        // `MetadataValue` (the "branch_weights" string). The LLVM C
                        // API call `LLVMMDNode` with mixed operand types is not
                        // exposed through inkwell's safe wrapper. To attach !prof
                        // metadata correctly, a raw `llvm-sys` call is needed:
                        //
                        //   use llvm_sys::core::*;
                        //   let prof_kind = LLVMGetMDKindIDInContext(ctx, "prof", 4);
                        //   let bw_str = LLVMMDStringInContext(ctx, "branch_weights", 14);
                        //   let t_val = LLVMConstInt(LLVMInt32TypeInContext(ctx), true_weight, 0);
                        //   let f_val = LLVMConstInt(LLVMInt32TypeInContext(ctx), false_weight, 0);
                        //   let md_ops = [bw_str, t_val, f_val];
                        //   let md_node = LLVMMDNodeInContext(ctx, md_ops.as_ptr(), 3);
                        //   LLVMSetMetadata(branch_inst, prof_kind, md_node);
                        //
                        // This is deferred until we add `llvm-sys` as a direct
                        // dependency (currently accessed indirectly via inkwell).
                        // The PGO data is loaded and indexed correctly; only the
                        // final metadata attachment step requires the raw API.
                        let _ = (branch_inst, true_weight, false_weight);
                    }
                }
            }
            Terminator::Switch {
                value,
                cases,
                default,
                default_args,
            } => {
                let switch_val = self.resolve(*value);
                let switch_int = self.ensure_i64(switch_val);
                let default_bb = self.block_map[default];

                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *default,
                    "switch-default",
                    default_args,
                );
                self.record_llvm_edge(current_bb, default_bb);

                let mut switch_cases: Vec<_> = Vec::with_capacity(cases.len());
                for (case_val, target, args) in cases {
                    let case_const = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(*case_val as u64, *case_val < 0);
                    let target_bb = self.block_map[target];
                    self.record_branch_args(source_block, current_bb, *target, "switch-case", args);
                    self.record_llvm_edge(current_bb, target_bb);
                    switch_cases.push((case_const, target_bb));
                }

                self.backend
                    .builder
                    .build_switch(switch_int, default_bb, &switch_cases)
                    .unwrap();
            }
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                // Generator/coroutine `_poll` dispatch.  The saved resume state
                // is restored by the runtime across the suspend boundary, so the
                // dispatch value is read from the frame header here (not an SSA
                // value): `molt_obj_get_state(self)`.  State 0 (initial entry)
                // takes the `default` edge; every saved resume state dispatches
                // to the matching suspend op's REAL resume continuation block.
                //
                // This is the first-class replacement for the old synthetic
                // `state_resume_*` block machinery: the switch targets are the
                // real TIR blocks the main lowering loop emits (so their phis are
                // the phis the SSA pass placed), and `record_branch_args` supplies
                // each dispatch edge's incomings, which `finalize_phis` fills.
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let get_state_fn = self.ensure_runtime_i64_fn("molt_obj_get_state", 1);
                let state_val = self
                    .backend
                    .builder
                    .build_call(get_state_fn, &[self_bits.into()], "state_dispatch_state")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();

                let default_bb = self.block_map[default];
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *default,
                    "state-dispatch-default",
                    default_args,
                );
                self.record_llvm_edge(current_bb, default_bb);

                let mut switch_cases: Vec<_> = Vec::with_capacity(cases.len());
                for (state_id, target, args) in cases {
                    let case_const = i64_ty.const_int(*state_id as u64, *state_id < 0);
                    let target_bb = self.block_map[target];
                    self.record_branch_args(
                        source_block,
                        current_bb,
                        *target,
                        "state-dispatch-case",
                        args,
                    );
                    self.record_llvm_edge(current_bb, target_bb);
                    switch_cases.push((case_const, target_bb));
                }

                self.backend
                    .builder
                    .build_switch(state_val, default_bb, &switch_cases)
                    .unwrap();
            }
            Terminator::Return { values } => {
                let linkage_abi = require_function_linkage_abi(self.func, self.backend);
                let linkage_return_type = linkage_abi.return_type.as_ref();
                if values.is_empty() {
                    self.build_empty_return();
                } else if values.len() == 1 {
                    let val = self.resolve(values[0]);
                    let return_type = linkage_return_type.unwrap_or_else(|| {
                        panic!(
                            "LLVM function `{}` returns a value through a native-void linkage ABI",
                            self.func.name
                        )
                    });
                    let ret_ty = lower_type(self.backend.context, return_type);
                    let val_ty = self
                        .value_types
                        .get(&values[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    let current_bb = self
                        .backend
                        .builder
                        .get_insert_block()
                        .expect("return must be lowered inside a basic block");
                    let ret_val = self.coerce_to_tir_type(val, &val_ty, return_type, current_bb);
                    let ret_val = self.coerce_to_type(ret_val, ret_ty, current_bb);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                } else {
                    // Multi-value return: pack into struct.
                    // For now, just return the first value.
                    let val = self.resolve(values[0]);
                    let return_type = linkage_return_type.unwrap_or_else(|| {
                        panic!(
                            "LLVM function `{}` returns values through a native-void linkage ABI",
                            self.func.name
                        )
                    });
                    let ret_ty = lower_type(self.backend.context, return_type);
                    let val_ty = self
                        .value_types
                        .get(&values[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    let current_bb = self
                        .backend
                        .builder
                        .get_insert_block()
                        .expect("return must be lowered inside a basic block");
                    let ret_val = self.coerce_to_tir_type(val, &val_ty, return_type, current_bb);
                    let ret_val = self.coerce_to_type(ret_val, ret_ty, current_bb);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                }
            }
            Terminator::Unreachable => {
                self.backend.builder.build_unreachable().unwrap();
            }
        }
    }

    // ── Phi node wiring ──

    /// Record that an actually emitted branch passes `args` to `target`.
    pub(super) fn record_branch_args(
        &mut self,
        source_block: BlockId,
        source_bb: BasicBlock<'ctx>,
        target: BlockId,
        edge_name: &'static str,
        args: &[ValueId],
    ) {
        self.phi_edges.push(PhiIncomingEdge {
            source_block,
            source_bb,
            target,
            edge_name,
            args: args.to_vec(),
        });
    }

    /// After all blocks are lowered, wire up phi node incoming values.
    /// Values are coerced to match the phi node's type when needed (e.g., an
    /// i1 bool flowing into an i64 phi is zero-extended).
    ///
    /// This method also handles:
    /// - Mid-block branches from CheckException (not visible in TIR terminators)
    /// - Missing predecessors: if a phi node doesn't have an incoming value for
    ///   some predecessor, record a fatal lowering diagnostic. The compile path
    ///   must not turn malformed control/data flow into verified-but-wrong IR.
    pub(super) fn finalize_phis(&mut self) {
        // Collect phi info first to avoid borrow conflicts.
        let phi_info: Vec<_> = self
            .pending_phis
            .iter()
            .map(|(bid, idx, phi)| (*bid, *idx, phi.as_basic_value().get_type(), *phi))
            .collect();

        for (block_id, arg_index, phi_ty, phi) in &phi_info {
            let block = self.func.blocks.get(block_id).unwrap();
            let phi_tir_ty = block
                .args
                .get(*arg_index)
                .map(|arg| self.effective_block_arg_type(arg.id, &arg.ty))
                .unwrap_or(TirType::DynBox);

            // 1. Wire up predecessors from branches that were actually emitted
            //    into the LLVM CFG. This intentionally excludes dead TIR blocks
            //    whose terminators were not lowered and whose LLVM blocks were
            //    terminated with `unreachable`.
            let phi_edges = self.phi_edges.clone();
            for edge in phi_edges.iter().filter(|edge| edge.target == *block_id) {
                if *arg_index >= edge.args.len() {
                    self.record_fatal(format!(
                        "predecessor block {:?} {} branches to {:?} with {} argument(s), but phi argument index {} is required",
                        edge.source_block,
                        edge.edge_name,
                        block_id,
                        edge.args.len(),
                        arg_index
                    ));
                    continue;
                }
                let val_id = edge.args[*arg_index];
                let Some(val) = self.values.get(&val_id).copied() else {
                    self.record_fatal(format!(
                        "predecessor block {:?} passes undefined ValueId %{} to phi argument {} in block {:?}",
                        edge.source_block, val_id.0, arg_index, block_id
                    ));
                    continue;
                };
                let source_tir_ty = self
                    .value_types
                    .get(&val_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let coerced =
                    self.coerce_to_tir_type(val, &source_tir_ty, &phi_tir_ty, edge.source_bb);
                let coerced = self.coerce_to_type(coerced, *phi_ty, edge.source_bb);
                phi.add_incoming(&[(&coerced, edge.source_bb)]);
            }

            // 2. If the original TIR entry block was demoted behind a
            // trampoline, wire the function parameters in through that
            // synthetic predecessor. Entry args beyond the function arity are
            // true phi values and intentionally start as undef on the initial
            // call edge.
            if *block_id == self.func.entry_block
                && let Some(trampoline_bb) = self.entry_trampoline_bb
            {
                if let Some(param) = self.llvm_fn.get_nth_param(*arg_index as u32) {
                    let source_tir_ty = require_function_linkage_abi(self.func, self.backend)
                        .param_types
                        .get(*arg_index)
                        .cloned()
                        .unwrap_or_else(|| {
                            panic!(
                                "LLVM definition `{}` linkage ABI omitted resume parameter {}",
                                self.func.name, arg_index
                            )
                        });
                    let coerced =
                        self.coerce_to_tir_type(param, &source_tir_ty, &phi_tir_ty, trampoline_bb);
                    let coerced = self.coerce_to_type(coerced, *phi_ty, trampoline_bb);
                    phi.add_incoming(&[(&coerced, trampoline_bb)]);
                } else {
                    let undef = self.get_undef_for_type(*phi_ty);
                    phi.add_incoming(&[(&undef, trampoline_bb)]);
                }
            }
        }

        // 3. Final safety net: scan all phi nodes for missing predecessors.
        //    If any LLVM predecessor block is missing from a phi's incoming
        //    list, add an undef entry. This catches edge cases from synthetic
        //    blocks, trampoline blocks, and any other control flow that the
        //    TIR-level analysis doesn't fully capture.
        self.patch_incomplete_phis();
    }

    /// For each phi node in the function, check that every LLVM predecessor
    /// of the phi's parent block has an incoming entry. Missing entries are
    /// fatal lowering diagnostics.
    ///
    /// Uses the `llvm_pred_map` built during lowering to determine predecessors
    /// (no need to scan LLVM IR or use llvm-sys directly).
    pub(super) fn patch_incomplete_phis(&self) {
        use inkwell::values::InstructionOpcode;
        use std::collections::HashSet;

        let mut bb = self.llvm_fn.get_first_basic_block();
        while let Some(current_bb) = bb {
            // Look up predecessors from our map.
            if let Some(preds) = self.llvm_pred_map.get(&current_bb) {
                // Walk instructions looking for phi nodes (they're always at the top).
                let mut inst = current_bb.get_first_instruction();
                while let Some(i) = inst {
                    if i.get_opcode() != InstructionOpcode::Phi {
                        break; // phi nodes are always at the top of the block
                    }
                    // Use inkwell's PhiValue to inspect incoming blocks.
                    use inkwell::values::AsValueRef;
                    let phi: PhiValue<'ctx> = unsafe { PhiValue::new(i.as_value_ref()) };
                    let incoming_count = phi.count_incoming();
                    let mut covered: HashSet<BasicBlock<'ctx>> = HashSet::new();
                    for idx in 0..incoming_count {
                        if let Some((_, incoming_bb)) = phi.get_incoming(idx) {
                            covered.insert(incoming_bb);
                        }
                    }
                    for pred_bb in preds {
                        if !covered.contains(pred_bb) {
                            self.record_fatal(format!(
                                "phi in LLVM block {:?} is missing incoming value from predecessor {:?}",
                                current_bb, pred_bb
                            ));
                        }
                    }
                    inst = i.get_next_instruction();
                }
            }
            bb = current_bb.get_next_basic_block();
        }
    }

    /// Return an `undef` value of the given LLVM type.
    pub(super) fn get_undef_for_type(
        &self,
        ty: inkwell::types::BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        match ty {
            inkwell::types::BasicTypeEnum::IntType(it) => it.get_undef().into(),
            inkwell::types::BasicTypeEnum::FloatType(ft) => ft.get_undef().into(),
            inkwell::types::BasicTypeEnum::PointerType(pt) => pt.get_undef().into(),
            inkwell::types::BasicTypeEnum::ArrayType(at) => at.get_undef().into(),
            inkwell::types::BasicTypeEnum::StructType(st) => st.get_undef().into(),
            inkwell::types::BasicTypeEnum::VectorType(vt) => vt.get_undef().into(),
            inkwell::types::BasicTypeEnum::ScalableVectorType(svt) => svt.get_undef().into(),
        }
    }

    /// Coerce a value to a target LLVM type.  Inserts conversion instructions
    /// at the end of `in_block` (before the terminator) when the types differ.
    pub(super) fn coerce_to_type(
        &self,
        val: BasicValueEnum<'ctx>,
        target_ty: inkwell::types::BasicTypeEnum<'ctx>,
        in_block: BasicBlock<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let val_ty = val.get_type();
        if val_ty == target_ty {
            return val;
        }
        // Save current position and switch to the predecessor block.
        let saved_block = self.backend.builder.get_insert_block();
        // Insert BEFORE the terminator of in_block.
        if let Some(term) = in_block.get_terminator() {
            self.backend.builder.position_before(&term);
        } else {
            self.backend.builder.position_at_end(in_block);
        }
        let result = match (val, target_ty) {
            // i1 (bool) -> i64: zero-extend
            (BasicValueEnum::IntValue(iv), inkwell::types::BasicTypeEnum::IntType(target_int))
                if iv.get_type().get_bit_width() < target_int.get_bit_width() =>
            {
                self.backend
                    .builder
                    .build_int_z_extend(iv, target_int, "phi_zext")
                    .unwrap()
                    .into()
            }
            // i64 -> i1: truncate
            (BasicValueEnum::IntValue(iv), inkwell::types::BasicTypeEnum::IntType(target_int))
                if iv.get_type().get_bit_width() > target_int.get_bit_width() =>
            {
                self.backend
                    .builder
                    .build_int_truncate(iv, target_int, "phi_trunc")
                    .unwrap()
                    .into()
            }
            // f64 -> i64: bitcast
            (
                BasicValueEnum::FloatValue(fv),
                inkwell::types::BasicTypeEnum::IntType(target_int),
            ) => self
                .backend
                .builder
                .build_bit_cast(fv, target_int, "phi_f2i")
                .unwrap(),
            // i64 -> f64: bitcast
            (
                BasicValueEnum::IntValue(iv),
                inkwell::types::BasicTypeEnum::FloatType(target_float),
            ) => self
                .backend
                .builder
                .build_bit_cast(iv, target_float, "phi_i2f")
                .unwrap(),
            (
                BasicValueEnum::IntValue(iv),
                inkwell::types::BasicTypeEnum::PointerType(target_ptr),
            ) => self
                .backend
                .builder
                .build_int_to_ptr(iv, target_ptr, "phi_i2p")
                .unwrap()
                .into(),
            (
                BasicValueEnum::PointerValue(pv),
                inkwell::types::BasicTypeEnum::IntType(target_int),
            ) => self
                .backend
                .builder
                .build_ptr_to_int(pv, target_int, "phi_p2i")
                .unwrap()
                .into(),
            (
                BasicValueEnum::PointerValue(pv),
                inkwell::types::BasicTypeEnum::PointerType(target_ptr),
            ) => self
                .backend
                .builder
                .build_pointer_cast(pv, target_ptr, "phi_p2p")
                .unwrap()
                .into(),
            _ => {
                self.record_fatal(format!(
                    "unsupported LLVM phi coercion from {:?} to {:?} in block {:?}",
                    val_ty, target_ty, in_block
                ));
                self.get_undef_for_type(target_ty)
            }
        };
        // Restore builder position.
        if let Some(bb) = saved_block {
            self.backend.builder.position_at_end(bb);
        }
        result
    }

    pub(super) fn tir_type_is_dynbox_like(ty: &TirType) -> bool {
        !matches!(
            ty,
            TirType::I64 | TirType::F64 | TirType::Bool | TirType::Never
        )
    }

    pub(super) fn unbox_from_dynbox(
        &self,
        operand: BasicValueEnum<'ctx>,
        target_ty: &TirType,
    ) -> BasicValueEnum<'ctx> {
        let raw = self.ensure_i64(operand);
        let i64_ty = self.backend.context.i64_type();
        let f64_ty = self.backend.context.f64_type();
        match target_ty {
            TirType::I64 => unbox_i64_with_builder(
                &self.backend.builder,
                self.backend.context,
                &self.backend.module,
                raw,
            )
            .into(),
            TirType::F64 => self
                .backend
                .builder
                .build_bit_cast(raw, f64_ty, "unbox_f64")
                .unwrap(),
            TirType::Bool => {
                let bit = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(1, false), "bool_payload")
                    .unwrap();
                self.backend
                    .builder
                    .build_int_truncate(bit, self.backend.context.bool_type(), "unbox_bool")
                    .unwrap()
                    .into()
            }
            _ => operand,
        }
    }

    pub(super) fn coerce_to_tir_type(
        &self,
        val: BasicValueEnum<'ctx>,
        source_tir_ty: &TirType,
        target_tir_ty: &TirType,
        in_block: BasicBlock<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        if source_tir_ty == target_tir_ty {
            return val;
        }

        let saved_block = self.backend.builder.get_insert_block();
        if let Some(term) = in_block.get_terminator() {
            self.backend.builder.position_before(&term);
        } else {
            self.backend.builder.position_at_end(in_block);
        }

        let result = if Self::tir_type_is_dynbox_like(target_tir_ty)
            && !Self::tir_type_is_dynbox_like(source_tir_ty)
        {
            // `coerce_to_tir_type` materializes a value at a fixed position —
            // either the current block (return) or, for phi incoming edges, the
            // END of a predecessor block that already has a terminator. Both
            // restore the builder afterwards and (for phi edges) require the
            // result to be a single SSA value defined in `in_block`. The
            // overflow-safe integer box that adds a fits-inline branch would
            // split `in_block`, leaving the boxed value in a new merge block
            // that does not dominate the phi user. We therefore box integers
            // here with the branchless runtime call, which yields one SSA value
            // and never alters control flow. (`molt_int_from_i64` returns the
            // inline box for small values and a heap BigInt otherwise — the
            // same value the branch form produces.)
            if matches!(source_tir_ty, TirType::I64) {
                let raw = self.ensure_i64(val);
                self.box_i64_branchless(raw).into()
            } else {
                self.materialize_dynbox_bits(val, source_tir_ty).into()
            }
        } else if !Self::tir_type_is_dynbox_like(target_tir_ty)
            && Self::tir_type_is_dynbox_like(source_tir_ty)
        {
            self.unbox_from_dynbox(val, target_tir_ty)
        } else {
            val
        };

        if let Some(bb) = saved_block {
            self.backend.builder.position_at_end(bb);
        }
        result
    }

    // ── Helpers ──

    /// Resolve a ValueId to its LLVM value.
    ///
    /// If the value was never defined, record a fatal diagnostic. The fallback
    /// value only keeps diagnostic collection moving; checked lowering refuses
    /// to expose the resulting function.
    pub(super) fn resolve(&self, id: ValueId) -> BasicValueEnum<'ctx> {
        if let Some(val) = self.values.get(&id) {
            *val
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
        let i64_ty = self.backend.context.i64_type();
        match val {
            BasicValueEnum::IntValue(iv) => {
                if iv.get_type().get_bit_width() == 64 {
                    iv
                } else if iv.get_type().get_bit_width() < 64 {
                    self.backend
                        .builder
                        .build_int_z_extend(iv, i64_ty, "zext_i64")
                        .unwrap()
                } else {
                    self.backend
                        .builder
                        .build_int_truncate(iv, i64_ty, "trunc_i64")
                        .unwrap()
                }
            }
            BasicValueEnum::FloatValue(fv) => self
                .backend
                .builder
                .build_bit_cast(fv, i64_ty, "f2i")
                .unwrap()
                .into_int_value(),
            BasicValueEnum::PointerValue(pv) => self
                .backend
                .builder
                .build_ptr_to_int(pv, i64_ty, "ptr2i")
                .unwrap(),
            _ => panic!("Cannot convert {:?} to i64", val),
        }
    }

    pub(super) fn ensure_runtime_decl(
        &self,
        name: &str,
        fn_ty: inkwell::types::FunctionType<'ctx>,
        param_count: usize,
        return_abi: RuntimeReturnAbi,
    ) -> FunctionValue<'ctx> {
        if let Some(func) = self.backend.module.get_function(name) {
            return require_llvm_function_type(name, func, fn_ty);
        }
        if let Some(func) =
            declare_fixed_runtime_function(self.backend.context, &self.backend.module, name)
        {
            return require_llvm_function_type(name, func, fn_ty);
        }
        if !is_runtime_import_abi(name, param_count, return_abi) {
            panic!(
                "LLVM runtime import `{name}` has no ABI classification for conservative declaration"
            );
        }
        let func = declare_conservative_runtime_function(
            self.backend.context,
            &self.backend.module,
            name,
            fn_ty,
        );
        require_llvm_function_type(name, func, fn_ty)
    }

    pub(super) fn ensure_runtime_i64_fn(
        &self,
        name: &str,
        param_count: usize,
    ) -> FunctionValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..param_count).map(|_| i64_ty.into()).collect();
        self.ensure_runtime_decl(
            name,
            i64_ty.fn_type(&params, false),
            param_count,
            RuntimeReturnAbi::I64,
        )
    }

    pub(super) fn ensure_runtime_void_fn(
        &self,
        name: &str,
        param_count: usize,
    ) -> FunctionValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..param_count).map(|_| i64_ty.into()).collect();
        self.ensure_runtime_decl(
            name,
            self.backend.context.void_type().fn_type(&params, false),
            param_count,
            RuntimeReturnAbi::Void,
        )
    }

    pub(super) fn ensure_runtime_import(
        &self,
        signature: RuntimeImportSignature,
    ) -> FunctionValue<'ctx> {
        match signature.return_abi {
            RuntimeReturnAbi::I64 => {
                self.ensure_runtime_i64_fn(signature.name, signature.param_count)
            }
            RuntimeReturnAbi::Void => {
                self.ensure_runtime_void_fn(signature.name, signature.param_count)
            }
        }
    }

    pub(super) fn unbox_ptr_bits(
        &self,
        bits: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let masked = self
            .backend
            .builder
            .build_and(
                bits,
                i64_ty.const_int(nanbox::POINTER_MASK, false),
                "ptr_masked",
            )
            .unwrap();
        let shifted = self
            .backend
            .builder
            .build_left_shift(masked, i64_ty.const_int(16, false), "ptr_shifted")
            .unwrap();
        self.backend
            .builder
            .build_right_shift(shifted, i64_ty.const_int(16, false), true, "ptr_signext")
            .unwrap()
    }

    pub(super) fn emit_task_new_with_payload(
        &mut self,
        poll_addr: inkwell::values::IntValue<'ctx>,
        closure_size: i64,
        layout: TaskConstructorLayout,
        payload_operands: &[ValueId],
        call_name: &str,
    ) -> BasicValueEnum<'ctx> {
        layout.validate_closure_size(
            closure_size,
            payload_operands.len(),
            false,
            crate::GENERATOR_CONTROL_BYTES,
        );
        let kind_bits = crate::native_task_runtime_kind_bits(layout.runtime_kind());
        let payload_base = layout.payload_base_offset(crate::GENERATOR_CONTROL_BYTES);
        let i64_ty = self.backend.context.i64_type();
        let task_new_fn = self.ensure_runtime_import(MOLT_TASK_NEW);
        let task_bits = self
            .backend
            .builder
            .build_call(
                task_new_fn,
                &[
                    poll_addr.into(),
                    i64_ty.const_int(closure_size as u64, true).into(),
                    i64_ty.const_int(kind_bits as u64, true).into(),
                ],
                call_name,
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        // A failed task allocation returns boxed None with an exception pending.
        // It is not a task address and must not acquire payload/token ownership.
        // Rejoin with the allocator's exact result rather than returning here:
        // the surrounding TIR owns exception dispatch, frame exits, and drops.
        let initialized = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("task_init{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        let continuation = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("task_ready{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        self.all_llvm_blocks.push(initialized);
        self.all_llvm_blocks.push(continuation);
        let allocated = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                self.ensure_i64(task_bits),
                i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false),
                "task_allocated",
            )
            .unwrap();
        let allocation_block = self.backend.builder.get_insert_block().unwrap();
        self.record_llvm_edge(allocation_block, initialized);
        self.record_llvm_edge(allocation_block, continuation);
        self.backend
            .builder
            .build_conditional_branch(allocated, initialized, continuation)
            .unwrap();
        self.backend.builder.position_at_end(initialized);
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        // `molt_task_new` returns a NaN-boxed task handle. Frame payload stores
        // address raw heap memory, so strip the boxing tag before writing slots,
        // matching native `unbox_ptr_value` and WASM `handle_resolve`.
        let task_ptr_bits = self.unbox_ptr_bits(self.ensure_i64(task_bits));
        let task_ptr = self
            .backend
            .builder
            .build_int_to_ptr(task_ptr_bits, ptr_ty, "task_obj_ptr")
            .unwrap();
        let payload_base_words = (payload_base / 8) as usize;
        // Every payload slot owns one reference. A repeated value keeps one
        // identity: its first slot takes the operand's owned word (an object is
        // retained, a scalar boxed once, and a minted heap integer is already
        // that owner) and each later slot retains the same word. A failed mint
        // stores None with MemoryError pending; no Python code runs before the
        // enclosing operation's exception check observes it.
        let mut transferred: BTreeMap<ValueId, inkwell::values::IntValue<'ctx>> = BTreeMap::new();
        for (idx, &arg_id) in payload_operands.iter().enumerate() {
            let arg_bits = match transferred.get(&arg_id).copied() {
                Some(word) => {
                    let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                    let _ = self
                        .backend
                        .builder
                        .build_call(inc_fn, &[word.into()], "task_payload_inc_ref")
                        .unwrap();
                    word
                }
                None => {
                    let word = self.owned_operand_word(arg_id, "task_payload_inc_ref");
                    transferred.insert(arg_id, word);
                    word
                }
            };
            let field_ptr = unsafe {
                self.backend
                    .builder
                    .build_gep(
                        i64_ty,
                        task_ptr,
                        &[i64_ty.const_int((payload_base_words + idx) as u64, false)],
                        &format!("task_payload_ptr_{idx}"),
                    )
                    .unwrap()
            };
            self.backend
                .builder
                .build_store(field_ptr, arg_bits)
                .unwrap();
        }
        match layout.completion() {
            TaskCompletion::ReturnTask => {}
            TaskCompletion::RegisterCancelToken => {
                let get_token = self.ensure_runtime_import(MOLT_CANCEL_TOKEN_GET_CURRENT);
                let token = self
                    .backend
                    .builder
                    .build_call(get_token, &[], "task_current_token")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let register = self.ensure_runtime_import(MOLT_TASK_REGISTER_EXECUTION);
                self.backend
                    .builder
                    .build_call(
                        register,
                        &[task_bits.into(), token.into(), self.none_word().into()],
                        "",
                    )
                    .unwrap();
            }
            TaskCompletion::WrapAsyncGen => {
                panic!("LLVM async-generator wrappers must use the callable trampoline")
            }
        }
        let initialized_end = self.backend.builder.get_insert_block().unwrap();
        self.record_llvm_edge(initialized_end, continuation);
        self.backend
            .builder
            .build_unconditional_branch(continuation)
            .unwrap();
        self.backend.builder.position_at_end(continuation);
        task_bits
    }
}
