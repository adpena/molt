use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        match op.opcode {
            // -- Constants --
            OpCode::ConstInt => self.emit_const_int(op),
            OpCode::ConstFloat => self.emit_const_float(op),
            OpCode::ConstBool => self.emit_const_bool(op),
            OpCode::ConstNone => self.emit_const_none(op),
            OpCode::ConstStr => self.emit_const_str(op),
            OpCode::ConstBigInt => self.emit_const_bigint(op),
            OpCode::ConstBytes => self.emit_const_bytes(op),

            // -- Arithmetic (type-specialized) --
            OpCode::Add | OpCode::InplaceAdd => self.emit_binary_arith(op, "add"),
            OpCode::CheckedAdd => self.emit_checked_add(op),
            OpCode::CheckedMul => self.emit_checked_mul(op),
            OpCode::Sub | OpCode::InplaceSub => self.emit_binary_arith(op, "sub"),
            OpCode::Mul | OpCode::InplaceMul => self.emit_binary_arith(op, "mul"),
            OpCode::Div => self.emit_binary_arith(op, "div"),
            OpCode::FloorDiv => self.emit_binary_arith(op, "floordiv"),
            OpCode::Mod => self.emit_binary_arith(op, "mod"),
            OpCode::Pow => self.emit_binary_arith(op, "pow"),

            // -- Unary --
            OpCode::Neg => self.emit_unary(op, "neg"),
            OpCode::Pos => {
                // Pos is identity for numeric types.
                let result_id = op.results[0];
                let operand = op.operands[0];
                let val = self.values[&operand];
                let ty = self.value_types[&operand].clone();
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, ty);
            }
            OpCode::Not => self.emit_unary(op, "not"),

            // -- Comparison (type-specialized) --
            OpCode::Eq => self.emit_comparison(op, "eq"),
            OpCode::Ne => self.emit_comparison(op, "ne"),
            OpCode::Lt => self.emit_comparison(op, "lt"),
            OpCode::Le => self.emit_comparison(op, "le"),
            OpCode::Gt => self.emit_comparison(op, "gt"),
            OpCode::Ge => self.emit_comparison(op, "ge"),
            OpCode::Is | OpCode::IsNot => self.emit_identity(op),
            OpCode::In | OpCode::NotIn => self.emit_containment(op),

            // -- Bitwise --
            OpCode::BitAnd => self.emit_bitwise(op, "bit_and"),
            OpCode::BitOr => self.emit_bitwise(op, "bit_or"),
            OpCode::BitXor => self.emit_bitwise(op, "bit_xor"),
            OpCode::BitNot => self.emit_unary(op, "invert"),
            OpCode::Shl => self.emit_bitwise(op, "lshift"),
            OpCode::Shr => self.emit_bitwise(op, "rshift"),

            // -- Boolean --
            OpCode::And | OpCode::Or => {
                // Frontend BoolOp lowering uses And/Or ops to produce the
                // selected operand value inside already-structured control flow.
                // At this stage we must preserve Python operand-selection
                // semantics, not bitwise semantics.
                let result_id = op.results[0];
                let lhs = self.resolve(op.operands[0]);
                let rhs = self.resolve(op.operands[1]);
                let lhs_ty = self
                    .value_types
                    .get(&op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let rhs_ty = self
                    .value_types
                    .get(&op.operands[1])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let lhs_i64 = self.ensure_i64(lhs);
                let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                let truthy = self
                    .backend
                    .builder
                    .build_call(truthy_fn, &[lhs_i64.into()], "truthy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let cond_i1 = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        truthy,
                        self.backend.context.i64_type().const_zero(),
                        "boolop_cond",
                    )
                    .unwrap();
                let lhs_bits = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_bits = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let selected = if op.opcode == OpCode::And {
                    self.backend
                        .builder
                        .build_select(cond_i1, rhs_bits, lhs_bits, "bool_and")
                        .unwrap()
                } else {
                    self.backend
                        .builder
                        .build_select(cond_i1, lhs_bits, rhs_bits, "bool_or")
                        .unwrap()
                };
                if crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
                    op.opcode,
                ) {
                    let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                    self.backend
                        .builder
                        .build_call(
                            inc_fn,
                            &[selected.into_int_value().into()],
                            "boolop_selected_inc_ref",
                        )
                        .unwrap();
                }
                self.values.insert(result_id, selected);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::Bool => {
                let result_id = op.results[0];
                let operand_id = op.operands[0];
                let operand = self.resolve(operand_id);
                let operand_ty = self
                    .value_types
                    .get(&operand_id)
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
                        .build_float_compare(
                            inkwell::FloatPredicate::ONE,
                            operand.into_float_value(),
                            self.backend.context.f64_type().const_float(0.0),
                            "bool_f64",
                        )
                        .unwrap(),
                    _ => {
                        let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                        let truthy = self
                            .backend
                            .builder
                            .build_call(
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
                self.values.insert(result_id, bool_val.into());
                self.value_types.insert(result_id, TirType::Bool);
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
                self.values.insert(result_id, val);
                let ty = self
                    .value_types
                    .get(&op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                self.value_types.insert(result_id, ty);
            }

            // -- Refcount --
            OpCode::IncRef => {
                let val = self.resolve(op.operands[0]);
                let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                let bits = self.ensure_i64(val);
                self.backend
                    .builder
                    .build_call(inc_fn, &[bits.into()], "")
                    .unwrap();
                // IncRef has no result, but if it does, pass through.
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    let ty = self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    self.value_types.insert(op.results[0], ty);
                }
            }
            // Python lifetime boundary (#58): when a `del`/rebind/scope-exit
            // marker survives to LLVM, it is the release authority for that
            // named-local owner. The drop phase may rewrite some markers to
            // `DecRef`; any marker left here must still lower to the same
            // runtime release, matching the native backend's direct
            // `del_boundary` arm.
            OpCode::DelBoundary => {
                let val = self.resolve(op.operands[0]);
                let bits = self.ensure_i64(val);
                let dec_fn = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
                self.backend
                    .builder
                    .build_call(dec_fn, &[bits.into()], "")
                    .unwrap();
            }
            OpCode::DecRef => {
                let val = self.resolve(op.operands[0]);
                let dec_fn = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
                let bits = self.ensure_i64(val);
                self.backend
                    .builder
                    .build_call(dec_fn, &[bits.into()], "")
                    .unwrap();
            }

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
                    let ty = self
                        .value_types
                        .get(&missing)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.insert(result_id, val);
                        self.value_types.insert(result_id, ty.clone());
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
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
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
            OpCode::Copy => {
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });
                if let Some(kind) = original_kind
                    && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
                // TERMINAL FAIL-LOUD STATE (preserved-op passthrough class),
                // tied to the `CopyLowering` single source of truth.
                //
                // Reaching here means this `OpCode::Copy` carries an
                // `_original_kind` that `lower_preserved_simpleir_op` did NOT
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
                // whose `_original_kind` is a `CopyLowering::FreshValue`
                // (`alias_analysis::copy_kind_mints_fresh_owned_ref`). If such a
                // fresh-owned producer reached codegen as a silent operand-0
                // passthrough, the result would (a) be the wrong value AND (b)
                // alias operand 0 — which the drop pass then DOUBLE-FREES. The gate
                // therefore consults that classifier on every fatal so the table
                // and the backend cannot drift: a `FreshValue` reaching here is the
                // forbidden drift (a fresh-value op missing its explicit LLVM arm),
                // and the diagnostic names it as such; any other `_original_kind`
                // gets the general terminal message with operand/result counts.
                // Closing a kind = add an arm to `lower_preserved_simpleir_op` (or,
                // if `molt_<kind>` is a real boxed runtime intrinsic, the generic
                // fallback already covers it). See the `CopyLowering` docs and
                // `tests::copy_lowering_classes_are_total_and_disjoint`.
                if let Some(kind) = original_kind {
                    if crate::tir::passes::alias_analysis::copy_kind_reaches_no_incref_passthrough(
                        Some(kind),
                    ) {
                        // Not a `FreshValue` (a transparent-alias / inert-marker
                        // kind whose `molt_<kind>` intrinsic is also absent): the
                        // partner's general terminal state. Still fail loud — an
                        // unhandled `_original_kind` is never a sound passthrough.
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
                        // A `CopyLowering::FreshValue` reached the passthrough: the
                        // exact classifier?backend drift this gate exists to catch.
                        self.record_fatal(format!(
                            "fresh-value SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile AND make the result alias operand 0 (a \
                             drop-insertion double-free); it is in \
                             `alias_analysis::copy_kind_mints_fresh_owned_ref` so it \
                             MUST have a `lower_preserved_simpleir_op` arm (the \
                             classifier and the LLVM lowering have drifted)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    }
                    return;
                }

                // `_original_kind == None`: a genuine SSA value copy
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
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // Standard copy: pass through first operand.
                    let val = self.resolve(op.operands[0]);
                    let ty = self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.insert(result_id, val);
                        self.value_types.insert(result_id, ty.clone());
                    }
                }
            }

            // -- Allocation --
            OpCode::Alloc => {
                let result_id = op.results[0];
                let size = self.resolve(op.operands[0]);
                let size_i64 = self.ensure_i64(size);
                let alloc_fn = self.backend.module.get_function("molt_alloc").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(alloc_fn, &[size_i64.into()], "alloc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // -- Calls --
            OpCode::CallMethod => self.emit_call_method(op),

            OpCode::CallMethodIc => self.emit_call_method_ic(op),

            OpCode::CallSuperMethodIc => self.emit_call_super_method_ic(op),

            OpCode::CallBuiltin => self.emit_call_builtin(op),

            // -- OrdAt: fused ord(container[index]) --
            OpCode::OrdAt => {
                if op.operands.len() < 2 {
                    return;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let index_bits = self.materialize_dynbox_operand(op.operands[1]);
                let ord_at_fn = self.ensure_runtime_i64_fn("molt_ord_at", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_at_fn, &[obj_bits.into(), index_bits.into()], "ord_at")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- StackAlloc: alloca for stack-resident slots --
            // attrs: { "type": "i64" | "dynbox" | ... }
            // result: pointer stored as i64 (ptrtoint)
            OpCode::StackAlloc => {
                let i64_ty = self.backend.context.i64_type();
                let ptr = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "stack_slot")
                    .unwrap();
                let ptr_as_i64 = self
                    .backend
                    .builder
                    .build_ptr_to_int(ptr, i64_ty, "slot_ptr")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, ptr_as_i64.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- Free: stack-allocated slots are freed automatically — no-op --
            OpCode::Free => {
                // Stack memory is reclaimed by the function epilogue; nothing to emit.
            }

            // -- BuildList: [item0, item1, ...] --
            // Strategy: list_builder_new(capacity) + append + finish.
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
            OpCode::StateYield => self.emit_state_yield(op),
            OpCode::StateTransition => self.emit_state_transition(op),
            OpCode::ChanSendYield => self.emit_chan_send_yield(op),
            OpCode::ChanRecvYield => self.emit_chan_recv_yield(op),
            OpCode::Yield => self.emit_yield(op),
            OpCode::YieldFrom => self.emit_yield_from(op),
            OpCode::Raise => {
                let exc = self.resolve(op.operands[0]);
                let exc_i64 = self.ensure_i64(exc);
                let raise_fn = self.backend.module.get_function("molt_raise").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(raise_fn, &[exc_i64.into()], "raise")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], result);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }

            // -- WarnStderr: side-effecting diagnostic emit --
            OpCode::WarnStderr => {
                let msg = self.resolve(op.operands[0]);
                let msg_i64 = self.ensure_i64(msg);
                let warn_fn = self
                    .backend
                    .module
                    .get_function("molt_warn_stderr")
                    .unwrap();
                self.backend
                    .builder
                    .build_call(warn_fn, &[msg_i64.into()], "warn_stderr")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ExceptionPending: read the runtime exception-pending flag as
            //    a raw i64 boolean (`molt_exception_pending() != 0`).  Produced
            //    by `loop_break_if_exception` and consumed as the condition of
            //    the loop-exit CondBranch that breaks an iterator-consumer loop
            //    on a mid-iteration raise.  Non-foldable: it observes mutable
            //    runtime state, so the value (and the break) always survive.
            OpCode::ExceptionPending => {
                let pend_fn = self
                    .backend
                    .module
                    .get_function("molt_exception_pending")
                    .unwrap_or_else(|| {
                        let i64_ty = self.backend.context.i64_type();
                        let fn_ty = i64_ty.fn_type(&[], false);
                        self.backend.module.add_function(
                            "molt_exception_pending",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    });
                let raw = self
                    .backend
                    .builder
                    .build_call(pend_fn, &[], "exc_pending")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, raw);
                    // `molt_exception_pending` returns a raw i64 0/1 (NOT a
                    // NaN-boxed bool), so the consuming CondBranch must test it
                    // with `!= 0` (the TirType::I64 path) rather than routing it
                    // through `molt_is_truthy`, which would misinterpret the bit
                    // pattern of `1` as a boxed value.
                    self.value_types.insert(result_id, TirType::I64);
                }
            }

            // -- FunctionDefaultsVersion: read a function object's
            //    __defaults__/__kwdefaults__ mutation version stamp as a boxed
            //    inline int (`molt_function_defaults_version(func)`).  Produced
            //    by the compile-time defaults-devirt deopt guard and consumed by
            //    its `== 0` compare (baked literal vs live read).  Non-foldable:
            //    it observes mutable runtime state, so the read always survives.
            OpCode::FunctionDefaultsVersion => {
                let ver_fn = self.ensure_runtime_i64_fn("molt_function_defaults_version", 1);
                let func_val = op
                    .operands
                    .first()
                    .and_then(|id| self.values.get(id).copied())
                    .expect("FunctionDefaultsVersion operand not materialized");
                let raw = self
                    .backend
                    .builder
                    .build_call(ver_fn, &[func_val.into()], "func_defaults_version")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, raw);
                    // Returns a NaN-boxed inline int; the consuming `== 0`
                    // compare routes through the boxed-int equality path.
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- CheckException: inspect the current exception state --
            OpCode::CheckException => {
                let check_fn = self
                    .backend
                    .module
                    .get_function("molt_exception_pending")
                    .unwrap_or_else(|| {
                        let i64_ty = self.backend.context.i64_type();
                        let fn_ty = i64_ty.fn_type(&[], false);
                        self.backend.module.add_function(
                            "molt_exception_pending",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    });
                let result = self
                    .backend
                    .builder
                    .build_call(check_fn, &[], "check_exc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let Some(target_label) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return;
                };
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
                self.record_branch_args(
                    source_block,
                    branch_from_bb,
                    target_block_id,
                    "check-exception-edge",
                    &op.operands,
                );
                self.record_llvm_edge(branch_from_bb, target_bb);
                self.record_llvm_edge(branch_from_bb, continue_bb);
                self.backend
                    .builder
                    .build_conditional_branch(has_exception, target_bb, continue_bb)
                    .unwrap();
                self.backend.builder.position_at_end(continue_bb);
            }

            // -- Import: import module by name --
            OpCode::Import => {
                let result_id = op.results[0];
                let name = if let Some(&name_id) = op.operands.first() {
                    self.resolve(name_id)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("module") {
                    self.intern_string_const(module_name)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("s_value") {
                    self.intern_string_const(module_name)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("_var") {
                    self.intern_string_const(module_name)
                } else {
                    panic!(
                        "Import op missing module operand/attr in {}",
                        self.func.name
                    );
                };
                let name_i64 = self.ensure_i64(name);
                let import_fn = self
                    .backend
                    .module
                    .get_function("molt_module_import")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(import_fn, &[name_i64.into()], "import")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // -- ImportFrom: from module import name --
            // operands: [module, attr_name]
            OpCode::ImportFrom => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_attr",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleCacheGet: module-cache lookup by name --
            // operands: [module_name]
            OpCode::ModuleCacheGet => {
                let result_id = op.results[0];
                let get_fn = self.ensure_runtime_i64_fn("molt_module_cache_get", 1);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(get_fn, &[name_bits.into()], "module_cache_get")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // -- ModuleCacheSet: module-cache mutation by name --
            // operands: [module_name, module]
            OpCode::ModuleCacheSet => {
                let set_fn = self.ensure_runtime_i64_fn("molt_module_cache_set", 2);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let module_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[name_bits.into(), module_bits.into()],
                        "module_cache_set",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleCacheDel: module-cache deletion by name --
            // operands: [module_name]
            OpCode::ModuleCacheDel => {
                let del_fn = self.ensure_runtime_i64_fn("molt_module_cache_del", 1);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(del_fn, &[name_bits.into()], "module_cache_del")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleGetAttr: module attribute read --
            // operands: [module, attr_name]
            OpCode::ModuleGetAttr => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_attr",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleImportFrom: `from M import name` binding --
            // operands: [module, attr_name]. CPython IMPORT_FROM semantics:
            // ImportError (not AttributeError) on miss, with a sys.modules
            // submodule fallback (see molt_module_import_from).
            OpCode::ModuleImportFrom => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_import_from",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleGetGlobal: CPython-style module global lookup --
            // operands: [module, global_name]
            OpCode::ModuleGetGlobal => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_global",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleGetName: module name/attribute lookup helper --
            // operands: [module, attr_name]
            OpCode::ModuleGetName => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_name",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleSetAttr: module attribute mutation --
            // operands: [module, attr_name, value]
            OpCode::ModuleSetAttr => {
                let set_fn = self.ensure_runtime_i64_fn("molt_module_set_attr", 3);
                let module_bits = self.materialize_dynbox_operand(op.operands[0]);
                let attr_bits = self.materialize_dynbox_operand(op.operands[1]);
                let val_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[module_bits.into(), attr_bits.into(), val_bits.into()],
                        "module_set_attr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- ModuleDelGlobal: CPython-style module global deletion --
            // operands: [module, global_name]
            OpCode::ModuleDelGlobal => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_del_global",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ModuleDelGlobalIfPresent => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_del_global_if_present",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // -- SCF dialect ops --
            // Structured control flow ops are desugared into LLVM basic blocks.
            // ScfIf uses conditional branches to then/else blocks with a merge phi.
            // ScfFor/ScfWhile delegate to runtime helpers since full loop lowering
            // requires loop analysis infrastructure (induction variable detection,
            // trip count computation) that lives in a separate pass.
            // ScfYield maps to a runtime call that returns its value.
            OpCode::ScfIf => {
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();

                // Resolve condition and coerce to i1.
                let cond_i64 = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                let truthy_result = self
                    .backend
                    .builder
                    .build_call(truthy_fn, &[cond_i64.into()], "scf_if_truthy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let cond_i1 = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        truthy_result.into_int_value(),
                        i64_ty.const_int(0, false),
                        "scf_if_cond",
                    )
                    .unwrap();

                // Resolve then/else function operands.
                let then_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let else_fn_bits = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };

                // Create basic blocks for then, else, and merge.
                let current_fn = self.llvm_fn;
                let then_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_then");
                let else_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_else");
                let merge_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_merge");
                self.all_llvm_blocks.push(then_bb);
                self.all_llvm_blocks.push(else_bb);
                self.all_llvm_blocks.push(merge_bb);

                self.backend
                    .builder
                    .build_conditional_branch(cond_i1, then_bb, else_bb)
                    .unwrap();

                // Then block: call then_fn via molt_call_0 and branch to merge.
                self.backend.builder.position_at_end(then_bb);
                let call0_fn = self.backend.module.get_function("molt_call_0").unwrap();
                let then_result = self
                    .backend
                    .builder
                    .build_call(call0_fn, &[then_fn_bits.into()], "scf_then_result")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                let then_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Else block: call else_fn via molt_call_0 and branch to merge.
                self.backend.builder.position_at_end(else_bb);
                let else_result = self
                    .backend
                    .builder
                    .build_call(call0_fn, &[else_fn_bits.into()], "scf_else_result")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                let else_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Merge block: phi node selects then/else result.
                self.backend.builder.position_at_end(merge_bb);
                let phi = self
                    .backend
                    .builder
                    .build_phi(i64_ty, "scf_if_phi")
                    .unwrap();
                phi.add_incoming(&[(&then_result, then_exit_bb), (&else_result, else_exit_bb)]);
                let phi_val = phi.as_basic_value();

                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, phi_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfFor => {
                // ScfFor delegates to the runtime: full loop lowering requires
                // induction variable detection and trip count analysis that runs
                // as a separate TIR pass before LLVM lowering.
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let lb = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let ub = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let step = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(1, false)
                };
                let body_fn_bits = if op.operands.len() > 3 {
                    let v = self.resolve(op.operands[3]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let scf_for_fn = self.backend.module.get_function("molt_scf_for").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        scf_for_fn,
                        &[lb.into(), ub.into(), step.into(), body_fn_bits.into()],
                        "scf_for",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfWhile => {
                // ScfWhile delegates to the runtime: full loop lowering requires
                // condition hoisting and break/continue analysis.
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let cond_fn_bits = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let body_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let scf_while_fn = self.backend.module.get_function("molt_scf_while").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        scf_while_fn,
                        &[cond_fn_bits.into(), body_fn_bits.into()],
                        "scf_while",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfYield => {
                // ScfYield returns its operand value (or None if no operand).
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let val = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                };
                let scf_yield_fn = self.backend.module.get_function("molt_scf_yield").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(scf_yield_fn, &[val.into()], "scf_yield")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // Exception region markers. LLVM still uses polling-based Molt
            // exceptions, but the runtime expects a handler frame to be
            // established around try regions so raise/catch semantics match
            // native and wasm.
            OpCode::TryStart => {
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_stack_enter", 0);
                let baseline = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[], "try_enter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let baseline_slot = self.build_entry_i64_alloca("try_baseline");
                self.backend
                    .builder
                    .build_store(baseline_slot, baseline)
                    .unwrap();
                self.try_stack_baselines.push(baseline_slot);
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, baseline);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::TryEnd => {
                if let Some(baseline_slot) = self.try_stack_baselines.pop() {
                    let exit_fn = self.ensure_runtime_i64_fn("molt_exception_stack_exit", 1);
                    let baseline_bits = self
                        .backend
                        .builder
                        .build_load(
                            self.backend.context.i64_type(),
                            baseline_slot,
                            "try_baseline_load",
                        )
                        .unwrap()
                        .into_int_value();
                    let result = self
                        .backend
                        .builder
                        .build_call(exit_fn, &[baseline_bits.into()], "try_exit")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }
            OpCode::IterNextUnboxed => self.emit_iter_next_unboxed(op),
            OpCode::ObjectNewBound | OpCode::ObjectNewBoundStack => {
                let Some(&class_id) = op.operands.first() else {
                    panic!("{:?} requires class operand", op.opcode);
                };
                let class_bits = self.materialize_dynbox_operand(class_id);
                let result = if let Some(AttrValue::Int(payload_size)) = op.attrs.get("value")
                    && *payload_size > 0
                {
                    let new_fn = self.ensure_runtime_i64_fn("molt_object_new_bound_sized", 2);
                    let size_bits = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(*payload_size as u64, false);
                    self.backend
                        .builder
                        .build_call(
                            new_fn,
                            &[class_bits.into(), size_bits.into()],
                            "object_new_bound_sized",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                } else {
                    let new_fn = self.ensure_runtime_i64_fn("molt_object_new_bound", 1);
                    self.backend
                        .builder
                        .build_call(new_fn, &[class_bits.into()], "object_new_bound")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                };
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }
}
