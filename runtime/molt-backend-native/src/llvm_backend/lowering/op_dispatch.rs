use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        match op.opcode {
            // -- Constants --
            OpCode::ConstInt => self.emit_const_int(op),
            OpCode::ModuleSlot => self.emit_module_slot(op),
            OpCode::ConstFloat => self.emit_const_float(op),
            OpCode::ConstBool => self.emit_const_bool(op),
            OpCode::ConstNone => self.emit_const_none(op),
            OpCode::ConstStr => self.emit_const_str(op),
            OpCode::ConstBigInt => self.emit_const_bigint(op),
            OpCode::ConstBytes => self.emit_const_bytes(op),

            // -- Arithmetic (type-specialized) --
            OpCode::Add | OpCode::InplaceAdd => self.emit_binary_arith(op, OpKind::Add),
            OpCode::CheckedAdd => self.emit_checked_add(op),
            OpCode::CheckedMul => self.emit_checked_mul(op),
            OpCode::Sub | OpCode::InplaceSub => self.emit_binary_arith(op, OpKind::Sub),
            OpCode::Mul | OpCode::InplaceMul => self.emit_binary_arith(op, OpKind::Mul),
            OpCode::Div => self.emit_binary_arith(op, OpKind::Div),
            OpCode::FloorDiv => self.emit_binary_arith(op, OpKind::Floordiv),
            OpCode::Mod => self.emit_binary_arith(op, OpKind::Mod),
            OpCode::Pow => self.emit_binary_arith(op, OpKind::Pow),

            // -- Unary --
            OpCode::Neg => self.emit_unary(op, OpKind::Neg),
            OpCode::Pos => self.emit_unary(op, OpKind::Pos),
            OpCode::Not => self.emit_unary(op, OpKind::Not),

            // -- Comparison (type-specialized) --
            OpCode::Eq => self.emit_comparison(op, OpKind::Eq),
            OpCode::Ne => self.emit_comparison(op, OpKind::Ne),
            OpCode::Lt => self.emit_comparison(op, OpKind::Lt),
            OpCode::Le => self.emit_comparison(op, OpKind::Le),
            OpCode::Gt => self.emit_comparison(op, OpKind::Gt),
            OpCode::Ge => self.emit_comparison(op, OpKind::Ge),
            OpCode::Is | OpCode::IsNot => self.emit_identity(op),
            OpCode::In | OpCode::NotIn => self.emit_containment(op),

            // -- Bitwise --
            OpCode::BitAnd => self.emit_bitwise(op, OpKind::BitAnd),
            OpCode::BitOr => self.emit_bitwise(op, OpKind::BitOr),
            OpCode::BitXor => self.emit_bitwise(op, OpKind::BitXor),
            OpCode::BitNot => self.emit_unary(op, OpKind::Invert),
            OpCode::Shl => self.emit_bitwise(op, OpKind::Lshift),
            OpCode::Shr => self.emit_bitwise(op, OpKind::Rshift),

            // -- Boolean --
            OpCode::And | OpCode::Or => self.emit_boolean_selection(op),
            OpCode::Bool => {
                let result_id = op.results[0];
                let operand_id = op.operands[0];
                let operand = self.resolve(operand_id);
                let operand_ty = self
                    .values
                    .ty(operand_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);

                let bool_val = match operand_ty {
                    TirType::Bool => operand.into_int_value(),
                    TirType::I64 => self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            operand.into_int_value(),
                            self.backend.context.i64_type().const_zero(),
                            "bool_i64",
                        )
                        .unwrap(),
                    TirType::F64 => self
                        .backend
                        .builder
                        // `bool(nan)` is True: unordered-or-unequal to zero.
                        .build_float_compare(
                            inkwell::FloatPredicate::UNE,
                            operand.into_float_value(),
                            self.backend.context.f64_type().const_float(0.0),
                            "bool_f64",
                        )
                        .unwrap(),
                    _ => {
                        let truthy_fn = self.ensure_runtime_import(RuntimeFn::IsTruthy);
                        let truthy = self
                            .backend
                            .builder
                            .build_execution_call(
                                truthy_fn,
                                &[self.ensure_i64(operand).into()],
                                "truthy_bool",
                            )
                            .unwrap()
                            .try_as_basic_value()
                            .unwrap_basic()
                            .into_int_value();
                        self.backend
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::NE,
                                truthy,
                                self.backend.context.i64_type().const_zero(),
                                "bool_dynbox",
                            )
                            .unwrap()
                    }
                };
                self.values.set_value(result_id, bool_val.into());
                self.values.set_type(result_id, TirType::Bool);
            }

            // -- Late-TIR representation primitives and the cold runtime edge
            // (HF-821): bit-level meaning from the shared word generators.
            OpCode::IsInlineInt => {
                let word = self.ensure_i64(self.resolve(op.operands[0]));
                let is_int = nanbox::boxed_word::generate_has_tag(
                    &mut self.words(),
                    word,
                    nanbox::boxed_word::BoxTag::Int,
                );
                self.values.set_value(op.results[0], is_int.into());
                self.values.set_type(op.results[0], TirType::Bool);
            }
            OpCode::UnboxInlineInt => {
                let word = self.ensure_i64(self.resolve(op.operands[0]));
                let raw = nanbox::boxed_word::generate_unbox_inline_int(&mut self.words(), word);
                self.values.set_value(op.results[0], raw.into());
                self.values.set_type(op.results[0], TirType::I64);
            }
            OpCode::IsFloatWord => {
                let word = self.ensure_i64(self.resolve(op.operands[0]));
                let is_float = nanbox::boxed_word::generate_is_float(&mut self.words(), word);
                self.values.set_value(op.results[0], is_float.into());
                self.values.set_type(op.results[0], TirType::Bool);
            }
            OpCode::UnboxFloatWord => {
                let word = self.ensure_i64(self.resolve(op.operands[0]));
                let raw = self
                    .backend
                    .builder
                    .build_bit_cast(word, self.backend.context.f64_type(), "unbox_float")
                    .unwrap();
                self.values.set_value(op.results[0], raw);
                self.values.set_type(op.results[0], TirType::F64);
            }
            OpCode::IsBoolWord => {
                let word = self.ensure_i64(self.resolve(op.operands[0]));
                let is_bool = nanbox::boxed_word::generate_has_tag(
                    &mut self.words(),
                    word,
                    nanbox::boxed_word::BoxTag::Bool,
                );
                self.values.set_value(op.results[0], is_bool.into());
                self.values.set_type(op.results[0], TirType::Bool);
            }
            OpCode::UnboxBoolWord => {
                let word = self.ensure_i64(self.resolve(op.operands[0]));
                let truth = nanbox::boxed_word::generate_unbox_bool(&mut self.words(), word);
                self.values.set_value(op.results[0], truth.into());
                self.values.set_type(op.results[0], TirType::Bool);
            }
            OpCode::CodeIs => self.emit_code_is(op),
            OpCode::DirectCall => self.emit_guarded_direct_call(op),
            OpCode::BoxInlineInt => {
                let value = self.ensure_i64(self.resolve(op.operands[0]));
                let word = nanbox::boxed_word::generate_box_inline_int(&mut self.words(), value);
                self.values.set_value(op.results[0], word.into());
                self.values.set_type(op.results[0], TirType::DynBox);
            }
            OpCode::CallRuntime => {
                let runtime = op
                    .runtime
                    .expect("verified: CallRuntime carries its runtime entry");
                match runtime.boxed_abi() {
                    Some(abi) => self.emit_boxed_runtime_call(op, abi),
                    None => self.emit_raw_runtime_call(op, runtime),
                }
            }

            // -- Box/Unbox --
            OpCode::BoxVal => self.emit_box(op),
            OpCode::UnboxVal => self.emit_unbox(op),
            OpCode::TypeGuard => {
                // Type guard: in lowered code, this is a no-op assertion.
                // The value passes through; if the guard fails at runtime,
                // deopt kicks in (handled elsewhere).
                let result_id = op.results[0];
                let val = self.resolve(op.operands[0]);
                self.values.set_value(result_id, val);
                let ty = self
                    .values
                    .ty(op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                self.values.set_type(result_id, ty);
            }

            // -- Refcount --
            OpCode::IncRef => self.emit_refcount(
                op,
                molt_codegen_abi::inline_refcount::RefcountOperation::Retain,
            ),
            // Surviving Python lifetime boundaries share terminal release
            // with inserted drops. Raw carriers have no runtime heap owner.
            OpCode::DelBoundary | OpCode::DecRef => self.emit_refcount(
                op,
                molt_codegen_abi::inline_refcount::RefcountOperation::Release,
            ),

            // -- Memory / Attribute / Index --
            OpCode::LoadAttr => self.emit_load_attr(op),
            OpCode::StoreAttr => self.emit_store_attr(op),
            OpCode::DelAttr => self.emit_del_attr(op),
            OpCode::Index => self.emit_index(op),
            OpCode::StoreIndex => self.emit_store_index(op),
            OpCode::DelIndex => self.emit_del_index(op),

            // -- Call --
            OpCode::Call => self.emit_call(op),

            // -- DeleteVar local-slot transition --
            // DeleteVar defines the local's new SSA value as the missing sentinel
            // operand. Ownership of the previous slot occupant is modeled by the
            // drop fact plane (DecRef / explicit consumed operands), not by LLVM
            // lowering.
            OpCode::DeleteVar => {
                if op.results.is_empty() {
                    // Side-effect-only legacy shapes have no SSA value to bind.
                } else if let Some(&missing) = op.operands.first() {
                    let val = self.resolve(missing);
                    let ty = self.values.ty(missing).cloned().unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.set_value(result_id, val);
                        self.values.set_type(result_id, ty.clone());
                    }
                } else {
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false)
                        .into();
                    for &result_id in &op.results {
                        self.values.set_value(result_id, none_val);
                        self.values.set_type(result_id, TirType::DynBox);
                    }
                }
            }

            // -- SSA Copy --
            // Also serves as the fallback for unknown frontend ops that were
            // mapped to Copy by the SSA converter.  Handle all combinations of
            // operand/result counts gracefully:
            //   - 0 operands, 0 results: no-op (side-effect only)
            //   - 0 operands, 1+ results: produce NaN-boxed None per result
            //   - 1+ operands, 0 results: no-op (side-effect only)
            //   - 1+ operands, 1+ results: pass-through first operand
            OpCode::FrameSetLocation => {
                assert!(self.lower_preserved_simpleir_op(op, OpKind::FrameSetLocation));
            }
            OpCode::Copy => {
                let original_kind = op.origin;
                if let Some(kind) = original_kind
                    && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
                // TERMINAL FAIL-LOUD STATE (preserved-op passthrough class),
                // tied to the `CopyLowering` single source of truth.
                //
                // Reaching here means this `OpCode::Copy` carries an
                // `origin` that `lower_preserved_simpleir_op` did NOT
                // handle — neither a dedicated arm nor the generic
                // `try_lower_preserved_runtime_call` (`molt_<kind>`) fallback
                // claimed it. EVERY such op is a SimpleIR op the native/WASM/Luau
                // lanes lower with dedicated semantics (a value-producing runtime
                // call, an RC adjustment, a type guard, a generator throw, or a
                // registry-owned identity fact). Passing operand 0 through by
                // default -- the historical behavior -- silently miscompiled
                // non-identity semantics: `abs(x)` returned `x`,
                // `...`/`NotImplemented` became `None`, `raise ... from ...`'s
                // `__cause__` link, `gen.throw`, special-attr loads, the
                // `borrow`/`release` refcount ops, the `guard_tag` type check, and
                // every fresh-value conversion (`int(x)`, `s[-5:]`, `dict.keys()`,
                // ...) were all DROPPED or returned operand 0. The few preserved
                // repr-identity ops whose operand-0 passthrough is correct are
                // claimed explicitly in `lower_preserved_simpleir_op`; any
                // preserved op that reaches this terminal state is therefore not a
                // sound default passthrough. Fail the build loudly here.
                //
                // SINGLE SOURCE OF TRUTH (the drift the `CopyLowering` classifier
                // forbids). The drop-insertion pass releases exactly the `Copy`s
                // whose `origin` is a `CopyLowering::OwnedValue`
                // (`alias_analysis::copy_kind_mints_owned_value`). If such a
                // fresh-owned producer reached codegen as a silent operand-0
                // passthrough, the result would (a) be the wrong value AND (b)
                // alias operand 0 — which the drop pass then DOUBLE-FREES. The gate
                // therefore consults that classifier on every fatal so the table
                // and the backend cannot drift: a `OwnedValue` reaching here is the
                // forbidden drift (a fresh-value op missing its explicit LLVM arm),
                // and the diagnostic names it as such; any other `origin`
                // gets the general terminal message with operand/result counts.
                // Closing a kind = add an arm to `lower_preserved_simpleir_op` (or,
                // if `molt_<kind>` is a real boxed runtime intrinsic, the generic
                // fallback already covers it). See the `CopyLowering` docs and
                // `tests::copy_lowering_classes_are_total_and_disjoint`.
                if let Some(kind) = original_kind {
                    if crate::tir::passes::alias_analysis::copy_kind_reaches_no_incref_passthrough(
                        Some(kind),
                    ) {
                        // Not a `OwnedValue` (a transparent-alias / inert-marker
                        // kind whose `molt_<kind>` intrinsic is also absent): the
                        // partner's general terminal state. Still fail loud — an
                        // unhandled `origin` is never a sound passthrough.
                        self.record_fatal(format!(
                            "unhandled preserved SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile or drop its side effect; add a \
                             `lower_preserved_simpleir_op` arm for it (or confirm \
                             `molt_{kind}` is a boxed runtime intrinsic so the \
                             generic fallback claims it)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    } else {
                        // A `CopyLowering::OwnedValue` reached the passthrough: the
                        // exact classifier?backend drift this gate exists to catch.
                        self.record_fatal(format!(
                            "fresh-value SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile AND make the result alias operand 0 (a \
                             drop-insertion double-free); it is in \
                             `alias_analysis::copy_kind_mints_owned_value` so it \
                             MUST have a `lower_preserved_simpleir_op` arm (the \
                             classifier and the LLVM lowering have drifted)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    }
                    return;
                }

                // `origin == None`: a genuine SSA value copy
                // (`copy`/`copy_var`/`load_var`/`store_var`). Operand-0
                // passthrough is the correct lowering.
                if op.results.is_empty() {
                    // No results — nothing to bind; skip.
                } else if op.operands.is_empty() {
                    // Unknown op with no operands — produce None for each result.
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false)
                        .into();
                    for &result_id in &op.results {
                        self.values.set_value(result_id, none_val);
                        self.values.set_type(result_id, TirType::DynBox);
                    }
                } else {
                    // Standard copy: pass through first operand.
                    let val = self.resolve(op.operands[0]);
                    let ty = self
                        .values
                        .ty(op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.set_value(result_id, val);
                        self.values.set_type(result_id, ty.clone());
                    }
                }
            }

            // -- Allocation --
            OpCode::Alloc => {
                let result_id = op.results[0];
                let size = self.resolve(op.operands[0]);
                let size_i64 = self.ensure_i64(size);
                let alloc_fn = self.ensure_runtime_import(RuntimeFn::Alloc);
                let unpublished = self
                    .backend
                    .builder
                    .build_execution_call(alloc_fn, &[size_i64.into()], "alloc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let publish_fn = self.ensure_runtime_import(RuntimeFn::ObjectPublishInitialized);
                let result = self
                    .backend
                    .builder
                    .build_execution_call(publish_fn, &[unpublished.into()], "publish_initialized")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.set_value(result_id, result);
                self.values.set_type(result_id, TirType::DynBox);
            }

            // -- Calls --
            OpCode::CallMethod => self.emit_call_method(op),

            OpCode::LoadMethod | OpCode::LoadSuperMethod => self.emit_load_method(op),

            OpCode::CallLoadedMethod => self.emit_call_loaded_method(op),

            OpCode::CallargsPushMethodReceiver => self.emit_callargs_push_method_receiver(op),

            OpCode::CallBuiltin => self.emit_call_builtin(op),

            // -- OrdAt: fused ord(container[index]) --
            OpCode::OrdAt => {
                if op.operands.len() < 2 {
                    return;
                }
                let ord_at_fn = self.ensure_runtime_import(RuntimeFn::OrdAt);
                let args = [
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Operand(op.operands[1]),
                ];
                self.emit_borrowed_runtime_call(
                    op,
                    ord_at_fn,
                    &args,
                    Self::canonical_boxed_return(RuntimeFn::OrdAt, 2),
                    "ord_at",
                    "ord_at",
                );
            }

            // Raw boxed frame allocation is not implemented. In particular,
            // a naked i64 alloca cannot stand in for a tagged object payload.
            OpCode::StackAlloc => {
                panic!(
                    "{}",
                    crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED
                );
            }

            // -- Free: stack-allocated slots are freed automatically — no-op --
            OpCode::Free => {
                // Stack memory is reclaimed by the function epilogue; nothing to emit.
            }

            // -- BuildList: [item0, item1, ...] --
            // Borrow the fixed operand range for one constructor transaction.
            OpCode::BuildList => self.emit_build_list(op),

            // -- BuildDict: {k0: v0, k1: v1, ...} --
            // operands: [k0, v0, k1, v1, ...]  (pairs)
            OpCode::BuildDict => self.emit_build_dict(op),

            // -- BuildTuple: (item0, item1, ...) --
            OpCode::BuildTuple => self.emit_build_tuple(op),

            // -- BuildSet: {item0, item1, ...} --
            OpCode::BuildSet => self.emit_build_set(op),

            // -- BuildSlice: slice(start, stop, step) --
            // operands: [start, stop, step]   (already declared as molt_slice_new)
            OpCode::BuildSlice => self.emit_build_slice(op),

            // -- GetIter: iter(obj) --
            OpCode::GetIter => self.emit_get_iter(op),

            // -- IterNext: next(iter) -> value (or StopIteration sentinel) --
            OpCode::IterNext => self.emit_iter_next(op),

            // -- ForIter: advance iterator, returning next value or exhaustion sentinel --
            OpCode::ForIter => self.emit_for_iter(op),

            // -- Async / generator / channel state machine --
            OpCode::AllocTask => self.emit_alloc_task(op),
            OpCode::StateSwitch => self.emit_state_switch(),
            OpCode::ClosureLoad => self.emit_closure_load(op),
            OpCode::ClosureStore => self.emit_closure_store(op),
            OpCode::FrameContextSet => self.emit_frame_context_set(op),
            OpCode::StateYield | OpCode::StateTransition => panic!(
                "{}: the shared terminal drop pass must expose it as explicit activation exits before LLVM emission",
                self.func.name
            ),
            OpCode::StateSet => self.emit_state_set(op),
            OpCode::IsPending => self.emit_is_pending(op),
            OpCode::TaskWait => self.emit_task_wait(op),
            OpCode::Yield => self.emit_yield(op),
            OpCode::YieldFrom => self.emit_yield_from(op),
            OpCode::Raise => {
                // `molt_raise` borrows the exception object and returns the
                // immortal None with that exception pending.
                let raise_fn = self.ensure_runtime_import(RuntimeFn::Raise);
                self.emit_borrowed_runtime_call(
                    op,
                    raise_fn,
                    &[RuntimeArg::Operand(op.operands[0])],
                    RuntimeResultCustody::Unowned,
                    "raise",
                    "raise",
                );
            }

            // -- WarnStderr: side-effecting diagnostic emit --
            OpCode::WarnStderr => {
                let warn_fn = self.ensure_runtime_import(RuntimeFn::WarnStderr);
                self.emit_borrowed_runtime_call(
                    op,
                    warn_fn,
                    &[RuntimeArg::Operand(op.operands[0])],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::Void),
                    "warn_stderr",
                    "warn_stderr",
                );
            }

            // -- ExceptionPending: read the runtime exception-pending flag as
            //    an exact i1 boolean (`molt_exception_pending() != 0`). Produced
            //    by `loop_break_if_exception` and consumed as the condition of
            //    the loop-exit CondBranch that breaks an iterator-consumer loop
            //    on a mid-iteration raise.  Non-foldable: it observes mutable
            //    runtime state, so the value (and the break) always survive.
            OpCode::ExceptionPending => {
                let raw = self.emit_exception_pending_word("exc_pending");
                if let Some(&result_id) = op.results.first() {
                    let pending = self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            raw,
                            self.backend.context.i64_type().const_zero(),
                            "exc_pending_bool",
                        )
                        .unwrap();
                    self.values.set_value(result_id, pending.into());
                    self.values.set_type(result_id, TirType::Bool);
                }
            }

            // -- AsyncWorkPending: test the thread's pending-work word. It
            //    services nothing; a promoted module loop branches on it to
            //    the unpromoted loop, whose poll runs the work.
            OpCode::AsyncWorkPending => {
                let pending = self.emit_async_work_pending();
                if let Some(&result_id) = op.results.first() {
                    self.values.set_value(result_id, pending.into());
                    self.values.set_type(result_id, TirType::Bool);
                }
            }

            // -- FunctionDefaultsVersion: read a function object's
            //    __defaults__/__kwdefaults__ mutation version stamp as a boxed
            //    inline int (`molt_function_defaults_version(func)`).  Produced
            //    by the compile-time defaults-devirt deopt guard and consumed by
            //    its `== 0` compare (baked literal vs live read).  Non-foldable:
            //    it observes mutable runtime state, so the read always survives.
            OpCode::FunctionDefaultsVersion => {
                let func_id = *op
                    .operands
                    .first()
                    .expect("FunctionDefaultsVersion requires a function operand");
                let ver_fn = self.ensure_runtime_import(RuntimeFn::FunctionDefaultsVersion);
                // Returns a NaN-boxed inline int; the consuming `== 0` compare
                // routes through the boxed-int equality path.
                self.emit_borrowed_runtime_call(
                    op,
                    ver_fn,
                    &[RuntimeArg::Operand(func_id)],
                    Self::canonical_boxed_return(RuntimeFn::FunctionDefaultsVersion, 1),
                    "function_defaults_version",
                    "func_defaults_version",
                );
            }

            // -- CheckException: transfer to its explicit exceptional successor --
            OpCode::CheckException => {
                let Some(target_label) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    self.record_fatal(
                        "check_exception requires an explicit exceptional successor label",
                    );
                    return;
                };
                let result = if op.is_async_work_poll() {
                    self.emit_async_work_poll(false, op.frame_location())
                } else {
                    self.emit_exception_pending_word("check_exc").into()
                };
                if let Some(&result_id) = op.results.first() {
                    self.values.set_value(result_id, result);
                    self.values.set_type(result_id, TirType::DynBox);
                }
                let Some(target_block_id) = self
                    .func
                    .label_id_map
                    .iter()
                    .find_map(|(bid, label)| (*label == target_label).then_some(BlockId(*bid)))
                else {
                    self.record_fatal(format!(
                        "check_exception target label {} is not present in label map",
                        target_label
                    ));
                    return;
                };
                let Some(&target_bb) = self.block_map.get(&target_block_id) else {
                    self.record_fatal(format!(
                        "check_exception target block {:?} is not present in LLVM block map",
                        target_block_id
                    ));
                    return;
                };
                let continue_bb = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("check_exc_cont{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(continue_bb);
                let pending = self.ensure_i64(result);
                let has_exception = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        pending,
                        self.backend.context.i64_type().const_zero(),
                        "check_exc_pending",
                    )
                    .unwrap();
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                let edge = op
                    .exception_edge_kind()
                    .expect("verified exceptional edge kind");
                let exception_bb = match edge {
                    molt_ir::tir::op_kinds_generated::ExceptionEdgeKind::Origin => {
                        let origin = self.backend.context.append_basic_block(
                            self.llvm_fn,
                            &format!("check_exc_origin{}", self.synthetic_block_counter),
                        );
                        self.synthetic_block_counter += 1;
                        self.all_llvm_blocks.push(origin);
                        self.backend.builder.position_at_end(origin);
                        if let Some(location) = op.frame_location() {
                            // The site's own LocationId; the normal path stores nothing.
                            self.store_frame_location(location);
                        }
                        let homes = if let Some(slot) = self.frame_homes {
                            self.backend
                                .builder
                                .build_load(self.backend.context.i64_type(), slot, "origin_homes")
                                .unwrap()
                                .into_int_value()
                        } else {
                            self.backend.context.i64_type().const_zero()
                        };
                        let service = self.ensure_runtime_import(RuntimeFn::ExceptionFrameOrigin);
                        self.backend
                            .builder
                            .build_execution_call(service, &[homes.into()], "origin_capture")
                            .unwrap();
                        self.record_branch_args(
                            source_block,
                            origin,
                            target_block_id,
                            "check-exception-origin-edge",
                            &op.operands,
                        );
                        self.record_llvm_edge(origin, target_bb);
                        self.backend
                            .builder
                            .build_unconditional_branch(target_bb)
                            .unwrap();
                        origin
                    }
                    molt_ir::tir::op_kinds_generated::ExceptionEdgeKind::Unwind => {
                        self.record_branch_args(
                            source_block,
                            branch_from_bb,
                            target_block_id,
                            "check-exception-unwind-edge",
                            &op.operands,
                        );
                        target_bb
                    }
                };
                self.backend.builder.position_at_end(branch_from_bb);
                self.record_llvm_edge(branch_from_bb, exception_bb);
                self.record_llvm_edge(branch_from_bb, continue_bb);
                self.backend
                    .builder
                    .build_conditional_branch(has_exception, exception_bb, continue_bb)
                    .unwrap();
                self.backend.builder.position_at_end(continue_bb);
            }

            // -- Import: import module by name --
            OpCode::Import => {
                let result_id = op.results[0];
                let import_fn = self.ensure_runtime_import(RuntimeFn::ModuleImport);
                let result = if let Some(&name_id) = op.operands.first() {
                    self.borrowed_runtime_call_value(
                        import_fn,
                        &[RuntimeArg::Operand(name_id)],
                        false,
                        "import",
                        "import",
                    )
                } else {
                    let module_name = ["module", "s_value", "_var"]
                        .iter()
                        .find_map(|key| match op.attrs.get(*key) {
                            Some(AttrValue::Str(name)) => Some(name.as_str()),
                            _ => None,
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "Import op missing module operand/attr in {}",
                                self.func.name
                            )
                        });
                    self.with_owned_name(module_name, |this, name_i64| {
                        this.backend
                            .builder
                            .build_execution_call(import_fn, &[name_i64.into()], "import")
                            .unwrap()
                            .try_as_basic_value()
                            .unwrap_basic()
                    })
                    .into_int_value()
                };
                self.values.set_value(result_id, result.into());
                self.values.set_type(result_id, TirType::DynBox);
            }

            // -- ImportFrom: from module import name --
            // operands: [module, attr_name]
            OpCode::ImportFrom => self.emit_module_runtime_call(op, RuntimeFn::ModuleGetAttr, 2),

            // -- ModuleCacheGet: module-cache lookup by name --
            // operands: [module_name]
            OpCode::ModuleCacheGet => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleCacheGet, 1)
            }

            // -- ModuleCacheSet: module-cache mutation by name --
            // operands: [module_name, module]
            OpCode::ModuleCacheSet => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleCacheSet, 2)
            }

            // -- ModuleCacheDel: module-cache deletion by name --
            // operands: [module_name]
            OpCode::ModuleCacheDel => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleCacheDel, 1)
            }

            // -- ModuleGetAttr: module attribute read --
            // operands: [module, attr_name]
            OpCode::ModuleGetAttr => self.emit_module_runtime_call(op, RuntimeFn::ModuleGetAttr, 2),

            // -- ModuleImportFrom: `from M import name` binding --
            // operands: [module, attr_name]. CPython IMPORT_FROM semantics:
            // ImportError (not AttributeError) on miss, with a sys.modules
            // submodule fallback (see molt_module_import_from).
            OpCode::ModuleImportFrom => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleImportFrom, 2)
            }

            // -- ModuleGetGlobal: CPython-style module global lookup --
            // operands: [module, global_name]
            OpCode::ModuleGetGlobal => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleGetGlobal, 2)
            }
            // -- Module global slot sites (call_anatomy.md 4.5) --
            OpCode::GlobalSlotBase => self.emit_global_slot_base(op),
            OpCode::GlobalSlotGet => self.emit_global_slot_get(op),

            // -- ModuleGetName: module name/attribute lookup helper --
            // operands: [module, attr_name]
            OpCode::ModuleGetName => self.emit_module_runtime_call(op, RuntimeFn::ModuleGetName, 2),

            // -- ModuleSetAttr: module attribute mutation --
            // operands: [module, attr_name, value]; the module dict retains the
            // value, so a box minted for a raw value is released after the store.
            OpCode::ModuleSetAttr => self.emit_module_runtime_call(op, RuntimeFn::ModuleSetAttr, 3),

            // -- ModuleDelGlobal: CPython-style module global deletion --
            // operands: [module, global_name]
            OpCode::ModuleDelGlobal => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleDelGlobal, 2)
            }
            OpCode::ModuleDelGlobalIfPresent => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleDelGlobalIfPresent, 2)
            }

            // -- SCF dialect ops --
            // These must reach LLVM as real CFG, loop, and phi structure. The
            // old runtime-delegate names were never exported by molt-runtime, so
            // keeping them as declarations converted missing lowering into link
            // failures. Fail closed here until the structured lowering pass owns
            // the full CFG form.
            OpCode::ScfIf => self.record_unlowered_structured_op(
                op,
                "lower structured if/else regions into LLVM CFG before codegen",
            ),
            OpCode::ScfFor => self.record_unlowered_structured_op(
                op,
                "lower counted or iterator loops into LLVM CFG before codegen",
            ),
            OpCode::ScfWhile => self.record_unlowered_structured_op(
                op,
                "lower while regions into LLVM CFG before codegen",
            ),
            OpCode::ScfYield => self.record_unlowered_structured_op(
                op,
                "thread SCF region results through block arguments or phi nodes before codegen",
            ),

            // Path-local region metadata does not mutate runtime state. The
            // preserved exception_push/pop and stack operations own that state;
            // explicit exception edges own transfer, independent of block order.
            OpCode::TryStart | OpCode::TryEnd => {}
            OpCode::IterNextUnboxed => self.emit_iter_next_unboxed(op),
            OpCode::UnpackSequence => {
                assert!(
                    self.lower_preserved_container_op(op, OpKind::UnpackSequence),
                    "verified UnpackSequence must lower through the canonical runtime ABI"
                );
            }
            OpCode::ObjectNewBound => {
                let Some(&class_id) = op.operands.first() else {
                    panic!("{:?} requires class operand", op.opcode);
                };
                let new_fn = self.ensure_runtime_import(RuntimeFn::ObjectNewBound);
                self.emit_borrowed_runtime_call(
                    op,
                    new_fn,
                    &[RuntimeArg::Operand(class_id)],
                    Self::canonical_boxed_return(RuntimeFn::ObjectNewBound, 1),
                    "object_new_bound",
                    "object_new_bound",
                );
            }
            OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }

    /// Module and import operations are positional boxed runtime calls with
    /// canonical rows: every operand is borrowed through one custody and the
    /// owned result is bound or released.
    fn emit_module_runtime_call(&mut self, op: &TirOp, function: RuntimeFn, arity: usize) {
        let callee = self.ensure_runtime_import(function);
        let args: Vec<RuntimeArg<'ctx>> = op.operands[..arity]
            .iter()
            .map(|&operand| RuntimeArg::Operand(operand))
            .collect();
        self.emit_borrowed_runtime_call(
            op,
            callee,
            &args,
            Self::canonical_boxed_return(function, arity),
            "module_call",
            function.symbol(),
        );
    }
}
