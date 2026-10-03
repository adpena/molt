use super::*;
use crate::runtime_import_abi::{MOLT_ASYNCGEN_NEW, MOLT_DEC_REF_OBJ};

/// Handler-owned routing slice. `code_new` and the CallArgs push/expand
/// steps are exact `molt_<kind>` boxed-ABI calls and take the shared admitted
/// boxed-call path instead of an arm here.
pub(super) const HANDLED_KINDS: &[&str] = &[
    "builtin_func",
    "func_new",
    "func_new_closure",
    "function_closure_bits",
    "asyncgen_new",
    "code_slot_set",
    "code_slots_init",
    "trace_enter_slot",
    "trace_exit",
    "frame_home_store",
    "frame_home_cell",
    "frame_home_private_cell",
    "frame_home_load",
    "frame_home_take",
    "frame_home_clear",
    "frame_locals",
    "frame_locals_set",
    "line",
    "callargs_new",
];

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// The code slots this function addresses in its frame's homes: one past
    /// its largest slot, or `None` when it touches none.
    fn frame_home_slot_count(&self) -> Option<u64> {
        self.func
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .filter(|op| {
                matches!(
                    op.attrs.get("_original_kind"),
                    Some(AttrValue::Str(kind)) if matches!(
                        kind.as_str(),
                        "frame_home_store"
                            | "frame_home_cell"
                            | "frame_home_private_cell"
                            | "frame_home_load"
                            | "frame_home_take"
                            | "frame_home_clear"
                    )
                )
            })
            .filter_map(|op| match op.attrs.get("value") {
                Some(AttrValue::Int(slot)) => u64::try_from(*slot).ok(),
                _ => None,
            })
            .max()
            .map(|slot| slot + 1)
    }

    fn has_frame_entry(&self) -> bool {
        self.func
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|op| {
                matches!(
                    op.attrs.get("_original_kind"),
                    Some(AttrValue::Str(kind)) if kind == "trace_enter_slot"
                )
            })
    }

    /// Borrow the executing frame's homes at the builder's position and keep
    /// the base in this function's entry slot. Returns the base: 0 when the
    /// frame entry failed (its exception pending) or the frame has too few
    /// homes (`SystemError` raised); the caller routes 0 to an exception exit.
    fn lend_frame_homes(&mut self, slots: u64) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let slot = match self.frame_homes {
            Some(slot) => slot,
            None => {
                let slot = self.build_entry_i64_alloca("frame_homes_slot");
                self.frame_homes = Some(slot);
                slot
            }
        };
        let lend = self.ensure_runtime_i64_fn("molt_frame_homes", 1);
        let homes = self
            .backend
            .builder
            .build_call(
                lend,
                &[i64_ty.const_int(slots, false).into()],
                "frame_homes",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        self.backend.builder.build_store(slot, homes).unwrap();
        homes
    }

    /// A split chunk has no frame entry of its own: at function entry it
    /// borrows the homes of the frame it runs in, and returns at once, with
    /// the exception pending, when none are lent.
    pub(in crate::llvm_backend::lowering) fn lend_chunk_frame_homes(&mut self) {
        if self.has_frame_entry() {
            return;
        }
        let Some(slots) = self.frame_home_slot_count() else {
            return;
        };
        let homes = self.lend_frame_homes(slots);
        let failed = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                homes,
                self.backend.context.i64_type().const_zero(),
                "frame_homes_unlent",
            )
            .unwrap();
        let fail_bb = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, "frame_homes_unlent");
        let lent_bb = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, "frame_homes_lent");
        self.all_llvm_blocks.extend([fail_bb, lent_bb]);
        self.backend
            .builder
            .build_conditional_branch(failed, fail_bb, lent_bb)
            .unwrap();
        self.backend.builder.position_at_end(fail_bb);
        self.build_empty_return();
        self.backend.builder.position_at_end(lent_bb);
    }

    /// The address of one slot's home word at `offset` within it.
    fn frame_home_word(
        &mut self,
        slot: i64,
        offset: i32,
    ) -> Option<inkwell::values::PointerValue<'ctx>> {
        let homes_slot = self.frame_homes?;
        let i64_ty = self.backend.context.i64_type();
        let builder = &self.backend.builder;
        let homes = builder
            .build_load(i64_ty, homes_slot, "frame_homes")
            .unwrap()
            .into_int_value();
        let byte = slot
            .checked_mul(molt_codegen_abi::FRAME_HOME_BYTES)?
            .checked_add(i64::from(offset))?;
        let address = builder
            .build_int_add(
                homes,
                i64_ty.const_int(byte as u64, false),
                "frame_home_address",
            )
            .unwrap();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        Some(
            builder
                .build_int_to_ptr(address, ptr_ty, "frame_home")
                .unwrap(),
        )
    }

    /// Publish a boxed view of a raw integer in its owning home. Inline values
    /// need no runtime call; the slow path uses the same persistent publication
    /// as every other home observer. Failure leaves the raw home intact and
    /// returns the missing sentinel with the exception pending.
    fn box_frame_home_raw_int(
        &mut self,
        kind_ptr: inkwell::values::PointerValue<'ctx>,
        bits_ptr: inkwell::values::PointerValue<'ctx>,
        raw: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let context = self.backend.context;
        let i64_ty = context.i64_type();
        let fits = inline_int_fits_with_builder(&self.backend.builder, context, raw);
        let inline_bb = context.append_basic_block(self.llvm_fn, "frame_home_box_inline");
        let slow_bb = context.append_basic_block(self.llvm_fn, "frame_home_box_slow");
        let done_bb = context.append_basic_block(self.llvm_fn, "frame_home_boxed");
        self.all_llvm_blocks.extend([inline_bb, slow_bb, done_bb]);
        let origin = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(fits, inline_bb, slow_bb)
            .unwrap();
        self.record_llvm_edge(origin, inline_bb);
        self.record_llvm_edge(origin, slow_bb);

        self.backend.builder.position_at_end(inline_bb);
        let inline = inline_int_box_with_builder(&self.backend.builder, context, raw);
        self.backend.builder.build_store(bits_ptr, inline).unwrap();
        self.backend
            .builder
            .build_store(
                kind_ptr,
                i64_ty.const_int(molt_codegen_abi::FRAME_HOME_PLAIN as u64, false),
            )
            .unwrap();
        self.backend
            .builder
            .build_unconditional_branch(done_bb)
            .unwrap();
        self.record_llvm_edge(inline_bb, done_bb);

        self.backend.builder.position_at_end(slow_bb);
        let home = self
            .backend
            .builder
            .build_ptr_to_int(kind_ptr, i64_ty, "frame_home_address")
            .unwrap();
        let load_fn = self.ensure_runtime_i64_fn("molt_frame_home_load", 1);
        let slow = self
            .backend
            .builder
            .build_call(load_fn, &[home.into()], "frame_home_load")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        self.backend
            .builder
            .build_unconditional_branch(done_bb)
            .unwrap();
        self.record_llvm_edge(slow_bb, done_bb);

        self.backend.builder.position_at_end(done_bb);
        let boxed = self
            .backend
            .builder
            .build_phi(i64_ty, "frame_home_boxed_value")
            .unwrap();
        boxed.add_incoming(&[(&inline, inline_bb), (&slow, slow_bb)]);
        boxed.as_basic_value().into_int_value()
    }

    /// Replace one slot's pair, materialize its boxed view when required, then
    /// release the displaced owner. A finalizer must see the completed new
    /// binding, including the box the returned view borrows. `None`: no lent
    /// home. The result borrows the published home; it owns no temporary box.
    fn replace_frame_home(
        &mut self,
        slot: i64,
        kind: i64,
        bits: inkwell::values::IntValue<'ctx>,
        boxed_view: bool,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        let i64_ty = self.backend.context.i64_type();
        let (Some(kind_ptr), Some(bits_ptr)) = (
            self.frame_home_word(slot, molt_codegen_abi::FRAME_HOME_KIND_OFFSET),
            self.frame_home_word(slot, molt_codegen_abi::FRAME_HOME_BITS_OFFSET),
        ) else {
            return None;
        };
        let builder = &self.backend.builder;
        let old_kind = builder
            .build_load(i64_ty, kind_ptr, "frame_home_old_kind")
            .unwrap()
            .into_int_value();
        let old_bits = builder
            .build_load(i64_ty, bits_ptr, "frame_home_old_bits")
            .unwrap();
        builder
            .build_store(kind_ptr, i64_ty.const_int(kind as u64, false))
            .unwrap();
        builder.build_store(bits_ptr, bits).unwrap();
        let view = if kind == molt_codegen_abi::FRAME_HOME_RAW_INT && boxed_view {
            self.box_frame_home_raw_int(kind_ptr, bits_ptr, bits)
        } else {
            bits
        };
        let builder = &self.backend.builder;
        let holds = builder
            .build_and(
                old_kind,
                i64_ty.const_int(molt_codegen_abi::FRAME_HOME_HOLDS_REFERENCE as u64, false),
                "frame_home_old_owner",
            )
            .unwrap();
        let holds = builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                holds,
                i64_ty.const_zero(),
                "frame_home_release_needed",
            )
            .unwrap();
        let release_bb = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, "frame_home_release");
        let done_bb = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, "frame_home_replaced");
        self.all_llvm_blocks.extend([release_bb, done_bb]);
        self.backend
            .builder
            .build_conditional_branch(holds, release_bb, done_bb)
            .unwrap();
        self.backend.builder.position_at_end(release_bb);
        let dec_ref = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        self.backend
            .builder
            .build_call(dec_ref, &[old_bits.into()], "frame_home_release")
            .unwrap();
        self.backend
            .builder
            .build_unconditional_branch(done_bb)
            .unwrap();
        self.backend.builder.position_at_end(done_bb);
        Some(view)
    }

    /// The runtime entry custody of the function object `op` (a `func_new` or
    /// `func_new_closure`) creates for `name`. A task constructor's arguments
    /// enter through its task trampoline, which retains each into the new
    /// task, so it borrows. Otherwise the word comes from the parameter
    /// declaration in the target's linkage row; a target without one, or one
    /// the runtime cannot encode in one bit, is a compiler error: guessing
    /// "borrowed" would let a transferring body release its caller's
    /// references.
    fn function_entry_custody_word(
        &self,
        op: &TirOp,
        name: &str,
        has_closure: bool,
        arity: usize,
    ) -> u64 {
        if op.attrs.contains_key("task_kind") {
            return 0;
        }
        let linkage_abi = self
            .backend
            .function_linkage_abis
            .get(name)
            .unwrap_or_else(|| panic!("func_new target `{name}` has no exact native linkage ABI"));
        let transferred: Vec<bool> = linkage_abi
            .parameter_custody
            .iter()
            .map(|custody| matches!(custody, crate::ir::ParameterCustody::Transferred))
            .collect();
        molt_codegen_abi::EntryCustodyDeclaration::declare(
            linkage_abi.source_signature.has_closure,
            linkage_abi.source_signature.arity,
            &transferred,
        )
        .encode(has_closure, arity)
        .unwrap_or_else(|error| {
            panic!("func_new target `{name}` has no runtime entry custody: {error:?}")
        })
    }
}

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_callable_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            "function_closure_bits" => {
                if op.operands.len() != 1 || op.results.len() > 1 {
                    return false;
                }
                let closure_fn = self.ensure_runtime_i64_fn("molt_function_closure_bits", 1);
                // The function owns this edge. Only a retained SSA result gains
                // an owner; a discarded borrowed edge must never be decreffed.
                self.emit_borrowed_runtime_call(
                    op,
                    closure_fn,
                    &[RuntimeArg::Operand(op.operands[0])],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue),
                    kind,
                    "function_closure_bits",
                );
                true
            }
            "asyncgen_new" => {
                if op.operands.len() != 1 || op.results.len() > 1 {
                    return false;
                }
                // Dedicated task-wrapper ABI; a machine i64 declaration does
                // not authorize generic boxed-call admission for this symbol.
                let wrap = self.ensure_runtime_import(MOLT_ASYNCGEN_NEW);
                self.emit_borrowed_runtime_call(
                    op,
                    wrap,
                    &[RuntimeArg::Operand(op.operands[0])],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "asyncgen_new",
                );
                true
            }
            "builtin_func" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "builtin_func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, false);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "builtin_trampoline_ptr",
                    )
                    .unwrap();
                let arity_bits = i64_ty.const_int(arity as u64, false);
                let (new_fn, mut args) = if let Some(&name_id) = op.operands.first() {
                    (
                        self.ensure_runtime_i64_fn("molt_func_new_builtin_named", 4),
                        vec![RuntimeArg::Operand(name_id)],
                    )
                } else {
                    (
                        self.ensure_runtime_i64_fn("molt_func_new_builtin", 3),
                        Vec::new(),
                    )
                };
                args.extend([
                    RuntimeArg::Word(fn_ptr.into()),
                    RuntimeArg::Word(tramp_ptr.into()),
                    RuntimeArg::Word(arity_bits.into()),
                ]);
                self.emit_borrowed_runtime_call(
                    op,
                    new_fn,
                    &args,
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "builtin_func_new",
                );
                true
            }
            "func_new" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, false);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "func_trampoline_ptr",
                    )
                    .unwrap();
                let custody = self.function_entry_custody_word(op, func_name, false, arity);
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new", 4);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                            i64_ty.const_int(custody, false).into(),
                        ],
                        "func_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.bind_owned_runtime_result(op, result);
                true
            }
            "func_new_closure" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let Some(&closure_id) = op.operands.first() else {
                    return false;
                };
                let func = self.ensure_function_symbol(func_name, arity, true);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "closure_func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, true);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "closure_trampoline_ptr",
                    )
                    .unwrap();
                let custody = self.function_entry_custody_word(op, func_name, true, arity);
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new_closure", 5);
                // Code addresses, the arity and the entry custody are raw ABI
                // words; the function retains the borrowed closure object.
                self.emit_borrowed_runtime_call(
                    op,
                    new_fn,
                    &[
                        RuntimeArg::Word(fn_ptr.into()),
                        RuntimeArg::Word(tramp_ptr.into()),
                        RuntimeArg::Word(i64_ty.const_int(arity as u64, false).into()),
                        RuntimeArg::Operand(closure_id),
                        RuntimeArg::Word(i64_ty.const_int(custody, false).into()),
                    ],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "func_new_closure",
                );
                true
            }
            "code_slot_set" => {
                let code_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .expect("admitted code_slot_set ID");
                let slot_set_fn = self.ensure_runtime_i64_fn("molt_code_slot_set", 3);
                // The slot ID is a raw ABI word; the code object and globals
                // are borrowed. Only the exception state reports failure.
                self.emit_borrowed_runtime_call(
                    op,
                    slot_set_fn,
                    &[
                        RuntimeArg::Word(i64_ty.const_int(code_id as u64, true).into()),
                        RuntimeArg::Operand(op.operands[0]),
                        RuntimeArg::Operand(op.operands[1]),
                    ],
                    RuntimeResultCustody::SideEffect,
                    kind,
                    "code_slot_set",
                );
                true
            }
            "code_slots_init" => {
                let count = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .expect("admitted code_slots_init count");
                let init_fn = self.ensure_runtime_i64_fn("molt_code_slots_init", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        init_fn,
                        &[i64_ty.const_int(count as u64, true).into()],
                        "code_slots_init",
                    )
                    .unwrap();
                true
            }
            "trace_enter_slot" => {
                let code_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .expect("admitted trace_enter_slot ID");
                let enter_fn = self.ensure_runtime_i64_fn("molt_trace_enter_slot", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        enter_fn,
                        &[i64_ty.const_int(code_id as u64, true).into()],
                        "trace_enter_slot",
                    )
                    .unwrap();
                // The entry took the frame's binding homes: lend them before
                // the entry's adjacent exception check, which leaves for the
                // entry-failure label when the lend is 0.
                if let Some(slots) = self.frame_home_slot_count() {
                    self.lend_frame_homes(slots);
                }
                true
            }
            "trace_exit" => {
                let exit_fn = self.ensure_runtime_i64_fn("molt_trace_exit", 0);
                let _ = self
                    .backend
                    .builder
                    .build_call(exit_fn, &[], "trace_exit")
                    .unwrap();
                true
            }
            "frame_home_store" | "frame_home_cell" | "frame_home_private_cell" => {
                // The home takes operand 0's reference: the op consumes it. A
                // raw integer carrier is stored raw, holding no reference. A
                // boxed result is already an observation and must borrow the
                // persistent box published into that home, before displacement
                // can run a finalizer. Only the result's shared Repr fact can
                // authorize forwarding a bare integer view.
                let Some(&operand) = op.operands.first() else {
                    return false;
                };
                let Some(slot) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return false;
                };
                let value = self.resolve(operand);
                let value_ty = self
                    .value_types
                    .get(&operand)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let (home_kind, bits) = match (kind, &value_ty, value) {
                    ("frame_home_store", TirType::I64, BasicValueEnum::IntValue(raw)) => {
                        (molt_codegen_abi::FRAME_HOME_RAW_INT, raw)
                    }
                    ("frame_home_cell", ..) => (
                        molt_codegen_abi::FRAME_HOME_CELL,
                        self.adopted_storage_word(operand),
                    ),
                    ("frame_home_private_cell", ..) => (
                        molt_codegen_abi::FRAME_HOME_PRIVATE_CELL,
                        self.adopted_storage_word(operand),
                    ),
                    _ => (
                        molt_codegen_abi::FRAME_HOME_PLAIN,
                        self.adopted_storage_word(operand),
                    ),
                };
                let boxed_view = home_kind == molt_codegen_abi::FRAME_HOME_RAW_INT
                    && op
                        .results
                        .first()
                        .is_some_and(|&result| !self.repr_facts.is_raw_int_carrier(result));
                let Some(view) = self.replace_frame_home(slot, home_kind, bits, boxed_view) else {
                    return false;
                };
                // The result borrows the binding until this slot's next write.
                if let Some(&result) = op.results.first() {
                    if boxed_view {
                        self.values.insert(result, view.into());
                        self.value_types.insert(result, TirType::DynBox);
                    } else {
                        self.values.insert(result, value);
                        self.value_types.insert(result, value_ty);
                    }
                }
                true
            }
            "frame_home_clear" => {
                // `del`: the slot becomes unbound, then what it held goes.
                let Some(slot) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return false;
                };
                self.replace_frame_home(
                    slot,
                    molt_codegen_abi::FRAME_HOME_UNBOUND,
                    i64_ty.const_zero(),
                    false,
                )
                .is_some()
            }
            "frame_home_load" => {
                // A borrowed view of the slot's plain binding: `PLAIN` read
                // inline; the runtime boxes a raw integer into the home,
                // reports an unbound slot as the missing sentinel and raises
                // for a cell.
                let Some(slot) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return false;
                };
                let (Some(kind_ptr), Some(bits_ptr)) = (
                    self.frame_home_word(slot, molt_codegen_abi::FRAME_HOME_KIND_OFFSET),
                    self.frame_home_word(slot, molt_codegen_abi::FRAME_HOME_BITS_OFFSET),
                ) else {
                    return false;
                };
                let home_kind = self
                    .backend
                    .builder
                    .build_load(i64_ty, kind_ptr, "frame_home_kind")
                    .unwrap()
                    .into_int_value();
                let plain = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        home_kind,
                        i64_ty.const_int(molt_codegen_abi::FRAME_HOME_PLAIN as u64, false),
                        "frame_home_plain",
                    )
                    .unwrap();
                let inline_bb = self
                    .backend
                    .context
                    .append_basic_block(self.llvm_fn, "frame_home_inline");
                let slow_bb = self
                    .backend
                    .context
                    .append_basic_block(self.llvm_fn, "frame_home_slow");
                let done_bb = self
                    .backend
                    .context
                    .append_basic_block(self.llvm_fn, "frame_home_loaded");
                self.all_llvm_blocks.extend([inline_bb, slow_bb, done_bb]);
                self.backend
                    .builder
                    .build_conditional_branch(plain, inline_bb, slow_bb)
                    .unwrap();
                self.backend.builder.position_at_end(inline_bb);
                let inline_bits = self
                    .backend
                    .builder
                    .build_load(i64_ty, bits_ptr, "frame_home_bits")
                    .unwrap();
                self.backend
                    .builder
                    .build_unconditional_branch(done_bb)
                    .unwrap();
                self.backend.builder.position_at_end(slow_bb);
                let home = self
                    .backend
                    .builder
                    .build_ptr_to_int(kind_ptr, i64_ty, "frame_home_address")
                    .unwrap();
                let load_fn = self.ensure_runtime_i64_fn("molt_frame_home_load", 1);
                let slow_bits = self
                    .backend
                    .builder
                    .build_call(load_fn, &[home.into()], "frame_home_load")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(done_bb)
                    .unwrap();
                self.backend.builder.position_at_end(done_bb);
                let loaded = self
                    .backend
                    .builder
                    .build_phi(i64_ty, "frame_home_value")
                    .unwrap();
                loaded.add_incoming(&[(&inline_bits, inline_bb), (&slow_bits, slow_bb)]);
                if let Some(&result) = op.results.first() {
                    self.values.insert(result, loaded.as_basic_value());
                    self.value_types.insert(result, TirType::DynBox);
                }
                true
            }
            "frame_home_take" => {
                // PEP 709's save of an enclosing binding: the runtime moves it
                // out of the home, which becomes unbound; the result owns it.
                let Some(slot) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return false;
                };
                let Some(home) =
                    self.frame_home_word(slot, molt_codegen_abi::FRAME_HOME_KIND_OFFSET)
                else {
                    return false;
                };
                let home = self
                    .backend
                    .builder
                    .build_ptr_to_int(home, i64_ty, "frame_home_address")
                    .unwrap();
                let take_fn = self.ensure_runtime_i64_fn("molt_frame_home_take", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    take_fn,
                    &[RuntimeArg::Word(home.into())],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "frame_home_take",
                );
                true
            }
            "frame_locals" => {
                // `locals()`: the runtime's one authority over the executing
                // frame; the operand pairs serve targets that build the dict.
                let locals_fn = self.ensure_runtime_i64_fn("molt_locals_builtin", 0);
                self.emit_borrowed_runtime_call(
                    op,
                    locals_fn,
                    &[],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "frame_locals",
                );
                true
            }
            "frame_locals_set" => {
                let Some(&dict_id) = op.operands.first() else {
                    return false;
                };
                let frame_locals_fn = self.ensure_runtime_i64_fn("molt_frame_locals_set", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    frame_locals_fn,
                    &[RuntimeArg::Operand(dict_id)],
                    RuntimeResultCustody::SideEffect,
                    kind,
                    "frame_locals_set",
                );
                true
            }
            "line" => {
                let line = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let line_fn = self.ensure_runtime_i64_fn("molt_trace_set_line", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        line_fn,
                        &[i64_ty.const_int(line as u64, true).into()],
                        "trace_set_line",
                    )
                    .unwrap();
                true
            }
            "callargs_new" => {
                // The source call form picks the builder: a CALL_FUNCTION_EX
                // call site's arguments are its own tuple and mapping.
                let constructor = op
                    .call_argument_form()
                    .expect("validated callargs_new call form")
                    .runtime_constructor();
                let new_fn = self.ensure_runtime_i64_fn(constructor, 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[i64_ty.const_zero().into(), i64_ty.const_zero().into()],
                        "callargs_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.bind_owned_runtime_result(op, result);
                true
            }
            _ => unreachable!("preserved callable kind predicate accepted unknown kind {kind}"),
        }
    }
}
