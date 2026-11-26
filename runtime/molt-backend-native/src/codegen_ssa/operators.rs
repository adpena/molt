//! Operators (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "numeric"): arithmetic, comparison, bitwise, unary, identity, membership,
//! `and`/`or` selection, truth, and the late stage's word primitives, as ops.
//! Each takes the raw lane (`numeric`) where the plan proves one, else the
//! boxed runtime entry through operation-local custody.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::WordComparison;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::{CodegenBuilder, MachineKind};
use super::custody::{CallTarget, RuntimeArg};
use super::function::{FunctionCx, ResultCustody};
use super::numeric::{Operand, binary_arith, bitwise, checked_arith, comparison, divide, unary};
use super::scalar;
use super::value_table::HeldCarrier;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

/// The runtime entry of a numeric op `name`, from the one generated table.
fn numeric_runtime(name: OpKind) -> RuntimeFn {
    molt_ir::runtime_fn_generated::numeric_runtime_fn(name)
        .unwrap_or_else(|| unreachable!("{name} has no numeric runtime entry"))
}

/// The runtime entry that implements `op` on boxed operands
/// (`TirOp::numeric_runtime`: an augmented assignment's in-place entry), else
/// the entry of the binary `name`.
fn boxed_numeric_runtime(op: &TirOp, name: OpKind) -> RuntimeFn {
    op.numeric_runtime()
        .unwrap_or_else(|| numeric_runtime(name))
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// `id` as the numeric lanes take an operand.
    fn numeric_operand(&self, b: &mut B, id: ValueId) -> Operand<B::Value> {
        let operands = self.operands();
        Operand {
            id,
            value: operands.resolve(b, id),
            carrier: operands.held(id),
        }
    }

    /// The boxed runtime lane of a binary operator: both operands borrowed
    /// through one custody; the owned result is the operator's value.
    fn boxed_binary(
        &mut self,
        b: &mut B,
        function: RuntimeFn,
        lhs: ValueId,
        rhs: ValueId,
    ) -> B::Value {
        let callee = CallTarget::Callee(b.runtime_callee(function));
        let word = self.operands().call(
            b,
            callee,
            &[RuntimeArg::Operand(lhs), RuntimeArg::Operand(rhs)],
            false,
            "binary",
            function.symbol(),
        );
        b.word_value(word)
    }

    /// `+ - * / // % **` and their in-place forms: the raw lane, else the
    /// boxed entry, which honours the in-place dunder protocol for an
    /// augmented assignment (`molt_inplace_<op>` tries `__i<op>__` first).
    pub(crate) fn binary_arith_op(&mut self, b: &mut B, op: &TirOp, name: OpKind) {
        let result = op.results[0];
        let (lhs, rhs) = (op.operands[0], op.operands[1]);
        let (left, right) = (self.numeric_operand(b, lhs), self.numeric_operand(b, rhs));
        let raw = match name {
            OpKind::Div | OpKind::Floordiv | OpKind::Mod => {
                divide(b, self.plan, name, result, left, right)
            }
            _ => binary_arith(b, self.plan, op, name, left, right),
        };
        let (value, carrier) = raw.unwrap_or_else(|| {
            let value = self.boxed_binary(b, boxed_numeric_runtime(op, name), lhs, rhs);
            (value, HeldCarrier::Boxed)
        });
        self.bind(result, value, carrier);
    }

    /// `CheckedAdd`/`CheckedMul`, the overflow peel's ops: the raw lane, else
    /// the exact boxed entry (BigInt), whose overflow bit is constant false.
    pub(crate) fn checked_arith_op(&mut self, b: &mut B, op: &TirOp) {
        let [value_id, overflow_id] = [op.results[0], op.results[1]];
        let [lhs, rhs] = [op.operands[0], op.operands[1]];
        let (left, right) = (self.numeric_operand(b, lhs), self.numeric_operand(b, rhs));
        let ((value, carrier), overflow) = match checked_arith(b, self.plan, op.opcode, left, right)
        {
            Some((value, overflow)) => ((value, HeldCarrier::RawI64), overflow),
            None => {
                let runtime = match op.opcode {
                    OpCode::CheckedAdd => RuntimeFn::Add,
                    _ => RuntimeFn::Mul,
                };
                let boxed = self.boxed_binary(b, runtime, lhs, rhs);
                let never = b.bit_constant(false);
                ((boxed, HeldCarrier::Boxed), b.bit_value(never))
            }
        };
        self.bind(value_id, value, carrier);
        self.bind(overflow_id, overflow, HeldCarrier::RawBool);
    }

    /// `== != < <= > >=`: the raw lane, else the boxed entry.
    pub(crate) fn comparison_op(&mut self, b: &mut B, op: &TirOp, name: OpKind) {
        let result = op.results[0];
        let (lhs, rhs) = (op.operands[0], op.operands[1]);
        let (left, right) = (self.numeric_operand(b, lhs), self.numeric_operand(b, rhs));
        let (value, carrier) = comparison(b, name, left, right).unwrap_or_else(|| {
            let value = self.boxed_binary(b, numeric_runtime(name), lhs, rhs);
            (value, HeldCarrier::Boxed)
        });
        self.bind(result, value, carrier);
    }

    /// `& | ^ << >>`: the raw lane, else the boxed entry (in-place aware, as
    /// [`Self::binary_arith_op`]). The entries take boxed operands, so a raw
    /// int is boxed as a borrowed operand, never passed as its bits.
    pub(crate) fn bitwise_op(&mut self, b: &mut B, op: &TirOp, name: OpKind) {
        let result = op.results[0];
        let (lhs, rhs) = (op.operands[0], op.operands[1]);
        let (left, right) = (self.numeric_operand(b, lhs), self.numeric_operand(b, rhs));
        let (value, carrier) = bitwise(b, self.plan, op, name, left, right).unwrap_or_else(|| {
            let value = self.boxed_binary(b, boxed_numeric_runtime(op, name), lhs, rhs);
            (value, HeldCarrier::Boxed)
        });
        self.bind(result, value, carrier);
    }

    /// `- + not ~`: the raw lane, else the boxed entry, which borrows the
    /// boxed object (`~True` needs its Bool tag).
    pub(crate) fn unary_op(&mut self, b: &mut B, op: &TirOp, name: OpKind) {
        let result = op.results[0];
        let operand = op.operands[0];
        let input = self.numeric_operand(b, operand);
        let (value, carrier) = match unary(b, self.plan, op, name, input) {
            Some(raw) => raw,
            None => {
                let function = match name {
                    OpKind::Not => RuntimeFn::Not,
                    numeric => numeric_runtime(numeric),
                };
                let callee = CallTarget::Callee(b.runtime_callee(function));
                let word = self.operands().call(
                    b,
                    callee,
                    &[RuntimeArg::Operand(operand)],
                    false,
                    "unary",
                    name.as_str(),
                );
                (b.word_value(word), HeldCarrier::Boxed)
            }
        };
        self.bind(result, value, carrier);
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// `is` / `is not`: identity of object words, never of raw payloads (a
    /// boxed False and a raw zero bit are one singleton). One custody boxes
    /// each operand once; a failed box yields None, which is not 1, so only a
    /// committed machine 1 is true, for `is not` too.
    pub(crate) fn identity_op(&mut self, b: &mut B, op: &TirOp) {
        let result = op.results[0];
        let invert = op.opcode == OpCode::IsNot;
        let word = self
            .operands()
            .with_words(b, &op.operands, "identity", |b, words, name| {
                let same = b.compare(WordComparison::Equal, words[0], words[1]);
                let flag = if invert { b.bit_not(same) } else { same };
                let flag = b.bit_value(flag);
                let word = b.zero_extend(flag);
                let named = b.word_value(word);
                b.set_name(named, name);
                word
            });
        let one = b.constant(1);
        let truth = b.compare(WordComparison::Equal, word, one);
        let truth = b.bit_value(truth);
        b.set_name(truth, "identity_bool");
        self.bind(result, truth, HeldCarrier::RawBool);
    }

    /// `in` / `not in`: `molt_contains(container, item)` (operands in that
    /// order); `not in` keeps only the inverted truth of the membership
    /// result and releases an owned one.
    pub(crate) fn containment_op(&mut self, b: &mut B, op: &TirOp) {
        let contains = CallTarget::Callee(b.runtime_callee(RuntimeFn::Contains));
        let args = [
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
        ];
        let custody = ResultCustody::canonical(RuntimeFn::Contains, 2);
        if op.opcode != OpCode::NotIn {
            self.runtime_call(
                b,
                op,
                contains,
                &args,
                custody,
                "contains",
                RuntimeFn::Contains.symbol(),
            );
            return;
        }
        let word = self.operands().call(
            b,
            contains,
            &args,
            false,
            "contains",
            RuntimeFn::Contains.symbol(),
        );
        let truthy = b
            .call_runtime(RuntimeFn::IsTruthy, &[word])
            .expect("molt_is_truthy answers a word");
        let zero = b.constant(0);
        let not_in = b.compare(WordComparison::Equal, truthy, zero);
        let not_in = b.bit_value(not_in);
        b.set_name(not_in, "not_in");
        if custody == ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue) {
            let release = b.refcount_site(RefcountOperation::Release);
            b.refcount(release, word);
        }
        self.bind(op.results[0], not_in, HeldCarrier::RawBool);
    }

    /// `and` / `or` select an operand inside already-structured control flow
    /// (Python selection, not bitwise). The result owns one reference to the
    /// selected operand. Two object carriers select a word and retain it;
    /// otherwise each edge materializes only its own operand, so a scalar is
    /// boxed once, only when selected, and a minted heap int is the owner.
    pub(crate) fn boolean_selection_op(&mut self, b: &mut B, op: &TirOp) {
        assert!(
            crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
                op.opcode
            ),
            "{:?} must mint an owned selected operand",
            op.opcode
        );
        let result = op.results[0];
        let operands = self.operands();
        let condition = {
            let value = operands.resolve(b, op.operands[0]);
            scalar::truth(b, value, operands.held(op.operands[0]))
        };
        // `a and b` is `b` when `a` is truthy; `a or b` is `a`.
        let (name, when_true, when_false) = if op.opcode == OpCode::And {
            ("bool_and", op.operands[1], op.operands[0])
        } else {
            ("bool_or", op.operands[0], op.operands[1])
        };
        let object = |id: ValueId| {
            !matches!(
                operands.held(id),
                HeldCarrier::RawI64
                    | HeldCarrier::RawF64
                    | HeldCarrier::RawBool
                    | HeldCarrier::Never
            )
        };
        let selected = if object(when_true) && object(when_false) {
            let yes = operands.resolve(b, when_true);
            let yes = scalar::word_of(b, yes);
            let no = operands.resolve(b, when_false);
            let no = scalar::word_of(b, no);
            let (yes, no) = (b.word_value(yes), b.word_value(no));
            let selected = b.select(condition, yes, no);
            b.set_name(selected, name);
            let selected = b.expect_word(selected);
            if !self.plan.boxed_refcount_is_noop(result) {
                let retain = b.refcount_site(RefcountOperation::Retain);
                b.refcount(retain, selected);
            }
            selected
        } else {
            let yes_block = b.create_block(&format!("{name}_true"));
            let no_block = b.create_block(&format!("{name}_false"));
            let merge = b.create_block(&format!("{name}_merge"));
            let joined = b.append_block_param(merge, MachineKind::Word);
            b.set_name(joined, name);
            b.branch_if(condition, (yes_block, &[]), (no_block, &[]));
            for (block, operand) in [(yes_block, when_true), (no_block, when_false)] {
                b.switch_to_block(block);
                let word = operands.owned_word(b, operand);
                let word = b.word_value(word);
                b.jump(merge, &[word]);
            }
            b.switch_to_block(merge);
            b.expect_word(joined)
        };
        let selected = b.word_value(selected);
        self.bind(result, selected, HeldCarrier::Boxed);
    }

    /// `Bool`: the truth of the operand, as a raw bit.
    pub(crate) fn bool_op(&mut self, b: &mut B, op: &TirOp) {
        let operands = self.operands();
        let value = operands.resolve(b, op.operands[0]);
        let bit = scalar::truth(b, value, operands.held(op.operands[0]));
        let bit = b.bit_value(bit);
        self.bind(op.results[0], bit, HeldCarrier::RawBool);
    }

    /// The late stage's representation primitives (HF-821): word tests and
    /// payload reads, each one shared word generator.
    ///
    /// A test or read asks about its operand's boxed word, whatever carrier
    /// the operand is held in: the representation facts may hold a value raw
    /// after the late stage guarded it as a word (HF-899: a try/except join of
    /// two int constants). A raw float is its own word and a raw bool is one
    /// tag away; a raw integer is answered from its carrier, since boxing it
    /// may mint a heap owner.
    pub(crate) fn word_primitive_op(&mut self, b: &mut B, op: &TirOp) {
        use nanbox::boxed_word::{self as word, BoxTag};
        let operand = op.operands[0];
        let held = self.operands().held(operand);
        let value = self.operands().resolve(b, operand);
        let raw = scalar::word_of(b, value);
        let reads_word = !matches!(op.opcode, OpCode::IntToFloat | OpCode::BoxInlineInt);
        if reads_word && held == HeldCarrier::RawI64 {
            let (value, carrier) = match op.opcode {
                OpCode::IsInlineInt => {
                    let bit = if self.plan.is_inline_safe_int(operand) {
                        b.bit_constant(true)
                    } else {
                        word::generate_inline_int_fits(b, raw)
                    };
                    (b.bit_value(bit), HeldCarrier::RawBool)
                }
                OpCode::UnboxInlineInt => (b.word_value(raw), HeldCarrier::RawI64),
                OpCode::IsFloatWord | OpCode::IsBoolWord => {
                    let bit = b.bit_constant(false);
                    (b.bit_value(bit), HeldCarrier::RawBool)
                }
                // Only reachable under a test above that is false for an int.
                OpCode::UnboxFloatWord => (b.int_to_float(raw), HeldCarrier::RawF64),
                OpCode::UnboxBoolWord => {
                    let bit = b.low_bit(raw);
                    (b.bit_value(bit), HeldCarrier::RawBool)
                }
                other => unreachable!("{other:?} is not a word test or read"),
            };
            self.bind(op.results[0], value, carrier);
            return;
        }
        let input = if reads_word && held == HeldCarrier::RawBool {
            b.or(raw, nanbox::QNAN | nanbox::TAG_BOOL)
        } else {
            raw
        };
        let (value, carrier) = match op.opcode {
            OpCode::IsInlineInt => {
                let bit = word::generate_has_tag(b, input, BoxTag::Int);
                (b.bit_value(bit), HeldCarrier::RawBool)
            }
            OpCode::UnboxInlineInt => {
                let raw = word::generate_unbox_inline_int(b, input);
                (b.word_value(raw), HeldCarrier::RawI64)
            }
            OpCode::IsFloatWord => {
                let bit = word::generate_is_float(b, input);
                (b.bit_value(bit), HeldCarrier::RawBool)
            }
            OpCode::UnboxFloatWord => (b.bits_to_float(input), HeldCarrier::RawF64),
            OpCode::IntToFloat => (b.int_to_float(input), HeldCarrier::RawF64),
            OpCode::IsBoolWord => {
                let bit = word::generate_has_tag(b, input, BoxTag::Bool);
                (b.bit_value(bit), HeldCarrier::RawBool)
            }
            OpCode::UnboxBoolWord => {
                let bit = word::generate_unbox_bool(b, input);
                (b.bit_value(bit), HeldCarrier::RawBool)
            }
            OpCode::BoxInlineInt => {
                let boxed = word::generate_box_inline_int(b, input);
                (b.word_value(boxed), HeldCarrier::Boxed)
            }
            other => unreachable!("{other:?} is not a word primitive"),
        };
        self.bind(op.results[0], value, carrier);
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Lower an operator op (see the module documentation).
    pub(crate) fn operator_op(&mut self, b: &mut B, op: &TirOp) {
        match op.opcode {
            OpCode::Add | OpCode::InplaceAdd => self.binary_arith_op(b, op, OpKind::Add),
            OpCode::Sub | OpCode::InplaceSub => self.binary_arith_op(b, op, OpKind::Sub),
            OpCode::Mul | OpCode::InplaceMul => self.binary_arith_op(b, op, OpKind::Mul),
            OpCode::Div => self.binary_arith_op(b, op, OpKind::Div),
            OpCode::FloorDiv => self.binary_arith_op(b, op, OpKind::Floordiv),
            OpCode::Mod => self.binary_arith_op(b, op, OpKind::Mod),
            OpCode::Pow => self.binary_arith_op(b, op, OpKind::Pow),
            OpCode::CheckedAdd | OpCode::CheckedMul => self.checked_arith_op(b, op),
            OpCode::TryBuffer2dLoadI64 => self.try_buffer_load(b, op),
            OpCode::TryBuffer2dStoreI64 => self.try_buffer_store(b, op),
            OpCode::Neg => self.unary_op(b, op, OpKind::Neg),
            OpCode::Pos => self.unary_op(b, op, OpKind::Pos),
            OpCode::Not => self.unary_op(b, op, OpKind::Not),
            OpCode::BitNot => self.unary_op(b, op, OpKind::Invert),
            OpCode::Eq => self.comparison_op(b, op, OpKind::Eq),
            OpCode::Ne => self.comparison_op(b, op, OpKind::Ne),
            OpCode::Lt => self.comparison_op(b, op, OpKind::Lt),
            OpCode::Le => self.comparison_op(b, op, OpKind::Le),
            OpCode::Gt => self.comparison_op(b, op, OpKind::Gt),
            OpCode::Ge => self.comparison_op(b, op, OpKind::Ge),
            OpCode::Is | OpCode::IsNot => self.identity_op(b, op),
            OpCode::In | OpCode::NotIn => self.containment_op(b, op),
            OpCode::BitAnd => self.bitwise_op(b, op, OpKind::BitAnd),
            OpCode::BitOr => self.bitwise_op(b, op, OpKind::BitOr),
            OpCode::BitXor => self.bitwise_op(b, op, OpKind::BitXor),
            OpCode::Shl => self.bitwise_op(b, op, OpKind::Lshift),
            OpCode::Shr => self.bitwise_op(b, op, OpKind::Rshift),
            OpCode::And | OpCode::Or => self.boolean_selection_op(b, op),
            OpCode::Bool => self.bool_op(b, op),
            OpCode::IsInlineInt
            | OpCode::UnboxInlineInt
            | OpCode::IsFloatWord
            | OpCode::UnboxFloatWord
            | OpCode::IntToFloat
            | OpCode::IsBoolWord
            | OpCode::UnboxBoolWord
            | OpCode::BoxInlineInt => self.word_primitive_op(b, op),
            other => unreachable!("{other:?} is not an operator"),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Operators on the interpreting builder and as Cranelift machine code.
    //! The oracle is Python's meaning of each op and the runtime model's
    //! answer for the boxed entry it calls.

    use std::cell::RefCell;

    use super::super::differential::{Run, both, kernel, machine};
    use super::super::scalar::tests::{HEAP_INT, NONE_WORD, Value, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::types::TirType;
    use molt_ir::repr::Repr;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;
    const OTHER: u64 = (0x7ff8 << 48) | (4 << 48) | 0x3000;
    const TRUE_WORD: u64 = (0x7ff8 << 48) | (2 << 48) | 1;

    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    fn key(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    /// Lower `opcode` over %0.. (the inputs, held as `carriers`), results
    /// %10..; returns each result and its carrier.
    fn operator(
        opcode: OpCode,
        carriers: &[HeldCarrier],
        inputs: &[Value],
        results: usize,
        plan: &[(ValueId, Repr)],
        machine_only: bool,
    ) -> Run<Vec<Option<HeldCarrier>>> {
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..inputs.len() as u32).map(ValueId).collect(),
            results: (0..results as u32)
                .map(|index| ValueId(10 + index))
                .collect(),
            attrs: AttrDict::default(),
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        };
        let mut facts = ProducerCarrierPlan::default();
        facts.repr_by_value.extend(plan.iter().copied());
        let lower = kernel!(
            op: TirOp = op,
            plan: ProducerCarrierPlan = facts,
            carriers: Vec<HeldCarrier> = carriers.to_vec();
            |b, inputs| -> Vec<Option<HeldCarrier>> {
                let func = TirFunction::new(
                    "operators".into(),
                    vec![],
                    TirType::None,
                    molt_ir::FunctionReturnAbi::Value,
                );
                let mut values = ValueTable::with_capacity(16);
                for (index, (&input, &carrier)) in inputs.iter().zip(carriers.iter()).enumerate() {
                    values.set_value(ValueId(index as u32), input);
                    values.set_repr(ValueId(index as u32), carrier);
                }
                let diagnostics = RefCell::new(Vec::new());
                let mut cx = FunctionCx::<B> {
                    func: &func,
                    plan,
                    values: &mut values,
                    diagnostics: &diagnostics,
                    runtime: &super::super::differential::every_runtime_entry(),
                    linkage: &Default::default(),
                    functions: &Default::default(),
                    call_sites: &std::cell::Cell::new(0),
                };
                cx.operator_op(b, op);
                assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
                let ids = &op.results;
                let facts = ids.iter().map(|&id| values.repr(id)).collect();
                (facts, ids.iter().map(|&id| values.value(id).unwrap()).collect())
            }
        );
        if machine_only {
            machine(&lower, inputs, false)
        } else {
            both(&lower, inputs)
        }
    }

    const BOXED: HeldCarrier = HeldCarrier::Boxed;

    #[test]
    fn packed_store_commits_only_after_all_operand_checks() {
        use molt_codegen_abi::{
            BUFFER2D_STORAGE_PACKED, HEADER_SIZE_BYTES, TYPE_ID_BUFFER2D, box_ptr_bits,
        };
        let mut cells = [7i64];
        let header_words = HEADER_SIZE_BYTES as usize / size_of::<u64>();
        let mut object = [0u64; HEADER_SIZE_BYTES as usize / size_of::<u64>() + 4];
        object[0] = u64::from(TYPE_ID_BUFFER2D);
        object[header_words..].copy_from_slice(&[
            1,
            1,
            BUFFER2D_STORAGE_PACKED,
            cells.as_mut_ptr() as u64,
        ]);
        let owner = box_ptr_bits(object[header_words..].as_ptr() as u64) as u64;
        let none = molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE;
        let run_store = |col, word, held| {
            operator(
                OpCode::TryBuffer2dStoreI64,
                &[BOXED, HeldCarrier::RawI64, HeldCarrier::RawI64, held],
                &[
                    Value::Word(owner),
                    Value::Word(0),
                    Value::Word(col),
                    Value::Word(word),
                ],
                1,
                &[],
                true,
            )
        };

        let success = !molt_codegen_abi::MOLT_REFCOUNT_ATOMIC;
        let committed = if success { i64::MAX } else { 7 };
        let hot = run_store(0, i64::MAX as u64, HeldCarrier::RawI64);
        assert_eq!(hot.results, [Value::Bit(success)]);
        assert_eq!(cells, [committed]);
        for miss in [
            run_store(1, 0, HeldCarrier::RawI64),
            run_store(0, none, BOXED),
        ] {
            assert_eq!(miss.results, [Value::Bit(false)]);
            assert!(miss.runtime_calls.is_empty() && miss.refcounts.is_empty());
            assert_eq!(cells, [committed]);
        }
        assert!(hot.runtime_calls.is_empty() && hot.refcounts.is_empty());
    }

    #[test]
    fn packed_buffer_load_preserves_i64_and_misses_without_a_runtime_call() {
        use molt_codegen_abi::{
            BUFFER2D_STORAGE_PACKED, HEADER_SIZE_BYTES, TYPE_ID_BUFFER2D, box_ptr_bits,
        };
        let cells = [i64::MIN, i64::MAX];
        let header_words = HEADER_SIZE_BYTES as usize / size_of::<u64>();
        let mut object = [0u64; HEADER_SIZE_BYTES as usize / size_of::<u64>() + 4];
        object[0] = u64::from(TYPE_ID_BUFFER2D);
        object[header_words..].copy_from_slice(&[
            1,
            2,
            BUFFER2D_STORAGE_PACKED,
            cells.as_ptr() as u64,
        ]);
        let owner = box_ptr_bits(object[header_words..].as_ptr() as u64) as u64;
        let run = |owner, row, col, expected: Option<i64>| {
            let run = operator(
                OpCode::TryBuffer2dLoadI64,
                &[BOXED, HeldCarrier::RawI64, BOXED],
                &[
                    Value::Word(owner),
                    Value::Word(row as u64),
                    Value::Word(col),
                ],
                2,
                &[],
                true,
            );
            let expected = expected.filter(|_| !molt_codegen_abi::MOLT_REFCOUNT_ATOMIC);
            assert_eq!(
                run.results,
                [
                    Value::Word(expected.unwrap_or(0) as u64),
                    Value::Bit(expected.is_some())
                ]
            );
            assert_eq!(
                run.fact,
                [Some(HeldCarrier::RawI64), Some(HeldCarrier::RawBool)]
            );
            assert!(run.runtime_calls.is_empty() && run.refcounts.is_empty());
        };
        run(owner, 0i64, inline(0), Some(i64::MIN));
        run(owner, -1, inline(-1), Some(i64::MAX));
        run(owner, i64::MIN, inline(0), None);
        run(owner, 0, inline(2), None);
        run(owner, 0, NONE_WORD, None);
        run(NONE_WORD, 0, inline(0), None);
        object[header_words + 2] = 1;
        std::hint::black_box(&object);
        run(owner, 0, inline(0), None);
    }

    #[test]
    fn identity_compares_object_words_and_boxes_a_raw_operand_once() {
        for (opcode, left, right, expected) in [
            (OpCode::Is, OBJECT, OBJECT, true),
            (OpCode::Is, OBJECT, OTHER, false),
            (OpCode::IsNot, OBJECT, OTHER, true),
            (OpCode::IsNot, OBJECT, OBJECT, false),
        ] {
            let run = operator(
                opcode,
                &[BOXED, BOXED],
                &[Value::Word(left), Value::Word(right)],
                1,
                &[],
                false,
            );
            assert_eq!(run.results, [Value::Bit(expected)], "{opcode:?}");
            assert_eq!(run.fact, [Some(HeldCarrier::RawBool)]);
        }
        // A raw bool is its boxed singleton: `True is True`.
        let run = operator(
            OpCode::Is,
            &[HeldCarrier::RawBool, BOXED],
            &[Value::Bit(true), Value::Word(TRUE_WORD)],
            1,
            &[],
            false,
        );
        assert_eq!(run.results, [Value::Bit(true)]);
        // A full-range raw int mints, joins the failure path, and releases
        // its owner once; the machine runs that path.
        let run = operator(
            OpCode::Is,
            &[HeldCarrier::RawI64, BOXED],
            &[Value::Word(1 << 50), Value::Word(HEAP_INT)],
            1,
            &[],
            true,
        );
        assert_eq!(
            run.results,
            [Value::Bit(true)],
            "the minted int is the heap word"
        );
        assert_eq!(run.refcounts, [(RefcountOperation::Release, HEAP_INT)]);
    }

    #[test]
    fn membership_calls_contains_and_not_in_inverts_its_truth() {
        let inputs = [Value::Word(OBJECT), Value::Word(inline(3))];
        let contained = literal_word(RuntimeFn::Contains, &key(&[OBJECT, inline(3)]));
        let run = operator(OpCode::In, &[BOXED, BOXED], &inputs, 1, &[], false);
        assert_eq!(run.results, [Value::Word(contained)]);
        let run = operator(OpCode::NotIn, &[BOXED, BOXED], &inputs, 1, &[], false);
        // The model's membership word is an object, so it is truthy.
        assert_eq!(run.results, [Value::Bit(false)]);
        assert_eq!(
            run.runtime_calls,
            [RuntimeFn::Contains, RuntimeFn::IsTruthy]
        );
        assert_eq!(
            run.refcounts,
            [(RefcountOperation::Release, contained)],
            "the owned membership word is released"
        );
    }

    #[test]
    fn selection_returns_an_owned_operand_by_python_truth() {
        // Two objects: select and retain.
        for (opcode, condition, expected) in [
            (OpCode::And, OBJECT, OTHER),
            (OpCode::Or, OBJECT, OBJECT),
            (OpCode::And, NONE_WORD, NONE_WORD),
            (OpCode::Or, NONE_WORD, OTHER),
        ] {
            let run = operator(
                opcode,
                &[BOXED, BOXED],
                &[Value::Word(condition), Value::Word(OTHER)],
                1,
                &[],
                false,
            );
            assert_eq!(run.results, [Value::Word(expected)], "{opcode:?}");
            let retained: Vec<_> = run
                .refcounts
                .iter()
                .filter(|(operation, _)| *operation == RefcountOperation::Retain)
                .collect();
            // None is immortal: its retain never reaches the runtime.
            let reaching: &[&(RefcountOperation, u64)] = if expected == NONE_WORD {
                &[]
            } else {
                &[&(RefcountOperation::Retain, expected)]
            };
            assert_eq!(retained, reaching, "{opcode:?}");
        }
        // A raw operand is boxed only on the edge that selects it.
        for (condition, expected) in [(0, inline(0)), (5, OTHER)] {
            let run = operator(
                OpCode::And,
                &[HeldCarrier::RawI64, BOXED],
                &[Value::Word(condition), Value::Word(OTHER)],
                1,
                &[(ValueId(0), Repr::RawI64Safe)],
                true,
            );
            assert_eq!(run.results, [Value::Word(expected)], "{condition}");
        }
    }

    #[test]
    fn a_boxed_operand_takes_the_runtime_entry() {
        let inputs = [Value::Word(OBJECT), Value::Word(OTHER)];
        let run = operator(OpCode::Add, &[BOXED, BOXED], &inputs, 1, &[], false);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::Add,
                &key(&[OBJECT, OTHER])
            ))]
        );
        let run = operator(OpCode::Eq, &[BOXED, BOXED], &inputs, 1, &[], false);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::Eq,
                &key(&[OBJECT, OTHER])
            ))]
        );
        let run = operator(OpCode::Not, &[BOXED], &inputs[..1], 1, &[], false);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::Not, &key(&[OBJECT])))]
        );
        // The checked ops' boxed lane is exact: overflow is constant false.
        let run = operator(OpCode::CheckedAdd, &[BOXED, BOXED], &inputs, 2, &[], false);
        assert_eq!(
            run.results,
            [
                Value::Word(literal_word(RuntimeFn::Add, &key(&[OBJECT, OTHER]))),
                Value::Bit(false),
            ]
        );
    }

    #[test]
    fn truth_and_the_word_primitives_follow_the_nan_box_layout() {
        for (carrier, input, expected) in [
            (HeldCarrier::RawI64, Value::Word(0), false),
            (HeldCarrier::RawF64, Value::Float(f64::NAN), true),
            (BOXED, Value::Word(NONE_WORD), false),
        ] {
            let run = operator(OpCode::Bool, &[carrier], &[input], 1, &[], false);
            assert_eq!(run.results, [Value::Bit(expected)], "{input:?}");
        }
        for (opcode, input, expected) in [
            (OpCode::IsInlineInt, inline(-3), Value::Bit(true)),
            (OpCode::IsInlineInt, OBJECT, Value::Bit(false)),
            (
                OpCode::UnboxInlineInt,
                inline(-3),
                Value::Word(-3i64 as u64),
            ),
            (OpCode::IsFloatWord, 1.5f64.to_bits(), Value::Bit(true)),
            (OpCode::IsFloatWord, NONE_WORD, Value::Bit(false)),
            (OpCode::UnboxFloatWord, 1.5f64.to_bits(), Value::Float(1.5)),
            (OpCode::IsBoolWord, TRUE_WORD, Value::Bit(true)),
            (OpCode::UnboxBoolWord, TRUE_WORD, Value::Bit(true)),
            (OpCode::IntToFloat, 7, Value::Float(7.0)),
            (OpCode::BoxInlineInt, -3i64 as u64, Value::Word(inline(-3))),
        ] {
            let run = operator(opcode, &[BOXED], &[Value::Word(input)], 1, &[], false);
            assert_eq!(run.results, [expected], "{opcode:?} {input:#x}");
        }
    }
}
