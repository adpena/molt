use super::*;
use crate::codegen_ssa::HeldCarrier;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// `CheckedAdd` and `CheckedMul`, the overflow peel's ops: the walker's
    /// raw lane, else the boxed runtime entry, which is exact (BigInt), so
    /// its overflow bit is constant false and the peel's slow loop is dead.
    pub(super) fn emit_checked_arith(&mut self, op: &crate::tir::ops::TirOp) {
        let [value_id, overflow_id] = [op.results[0], op.results[1]];
        let [lhs_id, rhs_id] = [op.operands[0], op.operands[1]];
        let raw = crate::codegen_ssa::numeric::checked_arith(
            &mut self.codegen(),
            &self.repr_facts,
            op.opcode,
            self.numeric_operand(lhs_id),
            self.numeric_operand(rhs_id),
        );
        let ((value, carrier), overflow) = match raw {
            Some((value, overflow)) => ((value, HeldCarrier::RawI64), overflow),
            None => {
                let runtime = match op.opcode {
                    OpCode::CheckedAdd => RuntimeFn::Add,
                    _ => RuntimeFn::Mul,
                };
                let boxed = self.emit_boxed_binary_fallback(runtime, lhs_id, rhs_id);
                let never = self.backend.context.bool_type().const_zero();
                ((boxed, HeldCarrier::Boxed), never.into())
            }
        };
        self.set_lowered(value_id, value, carrier);
        // The bit is an i1 a conditional branch takes as its condition.
        self.set_lowered(overflow_id, overflow, HeldCarrier::RawBool);
    }

    /// `id` as the walker's numeric families take an operand.
    pub(super) fn numeric_operand(
        &self,
        id: ValueId,
    ) -> crate::codegen_ssa::numeric::Operand<BasicValueEnum<'ctx>> {
        crate::codegen_ssa::numeric::Operand {
            id,
            value: self.resolve(id),
            carrier: self
                .values
                .repr(id)
                .unwrap_or(crate::codegen_ssa::HeldCarrier::Boxed),
        }
    }

    /// Record `result`'s lowered value and the carrier it is held in.
    pub(super) fn set_lowered(
        &mut self,
        result: ValueId,
        value: BasicValueEnum<'ctx>,
        carrier: crate::codegen_ssa::HeldCarrier,
    ) {
        self.values.set_value(result, value);
        self.values.set_repr(result, carrier);
    }

    pub(super) fn emit_binary_arith(&mut self, op: &crate::tir::ops::TirOp, name: OpKind) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let (lhs, rhs) = (self.numeric_operand(lhs_id), self.numeric_operand(rhs_id));
        let raw = match name {
            OpKind::Div | OpKind::Floordiv | OpKind::Mod => crate::codegen_ssa::numeric::divide(
                &mut self.codegen(),
                &self.repr_facts,
                name,
                result_id,
                lhs,
                rhs,
            ),
            _ => crate::codegen_ssa::numeric::binary_arith(
                &mut self.codegen(),
                &self.repr_facts,
                op,
                name,
                lhs,
                rhs,
            ),
        };
        if let Some((value, carrier)) = raw {
            self.set_lowered(result_id, value, carrier);
            return;
        }
        // The boxed runtime entry. It must honour the in-place dunder
        // protocol for augmented assignment: an augassign op reaches here as
        // a first-class InplaceAdd/InplaceSub/InplaceMul opcode or as a
        // Copy-carried `inplace_floordiv`/`inplace_mod`/`inplace_pow`/... with
        // its `origin` preserved, and CPython requires `molt_inplace_<op>`
        // (which tries `__i<op>__` before the binary `__op__`/`__rop__`
        // chain), not the binary `molt_<op>`. The raw lanes above stay on the
        // binary instruction because builtin numerics have no in-place dunder.
        let boxed =
            self.emit_boxed_binary_fallback(Self::boxed_numeric_runtime(op, name), lhs_id, rhs_id);
        self.set_lowered(result_id, boxed, HeldCarrier::Boxed);
    }

    // ── Type-specialized comparison ──

    pub(super) fn emit_comparison(&mut self, op: &crate::tir::ops::TirOp, name: OpKind) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];

        let raw = crate::codegen_ssa::numeric::comparison(
            &mut self.codegen(),
            name,
            self.numeric_operand(lhs_id),
            self.numeric_operand(rhs_id),
        );
        let (value, carrier) = raw.unwrap_or_else(|| {
            let boxed =
                self.emit_boxed_binary_fallback(Self::numeric_runtime(name), lhs_id, rhs_id);
            (boxed, crate::codegen_ssa::HeldCarrier::Boxed)
        });
        self.set_lowered(result_id, value, carrier);
    }

    // ── Identity (is / is not) ──

    pub(super) fn emit_identity(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let invert = op.opcode == OpCode::IsNot;
        let i64_ty = self.backend.context.i64_type();
        // Identity consumes object words, not raw register payloads: a boxed
        // False and a raw i1 zero denote the same singleton. The shared operand
        // custody also boxes each full-width integer once and retires its owner.
        let word = self
            .with_borrowed_boxed_operands(&op.operands, "identity", |this, words, name| {
                let cmp = this
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::EQ, words[0], words[1], "is")
                    .unwrap();
                let flag = if invert {
                    this.backend.builder.build_not(cmp, "is_not").unwrap()
                } else {
                    cmp
                };
                this.backend
                    .builder
                    .build_int_z_extend(flag, i64_ty, name)
                    .unwrap()
            })
            .into_int_value();
        // Failed materialization returns None with the first error pending.
        // Only a committed machine 1 is true, including for `is not`.
        let val = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                word,
                i64_ty.const_int(1, false),
                "identity_bool",
            )
            .unwrap();
        self.values.set_value(result_id, val.into());
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::Bool),
        );
    }

    // ── Containment (in / not in) ──

    pub(super) fn emit_containment(&mut self, op: &crate::tir::ops::TirOp) {
        // `molt_contains(container, item)`. The membership op's operands are
        // [container, item] (matching the native `contains` arm and the SimpleIR
        // `contains`/`in`/`not_in` convention), so they must be passed in that
        // order — swapping them makes `3 in [1, 2, 3]` call `molt_contains(3,
        // [1, 2, 3])`, reporting `argument of type 'int' is not iterable`.
        let contains_fn = self.ensure_runtime_import(RuntimeFn::Contains);
        let args = [
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
        ];
        let custody = ResultCustody::canonical(RuntimeFn::Contains, 2);
        if op.opcode != OpCode::NotIn {
            self.emit_borrowed_runtime_call(
                op,
                contains_fn,
                &args,
                custody,
                "contains",
                RuntimeFn::Contains.symbol(),
            );
            return;
        }
        // `not in` keeps only the inverted truthiness of the membership result.
        let result_id = op.results[0];
        let val = self.borrowed_runtime_call_value(
            contains_fn,
            &args,
            false,
            "contains",
            RuntimeFn::Contains.symbol(),
        );
        let truthy_fn = self.ensure_runtime_import(RuntimeFn::IsTruthy);
        let truthy = self
            .backend
            .builder
            .build_execution_call(truthy_fn, &[val.into()], "truthy")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let not_in = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                truthy.into_int_value(),
                self.backend.context.i64_type().const_int(0, false),
                "not_in",
            )
            .unwrap();
        if custody == ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue) {
            let release = self.ensure_refcount_site(
                molt_codegen_abi::inline_refcount::RefcountOperation::Release,
            );
            release.emit(&self.backend.builder, val.into());
        }
        self.values.set_value(result_id, not_in.into());
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::Bool),
        );
    }

    /// The boxed runtime lane of a binary operator: both operands are borrowed
    /// through one custody, and the owned result is the operator's value.
    fn emit_boxed_binary_fallback(
        &mut self,
        function: RuntimeFn,
        lhs: ValueId,
        rhs: ValueId,
    ) -> BasicValueEnum<'ctx> {
        let callee = self.ensure_runtime_import(function);
        self.borrowed_runtime_call_value(
            callee,
            &[RuntimeArg::Operand(lhs), RuntimeArg::Operand(rhs)],
            false,
            "binary",
            function.symbol(),
        )
        .into()
    }

    /// The runtime entry of a numeric op `name`, from the one generated table.
    fn numeric_runtime(name: OpKind) -> RuntimeFn {
        molt_ir::runtime_fn_generated::numeric_runtime_fn(name)
            .unwrap_or_else(|| unreachable!("{name} has no numeric runtime entry"))
    }

    /// The runtime entry that implements `op` on boxed operands
    /// (`TirOp::numeric_runtime`), else the entry of the binary `name` a
    /// preserved carrier was routed by.
    fn boxed_numeric_runtime(op: &TirOp, name: OpKind) -> RuntimeFn {
        op.numeric_runtime()
            .unwrap_or_else(|| Self::numeric_runtime(name))
    }

    // ── Boolean selection (and / or) ──

    /// Frontend BoolOp lowering uses And/Or ops to produce the selected operand
    /// value inside already-structured control flow, so this preserves Python
    /// operand selection, not bitwise semantics. The result owns one reference
    /// to the selected operand (`result_mints_owned_selected_operand`). Two
    /// object carriers select a word and retain it. Otherwise each edge
    /// materializes only its own operand, so a scalar is boxed once, only when
    /// selected, and a minted heap integer is itself the result's owner.
    pub(super) fn emit_boolean_selection(&mut self, op: &crate::tir::ops::TirOp) {
        assert!(
            crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
                op.opcode
            ),
            "{:?} must mint an owned selected operand",
            op.opcode
        );
        let result_id = op.results[0];
        let cond = self.operand_truthiness(op.operands[0]);
        // `a and b` is `b` when `a` is truthy; `a or b` is `a`.
        let (name, when_true, when_false) = if op.opcode == OpCode::And {
            ("bool_and", op.operands[1], op.operands[0])
        } else {
            ("bool_or", op.operands[0], op.operands[1])
        };
        let object_carrier =
            |this: &Self, id: ValueId| Self::tir_type_is_dynbox_like(&this.repr_ty(id));
        let selected = if object_carrier(self, when_true) && object_carrier(self, when_false) {
            let true_bits = self.ensure_i64(self.resolve(when_true));
            let false_bits = self.ensure_i64(self.resolve(when_false));
            let selected = self
                .backend
                .builder
                .build_select(cond, true_bits, false_bits, name)
                .unwrap()
                .into_int_value();
            if !op
                .results
                .first()
                .is_some_and(|&result| self.repr_facts.boxed_refcount_is_noop(result))
            {
                let retain = self.ensure_refcount_site(
                    molt_codegen_abi::inline_refcount::RefcountOperation::Retain,
                );
                retain.emit(&self.backend.builder, selected.into());
            }
            selected
        } else {
            let suffix = self.synthetic_block_counter;
            self.synthetic_block_counter += 1;
            let true_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, &format!("{name}_true{suffix}"));
            let false_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, &format!("{name}_false{suffix}"));
            let merge_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, &format!("{name}_merge{suffix}"));
            self.backend
                .builder
                .build_conditional_branch(cond, true_bb, false_bb)
                .unwrap();
            let mut incoming = Vec::with_capacity(2);
            for (block, operand) in [(true_bb, when_true), (false_bb, when_false)] {
                self.backend.builder.position_at_end(block);
                let word = self.owned_operand_word(operand, "boolop_selected_inc_ref");
                let end = self.backend.builder.get_insert_block().unwrap();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                incoming.push((word, end));
            }
            self.backend.builder.position_at_end(merge_bb);
            let phi = self
                .backend
                .builder
                .build_phi(self.backend.context.i64_type(), name)
                .unwrap();
            for (word, end) in &incoming {
                phi.add_incoming(&[(word, *end)]);
            }
            phi.as_basic_value().into_int_value()
        };
        self.values.set_value(result_id, selected.into());
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    /// Python truthiness of an operand, read from its physical carrier.
    fn operand_truthiness(&self, operand: ValueId) -> inkwell::values::IntValue<'ctx> {
        let value = self.resolve(operand);
        let ty = self.repr_ty(operand);
        match ty {
            TirType::Bool => value.into_int_value(),
            TirType::I64 => self
                .backend
                .builder
                .build_int_compare(
                    inkwell::IntPredicate::NE,
                    value.into_int_value(),
                    self.backend.context.i64_type().const_zero(),
                    "boolop_cond",
                )
                .unwrap(),
            // NaN is truthy and -0.0 is falsy: unordered-or-unequal to zero.
            TirType::F64 => self
                .backend
                .builder
                .build_float_compare(
                    inkwell::FloatPredicate::UNE,
                    value.into_float_value(),
                    self.backend.context.f64_type().const_zero(),
                    "boolop_cond",
                )
                .unwrap(),
            _ => {
                let bits = self.ensure_i64(value);
                let truthy_fn = self.ensure_runtime_import(RuntimeFn::IsTruthy);
                let truthy = self
                    .backend
                    .builder
                    .build_execution_call(truthy_fn, &[bits.into()], "truthy")
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
                        "boolop_cond",
                    )
                    .unwrap()
            }
        }
    }

    // ── Bitwise ops ──

    pub(super) fn emit_bitwise(&mut self, op: &crate::tir::ops::TirOp, name: OpKind) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];

        let raw = crate::codegen_ssa::numeric::bitwise(
            &mut self.codegen(),
            &self.repr_facts,
            op,
            name,
            self.numeric_operand(lhs_id),
            self.numeric_operand(rhs_id),
        );
        let (value, carrier) = match raw {
            Some(raw) => raw,
            None => {
                // Honour the in-place dunder for `<<=`/`>>=` (and the inplace
                // bitwise family). A Copy-carried `inplace_lshift`/`inplace_rshift`
                // /`inplace_bit_*` reaches the bitwise emitter with the BINARY
                // `name` ("lshift"/"bit_or"/…) but must dispatch the boxed slow
                // path to `molt_inplace_<op>` so `__ilshift__`/`__ior__`/… is
                // tried before the binary `__op__`/`__rop__` chain. The fast int
                // lane above is unchanged (builtin int has no in-place dunder).
                // The runtime bitwise entries take NaN-BOXED operands. A raw
                // `TirType::I64` operand (e.g. the `4` in `x <<= 4`) must be boxed
                // as a borrowed operand, NOT passed through `ensure_i64`
                // (which forwards the raw i64 bit pattern — the runtime then
                // mis-reads `4` as the subnormal float 2e-323). This mirrors
                // `emit_binary_arith`'s boxed fallback; using `ensure_i64` here
                // was a latent miscompile of `<<`/`>>`/bitwise on a raw-int
                // operand, now exposed by the `<<=`/`>>=` in-place dunder path.
                let v = self.emit_boxed_binary_fallback(
                    Self::boxed_numeric_runtime(op, name),
                    lhs_id,
                    rhs_id,
                );
                (v, crate::codegen_ssa::HeldCarrier::Boxed)
            }
        };
        self.set_lowered(result_id, value, carrier);
    }

    // ── Unary ops ──

    pub(super) fn emit_unary(&mut self, op: &crate::tir::ops::TirOp, name: OpKind) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let raw = crate::codegen_ssa::numeric::unary(
            &mut self.codegen(),
            &self.repr_facts,
            op,
            name,
            self.numeric_operand(operand_id),
        );
        let (value, carrier) = match raw {
            Some(raw) => raw,
            None => {
                let function = match name {
                    OpKind::Not => RuntimeFn::Not,
                    numeric => Self::numeric_runtime(numeric),
                };
                // Runtime unary entries borrow boxed objects, not widened
                // machine bits. In particular, ~bool needs its Bool tag for
                // both integer semantics and the runtime deprecation warning.
                let func = self.ensure_runtime_import(function);
                let v = self.borrowed_runtime_call_value(
                    func,
                    &[RuntimeArg::Operand(operand_id)],
                    false,
                    "unary",
                    name.as_str(),
                );
                (v.into(), crate::codegen_ssa::HeldCarrier::Boxed)
            }
        };
        self.set_lowered(result_id, value, carrier);
    }
}

#[cfg(test)]
mod numeric_family_tests {
    use super::super::*;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::values::ValueId;

    #[test]
    fn float_divrem_preserves_zero_dispatch_and_python_remainder_sign() {
        for (opcode, runtime) in [(OpCode::Div, RuntimeFn::Div), (OpCode::Mod, RuntimeFn::Mod)] {
            let context = inkwell::context::Context::create();
            let mut backend = LlvmBackend::new(&context, "float_divrem_semantics");
            let mut func = TirFunction::new(
                "float_divrem".into(),
                vec![TirType::F64, TirType::F64],
                TirType::F64,
                molt_ir::FunctionReturnAbi::Value,
            );
            let result = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode,
                operands: vec![ValueId(0), ValueId(1)],
                results: vec![result],
                attrs: AttrDict::from_iter([("fast_math".into(), AttrValue::Bool(true))]),
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![result],
            };
            // Lowerer-only raw-ABI control. Parameter annotations alone are
            // deliberately boxed in production; this test exercises the
            // machine float operation's zero, NaN and remainder-sign rules.
            let mut facts = crate::representation_plan::ProducerCarrierPlan::default();
            for value in [ValueId(0), ValueId(1), result] {
                facts
                    .repr_by_value
                    .insert(value, molt_ir::repr::Repr::FloatUnboxed);
            }
            backend.function_repr_facts.insert(func.name.clone(), facts);
            try_lower_tir_to_llvm(&func, &backend).unwrap();
            backend.module.verify().unwrap();
            let ir = backend.module.print_to_string().to_string();
            // The walker's model tests own the values; LLVM must keep the
            // zero-divisor test, its raising runtime entry and IEEE rules.
            assert!(ir.contains("fcmp une"));
            assert!(ir.contains(runtime.linker_symbol()));
            assert!(
                !ir.contains("fdiv fast") && !ir.contains("frem fast"),
                "fast-math must not erase zero/NaN/sign semantics"
            );
        }
    }
}
