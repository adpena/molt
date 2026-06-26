use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
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

    /// NaN-box a raw signed `i64`, promoting to a heap BigInt when the value
    /// does not fit the 47-bit inline payload.
    ///
    /// The inline integer representation is a sign-extended 47-bit payload
    /// (range `[-(1<<46), (1<<46)-1]`). An unconditional `raw & INT_MASK | TAG`
    /// silently truncates any value outside that range to 47 bits — the LLVM
    /// integer-overflow miscompile this fixes. Instead we emit a single
    /// fits-inline range check: on the hot path (fits) we box inline; on the
    /// cold path we call `molt_int_from_i64`. This mirrors the native backend's
    /// `ensure_boxed_overflow_safe` and the WASM backend's
    /// `emit_inline_int_range_check` + runtime fallback.
    ///
    /// LLVM's range analysis (SCEV / known-bits) folds the branch away whenever
    /// it can prove `raw` fits inline (e.g. bounded loop induction variables and
    /// constants), so the check is free on values that are statically small.
    ///
    /// This form splits the current block, so callers that must keep the boxed
    /// value as a single SSA value in a fixed block (phi-incoming
    /// materialization, function-return coercion) use [`Self::box_i64_branchless`]
    /// instead.
    pub(super) fn box_i64_overflow_safe(
        &self,
        raw: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        box_i64_overflow_safe_with_builder(
            &self.backend.builder,
            self.backend.context,
            &self.backend.module,
            self.llvm_fn,
            raw,
        )
    }

    /// Branchless overflow-safe integer box: a single `molt_int_from_i64` call
    /// that yields one SSA value and never alters control flow. Used where the
    /// boxed value must be a single value in a fixed block (phi-incoming
    /// materialization, function-return coercion). `molt_int_from_i64` returns
    /// the inline NaN-box for values that fit the 47-bit payload and a heap
    /// BigInt otherwise, so the result matches `box_i64_overflow_safe`.
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

    pub(super) fn materialize_dynbox_bits(
        &self,
        operand: BasicValueEnum<'ctx>,
        operand_ty: &TirType,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        match operand_ty {
            TirType::I64 => {
                let raw = self.ensure_i64(operand);
                self.box_i64_overflow_safe(raw)
            }
            TirType::Bool => {
                let raw = match operand {
                    BasicValueEnum::IntValue(iv) if iv.get_type().get_bit_width() == 1 => self
                        .backend
                        .builder
                        .build_int_z_extend(iv, i64_ty, "zext_bool")
                        .unwrap(),
                    _ => self.ensure_i64(operand),
                };
                self.backend
                    .builder
                    .build_or(
                        raw,
                        i64_ty.const_int(nanbox::QNAN | nanbox::TAG_BOOL, false),
                        "box_bool",
                    )
                    .unwrap()
            }
            TirType::None => i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false),
            TirType::F64 => self
                .backend
                .builder
                .build_bit_cast(operand, i64_ty, "f64_to_i64")
                .unwrap()
                .into_int_value(),
            TirType::DynBox
            | TirType::BigInt
            | TirType::Str
            | TirType::Bytes
            | TirType::List(_)
            | TirType::Dict(_, _)
            | TirType::Iterator(_)
            | TirType::Set(_)
            | TirType::Tuple(_)
            | TirType::UserClass(_)
            | TirType::Ptr(_)
            | TirType::Func(_)
            | TirType::Box(_)
            | TirType::Union(_)
            | TirType::Never => self.ensure_i64(operand),
        }
    }

    pub(super) fn materialize_dynbox_operand(
        &self,
        operand_id: ValueId,
    ) -> inkwell::values::IntValue<'ctx> {
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        self.materialize_dynbox_bits(operand, &operand_ty)
    }

    pub(super) fn build_entry_i64_alloca(&self, name: &str) -> inkwell::values::PointerValue<'ctx> {
        let builder = self.backend.context.create_builder();
        let current_fn = self
            .backend
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_parent())
            .expect("llvm function missing while allocating try baseline");
        let entry = current_fn
            .get_first_basic_block()
            .expect("llvm function missing entry block");
        if let Some(first_instr) = entry.get_first_instruction() {
            builder.position_before(&first_instr);
        } else {
            builder.position_at_end(entry);
        }
        builder
            .build_alloca(self.backend.context.i64_type(), name)
            .unwrap()
    }

    pub(super) fn call_runtime_2_boxed(
        &self,
        name: &str,
        lhs_id: ValueId,
        rhs_id: ValueId,
    ) -> BasicValueEnum<'ctx> {
        let func = self
            .backend
            .module
            .get_function(name)
            .unwrap_or_else(|| panic!("Runtime function '{}' not declared", name));
        let lhs_i64 = self.materialize_dynbox_operand(lhs_id);
        let rhs_i64 = self.materialize_dynbox_operand(rhs_id);
        self.backend
            .builder
            .build_call(func, &[lhs_i64.into(), rhs_i64.into()], name)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    pub(super) fn emit_box(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let boxed: BasicValueEnum<'ctx> = self.materialize_dynbox_bits(operand, &operand_ty).into();

        self.values.insert(result_id, boxed);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    pub(super) fn emit_unbox(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
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
            TirType::I64 // default unbox target
        };

        let i64_ty = self.backend.context.i64_type();
        let raw = self.ensure_i64(operand);

        let unboxed: BasicValueEnum<'ctx> = match &target_ty {
            TirType::I64 => {
                // Extract payload: sign-extend from 47 bits
                let masked = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "payload")
                    .unwrap();
                // Sign extension: if bit 46 is set, fill upper bits
                let sign_bit = self
                    .backend
                    .builder
                    .build_and(
                        raw,
                        i64_ty.const_int(nanbox::INT_SIGN_BIT, false),
                        "sign_bit",
                    )
                    .unwrap();
                let is_neg = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        sign_bit,
                        i64_ty.const_int(0, false),
                        "is_neg",
                    )
                    .unwrap();
                let sign_extend = i64_ty.const_int(!nanbox::INT_MASK, false);
                let extended = self
                    .backend
                    .builder
                    .build_or(masked, sign_extend, "sign_extended")
                    .unwrap();
                let extended_basic: inkwell::values::BasicValueEnum = extended.into();
                let masked_basic: inkwell::values::BasicValueEnum = masked.into();

                self.backend
                    .builder
                    .build_select(is_neg, extended_basic, masked_basic, "unbox_i64")
                    .unwrap()
            }
            TirType::F64 => {
                // Bitcast i64 back to f64.
                let f64_ty = self.backend.context.f64_type();
                self.backend
                    .builder
                    .build_bit_cast(raw, f64_ty, "unbox_f64")
                    .unwrap()
            }
            TirType::Bool => {
                // Extract lowest bit
                let one = i64_ty.const_int(1, false);
                let bit = self
                    .backend
                    .builder
                    .build_and(raw, one, "bool_bit")
                    .unwrap();
                let bool_val = self
                    .backend
                    .builder
                    .build_int_truncate(bit, self.backend.context.bool_type(), "unbox_bool")
                    .unwrap();
                bool_val.into()
            }
            _ => raw.into(),
        };

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
                if values.is_empty() {
                    // Return void-equivalent (None sentinel for Python functions)
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let ret_val = self.backend.context.i64_type().const_int(none_bits, false);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                } else if values.len() == 1 {
                    let val = self.resolve(values[0]);
                    let ret_ty = lower_type(self.backend.context, &self.func.return_type);
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
                    let ret_val =
                        self.coerce_to_tir_type(val, &val_ty, &self.func.return_type, current_bb);
                    let ret_val = self.coerce_to_type(ret_val, ret_ty, current_bb);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                } else {
                    // Multi-value return: pack into struct.
                    // For now, just return the first value.
                    let val = self.resolve(values[0]);
                    let ret_ty = lower_type(self.backend.context, &self.func.return_type);
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
                    let ret_val =
                        self.coerce_to_tir_type(val, &val_ty, &self.func.return_type, current_bb);
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
                    let source_tir_ty = self
                        .func
                        .param_types
                        .get(*arg_index)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
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
            TirType::I64 => {
                let masked = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "payload")
                    .unwrap();
                let sign_test = self
                    .backend
                    .builder
                    .build_and(
                        masked,
                        i64_ty.const_int(nanbox::INT_SIGN_BIT, false),
                        "sign_test",
                    )
                    .unwrap();
                let is_neg = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        sign_test,
                        i64_ty.const_zero(),
                        "is_neg",
                    )
                    .unwrap();
                let sign_extend = i64_ty.const_int(!nanbox::INT_MASK, false);
                let extended = self
                    .backend
                    .builder
                    .build_or(masked, sign_extend, "sign_extend")
                    .unwrap();
                self.backend
                    .builder
                    .build_select(is_neg, extended, masked, "unbox_i64")
                    .unwrap()
            }
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
        if !is_classified_runtime_import(name, param_count, return_abi) {
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

    pub(super) fn raw_string_const_ptr_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let name_bytes = s.as_bytes();
        let global = self.backend.module.add_global(
            self.backend
                .context
                .i8_type()
                .array_type(name_bytes.len() as u32),
            None,
            &format!(
                "__guard_attr_str_{}_{}",
                self.const_str_counter,
                s.replace(|c: char| !c.is_alphanumeric(), "_")
            ),
        );
        self.const_str_counter += 1;
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&self.backend.context.const_string(name_bytes, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);
        let ptr_bits = self
            .backend
            .builder
            .build_ptr_to_int(global.as_pointer_value(), i64_ty, "guard_attr_ptr")
            .unwrap();
        let len_bits = i64_ty.const_int(name_bytes.len() as u64, false);
        (ptr_bits, len_bits)
    }

    pub(super) fn emit_task_new_with_payload(
        &mut self,
        poll_addr: inkwell::values::IntValue<'ctx>,
        closure_size: i64,
        kind_bits: i64,
        payload_base: i32,
        payload_operands: &[ValueId],
        call_name: &str,
    ) -> BasicValueEnum<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let task_new_fn = self.ensure_runtime_i64_fn("molt_task_new", 3);
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
        let inc_fn = self.ensure_runtime_i64_fn("molt_inc_ref_obj", 1);
        for (idx, &arg_id) in payload_operands.iter().enumerate() {
            let arg_bits = self.materialize_dynbox_operand(arg_id);
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
            let _ = self
                .backend
                .builder
                .build_call(inc_fn, &[arg_bits.into()], "task_payload_inc_ref")
                .unwrap();
        }
        task_bits
    }
}
