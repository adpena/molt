use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    // ── CheckedAdd: hardware-exact signed-overflow add (overflow_peel) ──
    //
    // A TOTAL function with two lanes:
    //
    // RAW lane (both operands proven overflow-safe raw-i64 carriers):
    // `(sum, flag) = llvm.sadd.with.overflow.i64` — LLVM's canonical
    // checked-arithmetic intrinsic. The sum is the wrapping i64 result,
    // observable ONLY on the flag=0 branch (the peel's CFG enforces this;
    // the slow loop is seeded from the PRE-iteration values). The flag is
    // an i1, consumed directly by CondBranch's `TirType::Bool` path.
    //
    // BOXED lane (any operand unproven — the v1 state on LLVM, whose
    // value-keyed RawI64Safe is a 47-bit-window contract that cannot carry
    // an unbounded accumulator): dispatch through `molt_add` with NaN-boxed
    // operands — BigInt-exact, so the sum can never silently wrap and the
    // flag is CONSTANT FALSE (the peel's slow path is correctly dead; same
    // semantics, no speedup until the RawI64Full lattice extension lands).
    pub(super) fn emit_checked_add(&mut self, op: &crate::tir::ops::TirOp) {
        let sum_id = op.results[0];
        let flag_id = op.results[1];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let raw_lane = matches!(lhs_ty, TirType::I64)
            && matches!(rhs_ty, TirType::I64)
            && self.repr_facts.is_raw_int_carrier(lhs_id)
            && self.repr_facts.is_raw_int_carrier(rhs_id);
        if raw_lane {
            let lhs = self.resolve(lhs_id).into_int_value();
            let rhs = self.resolve(rhs_id).into_int_value();
            let i64_ty = self.backend.context.i64_type();
            let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.sadd.with.overflow")
                .expect("llvm.sadd.with.overflow intrinsic must exist");
            let decl = intrinsic
                .get_declaration(&self.backend.module, &[i64_ty.into()])
                .expect("llvm.sadd.with.overflow.i64 declaration must succeed");
            let pair = self
                .backend
                .builder
                .build_call(decl, &[lhs.into(), rhs.into()], "checked_add")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_struct_value();
            let sum = self
                .backend
                .builder
                .build_extract_value(pair, 0, "ca_sum")
                .unwrap();
            let flag = self
                .backend
                .builder
                .build_extract_value(pair, 1, "ca_of")
                .unwrap();
            self.values.insert(sum_id, sum);
            self.value_types.insert(sum_id, TirType::I64);
            self.values.insert(flag_id, flag);
            // i1 — CondBranch's `TirType::Bool` arm uses it directly as the
            // branch condition (no truthiness call, no NaN-box round-trip).
            self.value_types.insert(flag_id, TirType::Bool);
        } else {
            let lhs = self.resolve(lhs_id);
            let rhs = self.resolve(rhs_id);
            let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
            let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
            let sum = self.call_runtime_2("molt_add", lhs_i64.into(), rhs_i64.into());
            self.values.insert(sum_id, sum);
            self.value_types.insert(sum_id, TirType::DynBox);
            let false_flag = self.backend.context.bool_type().const_zero();
            self.values.insert(flag_id, false_flag.into());
            self.value_types.insert(flag_id, TirType::Bool);
        }
    }

    // ── CheckedMul: hardware-exact signed-overflow multiply (overflow_peel) ──
    //
    // A TOTAL function with two lanes, mirroring `emit_checked_add` exactly.
    //
    // RAW lane (both operands proven overflow-safe raw-i64 carriers):
    // `(prod, flag) = llvm.smul.with.overflow.i64` — LLVM's canonical
    // checked-multiply intrinsic (the multiply analogue of
    // `llvm.sadd.with.overflow`). The product is the wrapping i64 result,
    // observable ONLY on the flag=0 branch (the peel's CFG enforces this;
    // the slow loop is seeded from the PRE-iteration values). The flag is an
    // i1, consumed directly by CondBranch's `TirType::Bool` path.
    //
    // BOXED lane (any operand unproven — the v1 state on LLVM, whose
    // value-keyed RawI64Safe is a 47-bit-window contract that cannot carry an
    // unbounded accumulator): dispatch through `molt_mul` with NaN-boxed
    // operands — BigInt-exact, so the product can never silently wrap and the
    // flag is CONSTANT FALSE (the peel's slow path is correctly dead; same
    // semantics, no speedup until the RawI64Full lattice extension lands).
    pub(super) fn emit_checked_mul(&mut self, op: &crate::tir::ops::TirOp) {
        let prod_id = op.results[0];
        let flag_id = op.results[1];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let raw_lane = matches!(lhs_ty, TirType::I64)
            && matches!(rhs_ty, TirType::I64)
            && self.repr_facts.is_raw_int_carrier(lhs_id)
            && self.repr_facts.is_raw_int_carrier(rhs_id);
        if raw_lane {
            let lhs = self.resolve(lhs_id).into_int_value();
            let rhs = self.resolve(rhs_id).into_int_value();
            let i64_ty = self.backend.context.i64_type();
            let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.smul.with.overflow")
                .expect("llvm.smul.with.overflow intrinsic must exist");
            let decl = intrinsic
                .get_declaration(&self.backend.module, &[i64_ty.into()])
                .expect("llvm.smul.with.overflow.i64 declaration must succeed");
            let pair = self
                .backend
                .builder
                .build_call(decl, &[lhs.into(), rhs.into()], "checked_mul")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_struct_value();
            let prod = self
                .backend
                .builder
                .build_extract_value(pair, 0, "cm_prod")
                .unwrap();
            let flag = self
                .backend
                .builder
                .build_extract_value(pair, 1, "cm_of")
                .unwrap();
            self.values.insert(prod_id, prod);
            self.value_types.insert(prod_id, TirType::I64);
            self.values.insert(flag_id, flag);
            // i1 — CondBranch's `TirType::Bool` arm uses it directly as the
            // branch condition (no truthiness call, no NaN-box round-trip).
            self.value_types.insert(flag_id, TirType::Bool);
        } else {
            let lhs = self.resolve(lhs_id);
            let rhs = self.resolve(rhs_id);
            let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
            let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
            let prod = self.call_runtime_2("molt_mul", lhs_i64.into(), rhs_i64.into());
            self.values.insert(prod_id, prod);
            self.value_types.insert(prod_id, TirType::DynBox);
            let false_flag = self.backend.context.bool_type().const_zero();
            self.values.insert(flag_id, false_flag.into());
            self.value_types.insert(flag_id, TirType::Bool);
        }
    }

    // ── Type-specialized binary arithmetic ──

    /// Emit a divisor-zero-guarded I64 division-family op (`/`, `//`, `%`).
    ///
    /// A raw machine `sdiv`/`srem` (or float divide) by zero is a SILENT
    /// miscompile: LLVM `sdiv x, 0` is poison (observed: a garbage NaN-box bit
    /// pattern instead of CPython's `ZeroDivisionError`). The native backend
    /// already guards this with an inline runtime zero-check; this mirrors that
    /// pattern for LLVM so all backends raise byte-identically.
    ///
    /// Shape (cold slow path so the non-zero hot path stays a straight-line raw
    /// divide — no perf regression vs the unguarded code):
    /// ```text
    ///   if rhs != 0 { fast: <raw divide>        }  ──┐
    ///   else        { slow: molt_<op>(box,box)  }  ──┤→ merge: phi
    /// ```
    /// `molt_floordiv`/`molt_mod`/`molt_div` set `ZeroDivisionError` for the
    /// zero divisor, so the slow path never returns normally; its (dead) result
    /// is still unboxed to the fast lane's carrier type to keep the phi
    /// well-typed.
    pub(super) fn emit_i64_divrem_zero_guarded(
        &mut self,
        op: &crate::tir::ops::TirOp,
        name: &str,
        lhs_i: inkwell::values::IntValue<'ctx>,
        rhs_i: inkwell::values::IntValue<'ctx>,
    ) -> (BasicValueEnum<'ctx>, TirType) {
        let i64_ty = self.backend.context.i64_type();
        let zero = i64_ty.const_zero();
        let rhs_nonzero = self
            .backend
            .builder
            .build_int_compare(inkwell::IntPredicate::NE, rhs_i, zero, "rhs_nonzero")
            .unwrap();
        let current_fn = self.llvm_fn;
        let fast_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "divrem_fast");
        let slow_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "divrem_zero");
        let merge_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "divrem_merge");
        self.all_llvm_blocks.push(fast_bb);
        self.all_llvm_blocks.push(slow_bb);
        self.all_llvm_blocks.push(merge_bb);
        self.backend
            .builder
            .build_conditional_branch(rhs_nonzero, fast_bb, slow_bb)
            .unwrap();

        // ── Fast path: divisor proven non-zero here, raw machine divide. ──
        self.backend.builder.position_at_end(fast_bb);
        let (fast_val, out_ty): (BasicValueEnum<'ctx>, TirType) = match name {
            "div" => {
                // Python `/` on ints returns float (7 / 2 == 3.5).
                let f64_ty = self.backend.context.f64_type();
                let lhs_f = self
                    .backend
                    .builder
                    .build_signed_int_to_float(lhs_i, f64_ty, "div_lhs_f")
                    .unwrap();
                let rhs_f = self
                    .backend
                    .builder
                    .build_signed_int_to_float(rhs_i, f64_ty, "div_rhs_f")
                    .unwrap();
                let v = self
                    .backend
                    .builder
                    .build_float_div(lhs_f, rhs_f, "div_f")
                    .unwrap();
                (v.into(), TirType::F64)
            }
            "floordiv" => {
                // Python `//`: floor toward -inf. q = sdiv; r = srem;
                // if (r != 0 && (lhs ^ rhs) < 0) q -= 1.
                let one = i64_ty.const_int(1, false);
                let q = self
                    .backend
                    .builder
                    .build_int_signed_div(lhs_i, rhs_i, "fdiv_q")
                    .unwrap();
                let r = self
                    .backend
                    .builder
                    .build_int_signed_rem(lhs_i, rhs_i, "fdiv_r")
                    .unwrap();
                let r_ne_0 = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, r, zero, "r_ne_0")
                    .unwrap();
                let xor = self
                    .backend
                    .builder
                    .build_xor(lhs_i, rhs_i, "signs_xor")
                    .unwrap();
                let signs_differ = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, xor, zero, "signs_differ")
                    .unwrap();
                let needs_adjust = self
                    .backend
                    .builder
                    .build_and(r_ne_0, signs_differ, "needs_adj")
                    .unwrap();
                let q_minus_1 = self.backend.builder.build_int_sub(q, one, "q_m1").unwrap();
                let q_m1_basic: BasicValueEnum<'ctx> = q_minus_1.into();
                let q_basic: BasicValueEnum<'ctx> = q.into();
                let adj = self
                    .backend
                    .builder
                    .build_select(needs_adjust, q_m1_basic, q_basic, "floordiv")
                    .unwrap();
                (adj, TirType::I64)
            }
            "mod" => {
                // Python `%`: result has the sign of the divisor.
                // r = srem; if (r != 0 && (r ^ rhs) < 0) r += rhs.
                let r = self
                    .backend
                    .builder
                    .build_int_signed_rem(lhs_i, rhs_i, "mod_r")
                    .unwrap();
                let r_ne_0 = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, r, zero, "mod_r_ne_0")
                    .unwrap();
                let xor = self
                    .backend
                    .builder
                    .build_xor(r, rhs_i, "mod_signs_xor")
                    .unwrap();
                let signs_differ = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, xor, zero, "mod_signs_differ")
                    .unwrap();
                let needs_adjust = self
                    .backend
                    .builder
                    .build_and(r_ne_0, signs_differ, "mod_adj")
                    .unwrap();
                let r_plus_rhs = self
                    .backend
                    .builder
                    .build_int_add(r, rhs_i, "mod_adjusted")
                    .unwrap();
                let r_adj_basic: BasicValueEnum<'ctx> = r_plus_rhs.into();
                let r_basic: BasicValueEnum<'ctx> = r.into();
                let result = self
                    .backend
                    .builder
                    .build_select(needs_adjust, r_adj_basic, r_basic, "pymod")
                    .unwrap();
                (result, TirType::I64)
            }
            other => unreachable!("emit_i64_divrem_zero_guarded called with {other:?}"),
        };
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let fast_pred = self.backend.builder.get_insert_block().unwrap();

        // ── Slow path: divisor == 0 ⇒ boxed runtime raises ZeroDivisionError. ──
        self.backend.builder.position_at_end(slow_bb);
        let rt_name = match name {
            "div" => "molt_div",
            "floordiv" => "molt_floordiv",
            "mod" => "molt_mod",
            other => unreachable!("emit_i64_divrem_zero_guarded called with {other:?}"),
        };
        let boxed = self
            .call_runtime_2_boxed(rt_name, op.operands[0], op.operands[1])
            .into_int_value();
        // The runtime raised; this value is unreachable-but-typed. Convert the
        // DynBox bits to the fast lane's carrier so the phi types line up.
        let slow_val: BasicValueEnum<'ctx> = match out_ty {
            TirType::F64 => self
                .backend
                .builder
                .build_bit_cast(boxed, self.backend.context.f64_type(), "div_zero_f64")
                .unwrap(),
            _ => unbox_dynbox_to_param_ty_with_builder(
                &self.backend.builder,
                self.backend.context,
                boxed,
                &out_ty,
            )
            .into(),
        };
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let slow_pred = self.backend.builder.get_insert_block().unwrap();

        // ── Merge. ──
        self.backend.builder.position_at_end(merge_bb);
        let phi_ty: inkwell::types::BasicTypeEnum<'ctx> = match out_ty {
            TirType::F64 => self.backend.context.f64_type().into(),
            _ => i64_ty.into(),
        };
        let phi = self.backend.builder.build_phi(phi_ty, "divrem").unwrap();
        phi.add_incoming(&[(&fast_val, fast_pred), (&slow_val, slow_pred)]);
        (phi.as_basic_value(), out_ty)
    }

    pub(super) fn emit_binary_arith(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let fast_math = has_attr(op, "fast_math");
        // When the `no_signed_wrap` attribute is set by a TIR analysis pass
        // (for example range_devirt for bounded induction increments), we
        // emit nsw-flagged integer instructions.  This enables LLVM's:
        //  - Strength reduction (e.g. `i * 4` → `i << 2` with guaranteed no wrap)
        //  - SCEV (Scalar Evolution) for loop trip count analysis
        //  - Loop vectorization with known induction variable ranges
        let nsw = has_attr(op, "no_signed_wrap");

        // Inline-safety gate (the structural fix for the LLVM int-overflow
        // miscompile): a raw machine `add`/`sub`/`mul` may only be emitted when
        // the plan proves the *result* fits the inline-int47 payload window.
        // `TirType::I64` alone is a *semantic* int — `type_refine` assigns
        // `add(I64, I64) -> I64` with no range proof — so gating on the type
        // would silently wrap and then truncate to 47 bits at box time. Names
        // outside the inline-safe set
        // fall through to the boxed runtime path (`molt_add`/`molt_sub`/
        // `molt_mul`), which is BigInt-correct, mirroring the native and WASM
        // backends.
        let int_inline_safe = self.repr_facts.is_inline_safe_int(result_id);

        let (val, out_ty) = match (&lhs_ty, &rhs_ty, name) {
            // I64 + I64 -> I64 (direct machine instruction).
            // When `nsw` is set, use build_int_nsw_add to tell LLVM the
            // result is guaranteed not to overflow as a signed i64.
            (TirType::I64, TirType::I64, "add") if int_inline_safe => {
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let v = if nsw {
                    self.backend.builder.build_int_nsw_add(lhs_i, rhs_i, "add")
                } else {
                    self.backend.builder.build_int_add(lhs_i, rhs_i, "add")
                }
                .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "sub") if int_inline_safe => {
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let v = if nsw {
                    self.backend.builder.build_int_nsw_sub(lhs_i, rhs_i, "sub")
                } else {
                    self.backend.builder.build_int_sub(lhs_i, rhs_i, "sub")
                }
                .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "mul") if int_inline_safe => {
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let v = if nsw {
                    self.backend.builder.build_int_nsw_mul(lhs_i, rhs_i, "mul")
                } else {
                    self.backend.builder.build_int_mul(lhs_i, rhs_i, "mul")
                }
                .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "div" | "floordiv" | "mod") => {
                // A raw machine divide by zero is poison (LLVM) — route through a
                // divisor-zero guard so a zero divisor raises ZeroDivisionError
                // via the boxed runtime instead of silently yielding garbage.
                self.emit_i64_divrem_zero_guarded(
                    op,
                    name,
                    lhs.into_int_value(),
                    rhs.into_int_value(),
                )
            }

            // F64 + F64 -> F64 (direct machine instruction).
            // When `fast_math = true` is set on the TIR op (injected by the
            // fast_math annotation pass), we apply LLVM's full fast-math flag
            // set to the emitted instruction via `InstructionValue::set_fast_math_flags`.
            (TirType::F64, TirType::F64, "add") => {
                let v = self
                    .backend
                    .builder
                    .build_float_add(lhs.into_float_value(), rhs.into_float_value(), "fadd")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "sub") => {
                let v = self
                    .backend
                    .builder
                    .build_float_sub(lhs.into_float_value(), rhs.into_float_value(), "fsub")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "mul") => {
                let v = self
                    .backend
                    .builder
                    .build_float_mul(lhs.into_float_value(), rhs.into_float_value(), "fmul")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "div") => {
                let v = self
                    .backend
                    .builder
                    .build_float_div(lhs.into_float_value(), rhs.into_float_value(), "fdiv")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "mod") => {
                let v = self
                    .backend
                    .builder
                    .build_float_rem(lhs.into_float_value(), rhs.into_float_value(), "fmod")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }

            // Everything else: call runtime (DynBox dispatch).
            //
            // The boxed slow path must honour the in-place dunder protocol for
            // augmented assignment. An augassign op reaches here either as a
            // first-class InplaceAdd/InplaceSub/InplaceMul opcode OR as a
            // Copy-carried `inplace_floordiv`/`inplace_mod`/`inplace_pow`/... with
            // its `_original_kind` preserved (the lower_preserved arms route those
            // here via emit_binary_arith with the binary `name`). In both cases
            // CPython requires `molt_inplace_<op>` — which tries `__i<op>__`
            // BEFORE the binary `__op__`/`__rop__` chain — not the binary
            // `molt_<op>`. Selecting `molt_<op>` here was a silent miscompile:
            // a class defining only `__iadd__`/`__ifloordiv__`/… had its `+=`/
            // `//=` routed to the binary fallback dunder. The fast int/float lanes
            // above stay on the binary instruction because builtin numerics have
            // no in-place dunder (so the result is byte-identical there).
            _ => {
                let is_inplace = opcode_uses_boxed_runtime_inplace_dispatch_table(op.opcode)
                    || op
                        .attrs
                        .get("_original_kind")
                        .and_then(|v| match v {
                            AttrValue::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .is_some_and(|k| k.starts_with("inplace_"));
                let rt_name = match (name, is_inplace) {
                    ("add", false) => "molt_add",
                    ("add", true) => "molt_inplace_add",
                    ("sub", false) => "molt_sub",
                    ("sub", true) => "molt_inplace_sub",
                    ("mul", false) => "molt_mul",
                    ("mul", true) => "molt_inplace_mul",
                    ("div", false) => "molt_div",
                    ("div", true) => "molt_inplace_div",
                    ("floordiv", false) => "molt_floordiv",
                    ("floordiv", true) => "molt_inplace_floordiv",
                    ("mod", false) => "molt_mod",
                    ("mod", true) => "molt_inplace_mod",
                    ("pow", false) => "molt_pow",
                    ("pow", true) => "molt_inplace_pow",
                    _ => unreachable!("unknown arith op: {}", name),
                };
                let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let v = self.call_runtime_2(rt_name, lhs_i64.into(), rhs_i64.into());
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Type-specialized comparison ──

    pub(super) fn emit_comparison(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let (val, out_ty) = match (&lhs_ty, &rhs_ty) {
            (TirType::I64, TirType::I64) => {
                use inkwell::IntPredicate;
                let pred = match name {
                    "eq" => IntPredicate::EQ,
                    "ne" => IntPredicate::NE,
                    "lt" => IntPredicate::SLT,
                    "le" => IntPredicate::SLE,
                    "gt" => IntPredicate::SGT,
                    "ge" => IntPredicate::SGE,
                    _ => unreachable!(),
                };
                let v = self
                    .backend
                    .builder
                    .build_int_compare(pred, lhs.into_int_value(), rhs.into_int_value(), name)
                    .unwrap();
                (v.into(), TirType::Bool)
            }
            (TirType::F64, TirType::F64) => {
                use inkwell::FloatPredicate;
                let pred = match name {
                    "eq" => FloatPredicate::OEQ,
                    "ne" => FloatPredicate::ONE,
                    "lt" => FloatPredicate::OLT,
                    "le" => FloatPredicate::OLE,
                    "gt" => FloatPredicate::OGT,
                    "ge" => FloatPredicate::OGE,
                    _ => unreachable!(),
                };
                let v = self
                    .backend
                    .builder
                    .build_float_compare(pred, lhs.into_float_value(), rhs.into_float_value(), name)
                    .unwrap();
                (v.into(), TirType::Bool)
            }
            _ => {
                let rt_name = match name {
                    "eq" => "molt_eq",
                    "ne" => "molt_ne",
                    "lt" => "molt_lt",
                    "le" => "molt_le",
                    "gt" => "molt_gt",
                    "ge" => "molt_ge",
                    _ => unreachable!(),
                };
                let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let v = self.call_runtime_2(rt_name, lhs_i64.into(), rhs_i64.into());
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Identity (is / is not) ──

    pub(super) fn emit_identity(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let lhs = self.resolve(op.operands[0]);
        let rhs = self.resolve(op.operands[1]);
        let lhs_i64 = self.ensure_i64(lhs);
        let rhs_i64 = self.ensure_i64(rhs);
        let cmp = self
            .backend
            .builder
            .build_int_compare(inkwell::IntPredicate::EQ, lhs_i64, rhs_i64, "is")
            .unwrap();
        let val: BasicValueEnum<'ctx> = if op.opcode == OpCode::IsNot {
            self.backend
                .builder
                .build_not(cmp, "is_not")
                .unwrap()
                .into()
        } else {
            cmp.into()
        };
        self.values.insert(result_id, val);
        self.value_types.insert(result_id, TirType::Bool);
    }

    // ── Containment (in / not in) ──

    pub(super) fn emit_containment(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        // `molt_contains(container, item)`. The membership op's operands are
        // [container, item] (matching the native `contains` arm and the SimpleIR
        // `contains`/`in`/`not_in` convention), so they must be passed in that
        // order — swapping them makes `3 in [1, 2, 3]` call `molt_contains(3,
        // [1, 2, 3])`, reporting `argument of type 'int' is not iterable`.
        let val = self.call_runtime_2_boxed("molt_contains", op.operands[0], op.operands[1]);
        let final_val = if op.opcode == OpCode::NotIn {
            // Invert the boolean result from molt_contains
            let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
            let item_i64 = self.ensure_i64(val);
            let truthy = self
                .backend
                .builder
                .build_call(truthy_fn, &[item_i64.into()], "truthy")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            let as_bool = self
                .backend
                .builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    truthy.into_int_value(),
                    self.backend.context.i64_type().const_int(0, false),
                    "not_in",
                )
                .unwrap();
            as_bool.into()
        } else {
            val
        };
        self.values.insert(result_id, final_val);
        let out_ty = if op.opcode == OpCode::NotIn {
            TirType::Bool
        } else {
            TirType::DynBox
        };
        self.value_types.insert(result_id, out_ty);
    }

    // ── Bitwise ops ──

    pub(super) fn emit_bitwise(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        // Overflow / shift-validity gate for the raw I64 lane. `bit_and`/
        // `bit_or`/`bit_xor` are unconditionally sound on raw i64 (the result
        // bits are a subset of the operand bits — no overflow, no UB), so they
        // always take the machine lane. `lshift`/`rshift` are NOT: a raw
        // `shl`/`ashr` whose count is `>= 64` is LLVM poison, and a `<<` result
        // that exceeds i64 wraps then truncates at box time — the silent
        // integer miscompile. The shift raw lane is therefore admitted only when
        // the plan proves the *result* is an inline-int47-safe carrier
        // (`RawI64Safe`), which the value-range seed grants for a shift ONLY when
        // its count is range-proven in `[0, 63]` AND the result fits the inline
        // window (single source of truth, shared with native/WASM). An unproven
        // shift falls through to the boxed `molt_lshift`/`molt_rshift` runtime —
        // BigInt-correct, negative-count `ValueError`-correct, huge-count
        // `OverflowError`-correct — exactly mirroring `emit_binary_arith`'s
        // `int_inline_safe` gate and the native backend (which boxes every
        // shift).
        let shift_inline_safe = self.repr_facts.is_inline_safe_int(result_id);
        let raw_i64_lane_ok = match name {
            "bit_and" | "bit_or" | "bit_xor" => true,
            "lshift" | "rshift" => shift_inline_safe,
            _ => unreachable!("emit_bitwise got non-bitwise name: {name}"),
        };
        let (val, out_ty) = match (&lhs_ty, &rhs_ty) {
            (TirType::I64, TirType::I64) if raw_i64_lane_ok => {
                let v = match name {
                    "bit_and" => self
                        .backend
                        .builder
                        .build_and(lhs.into_int_value(), rhs.into_int_value(), "band")
                        .unwrap(),
                    "bit_or" => self
                        .backend
                        .builder
                        .build_or(lhs.into_int_value(), rhs.into_int_value(), "bor")
                        .unwrap(),
                    "bit_xor" => self
                        .backend
                        .builder
                        .build_xor(lhs.into_int_value(), rhs.into_int_value(), "bxor")
                        .unwrap(),
                    "lshift" => self
                        .backend
                        .builder
                        .build_left_shift(lhs.into_int_value(), rhs.into_int_value(), "shl")
                        .unwrap(),
                    "rshift" => self
                        .backend
                        .builder
                        .build_right_shift(lhs.into_int_value(), rhs.into_int_value(), true, "shr")
                        .unwrap(),
                    _ => unreachable!(),
                };
                (v.into(), TirType::I64)
            }
            _ => {
                // Honour the in-place dunder for `<<=`/`>>=` (and the inplace
                // bitwise family). A Copy-carried `inplace_lshift`/`inplace_rshift`
                // /`inplace_bit_*` reaches the bitwise emitter with the BINARY
                // `name` ("lshift"/"bit_or"/…) but must dispatch the boxed slow
                // path to `molt_inplace_<op>` so `__ilshift__`/`__ior__`/… is
                // tried before the binary `__op__`/`__rop__` chain. The fast int
                // lane above is unchanged (builtin int has no in-place dunder).
                let is_inplace = op
                    .attrs
                    .get("_original_kind")
                    .and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .is_some_and(|k| k.starts_with("inplace_"));
                let rt_name = if is_inplace {
                    format!("molt_inplace_{}", name)
                } else {
                    format!("molt_{}", name)
                };
                // The runtime bitwise entries take NaN-BOXED operands. A raw
                // `TirType::I64` operand (e.g. the `4` in `x <<= 4`) must be boxed
                // via `materialize_dynbox_bits`, NOT passed through `ensure_i64`
                // (which forwards the raw i64 bit pattern — the runtime then
                // mis-reads `4` as the subnormal float 2e-323). This mirrors
                // `emit_binary_arith`'s boxed fallback; using `ensure_i64` here
                // was a latent miscompile of `<<`/`>>`/bitwise on a raw-int
                // operand, now exposed by the `<<=`/`>>=` in-place dunder path.
                let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let v = self.call_runtime_2(&rt_name, lhs_i64.into(), rhs_i64.into());
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Unary ops ──

    pub(super) fn emit_unary(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let (val, out_ty) = match (&operand_ty, name) {
            (TirType::I64, "neg") => {
                let v = self
                    .backend
                    .builder
                    .build_int_neg(operand.into_int_value(), "neg")
                    .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::F64, "neg") => {
                let v = self
                    .backend
                    .builder
                    .build_float_neg(operand.into_float_value(), "fneg")
                    .unwrap();
                (v.into(), TirType::F64)
            }
            (TirType::Bool, "not") => {
                let v = self
                    .backend
                    .builder
                    .build_not(operand.into_int_value(), "not")
                    .unwrap();
                (v.into(), TirType::Bool)
            }
            (TirType::I64, "invert") => {
                let v = self
                    .backend
                    .builder
                    .build_not(operand.into_int_value(), "invert")
                    .unwrap();
                (v.into(), TirType::I64)
            }
            _ => {
                let rt_name = match name {
                    "neg" => "molt_neg",
                    "not" => "molt_not",
                    "invert" => "molt_invert",
                    _ => unreachable!(),
                };
                let op_i64 = self.ensure_i64(operand);
                let func = self.backend.module.get_function(rt_name).unwrap();
                let v = self
                    .backend
                    .builder
                    .build_call(func, &[op_i64.into()], name)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }
}
