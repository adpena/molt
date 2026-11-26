//! Constants (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "constants"): literal values and program-slot reads, written once.
//!
//! A scalar literal is a machine constant in its raw carrier. A string,
//! bytes or big-integer literal is one load of its cell (call_anatomy.md 4.9):
//! the object's private, writable word that the literal's first evaluation
//! fills, through one cold runtime call, with the literal's immortal object.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::literal_cell::LiteralKind;
use molt_ir::code_slots::ProgramSlotKind;
use molt_ir::literal_payload::required_tir_literal_bytes;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, IntComparison, MachineKind};
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;

/// The cell of the literal (`kind`, `bytes`) in this object: one private,
/// writable, zero-initialized word named by its kind and payload digest.
pub(crate) fn literal_cell<B: CodegenBuilder>(
    b: &mut B,
    kind: LiteralKind,
    bytes: &[u8],
) -> B::Value {
    let symbol = molt_ir::code_slots::literal_cell_symbol(kind as u64, bytes);
    b.site_record(&symbol, &[0; 8])
}

/// Read the provider's literal cell only when its body is emitted here.
/// Identical literal payloads in other objects fill independent cells.
fn string_literal_cell_ready<B: CodegenBuilder>(
    b: &mut B,
    op: &TirOp,
    functions: &molt_ir::FunctionTable,
) -> B::Predicate {
    let provider = op.callee.expect("verified literal cell provider");
    if functions.defines(provider)
        && let Some(AttrValue::Bytes(bytes)) = op.attrs.get("bytes")
    {
        let cell = literal_cell(b, LiteralKind::Str, bytes);
        let word = b.load_word(cell, 0);
        let empty = b.constant(0);
        b.int_compare(IntComparison::NotEqual, word, empty)
    } else {
        b.bit_constant(false)
    }
}

/// The cold first evaluation of a literal: `molt_literal_cell_fill` builds
/// the object, publishes it into `cell` immortal and returns its word, or
/// returns None with the exception pending and leaves the cell zero.
pub(crate) fn fill_literal_cell<B: CodegenBuilder>(
    b: &mut B,
    cell: B::Value,
    kind: LiteralKind,
    bytes: &[u8],
) -> B::Value {
    let data = b.bytes_constant(bytes, "__molt_literal_bytes_", "");
    let callee = b.runtime_callee(RuntimeFn::LiteralCellFill);
    let kind_word = b.constant(kind as u64);
    let len = b.constant(bytes.len() as u64);
    let args = [cell, data, b.word_value(kind_word), b.word_value(len)];
    b.call_cold(callee, &args)
        .expect("molt_literal_cell_fill returns the literal's word")
}

/// A str, bytes or big-integer literal: its cell's word once filled. The
/// first evaluation finds the cell zero and calls `molt_literal_cell_fill`,
/// which builds the object, publishes it into the cell and makes it immortal
/// (HF-856). The result is a borrowed immortal object; releasing it is a
/// no-op. On a failed construction the fill returns None with the exception
/// pending, and the check after the op observes it.
fn literal<B: CodegenBuilder>(b: &mut B, kind: LiteralKind, bytes: &[u8]) -> B::Value {
    let cell = literal_cell(b, kind, bytes);
    let cached = b.load_word(cell, 0);
    let cached = b.word_value(cached);
    let empty = b.is_zero(cached);
    let fill = b.create_block("literal_fill");
    let done = b.create_block("literal_join");
    let answer = b.append_block_param(done, MachineKind::Word);
    b.branch_if(empty, (fill, &[]), (done, &[cached]));
    b.switch_to_block(fill);
    let filled = fill_literal_cell(b, cell, kind, bytes);
    b.jump(done, &[filled]);
    b.switch_to_block(done);
    b.set_name(answer, "literal");
    answer
}

/// Lower constants, program slots, and read-only literal cell observations.
pub(crate) fn lower<B: CodegenBuilder>(cx: &mut FunctionCx<'_, B>, b: &mut B, op: &TirOp) {
    let Some(&result) = op.results.first() else {
        // A discarded literal has no evaluation-time work.
        return;
    };
    let (value, ty) = match op.opcode {
        OpCode::ConstInt => {
            let value = match op.attrs.get("value") {
                Some(AttrValue::Int(value)) => *value,
                other => panic!("ConstInt missing integer value attribute: {other:?}"),
            };
            let word = b.constant(value as u64);
            (b.word_value(word), TirType::I64)
        }
        OpCode::ConstFloat => {
            let value = match op.attrs.get("f_value").or_else(|| op.attrs.get("value")) {
                Some(AttrValue::Float(value)) => *value,
                other => panic!("ConstFloat missing float value attribute: {other:?}"),
            };
            (b.float_constant(value), TirType::F64)
        }
        OpCode::ConstBool => {
            let value = match op.attrs.get("value") {
                Some(AttrValue::Bool(value)) => *value,
                Some(AttrValue::Int(value)) => *value != 0,
                other => panic!("ConstBool missing bool value attribute: {other:?}"),
            };
            let bit = b.bit_constant(value);
            (b.bit_value(bit), TirType::Bool)
        }
        OpCode::ConstNone => {
            let word = b.constant(nanbox::QNAN | nanbox::TAG_NONE);
            (b.word_value(word), TirType::None)
        }
        OpCode::ConstStr => (
            literal(b, LiteralKind::Str, required_tir_literal_bytes(op)),
            TirType::Str,
        ),
        OpCode::ConstBytes => (
            literal(b, LiteralKind::Bytes, required_tir_literal_bytes(op)),
            TirType::Bytes,
        ),
        OpCode::ConstBigInt => (
            literal(b, LiteralKind::BigInt, required_tir_literal_bytes(op)),
            TirType::DynBox,
        ),
        OpCode::StringLiteralCellReady => {
            let ready = string_literal_cell_ready(b, op, cx.functions);
            (b.bit_value(ready), TirType::Bool)
        }
        OpCode::ModuleSlot => {
            let owner = match op.attrs.get("s_value") {
                Some(AttrValue::Str(owner)) => owner.as_str(),
                _ => panic!("admitted program-slot owner"),
            };
            let word = b.program_slot(ProgramSlotKind::Module, owner);
            (b.word_value(word), TirType::I64)
        }
        other => unreachable!("{other:?} is not a constant"),
    };
    cx.bind(result, value, HeldCarrier::of(&ty));
}

#[cfg(test)]
mod tests {
    //! Constants on the interpreting builder and as Cranelift machine code.
    //! The oracle is each literal's value in its raw carrier, and for a heap
    //! literal the runtime model's object for exactly the literal's bytes.

    use std::cell::RefCell;

    use super::super::differential::{Run, both, kernel};
    use super::super::scalar::tests::{NONE_WORD, Value, literal_word, program_slot_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::values::ValueId;

    /// Lower one constant op binding %0; the fact is the carrier it bound.
    fn constant(opcode: OpCode, attrs: &[(&str, AttrValue)]) -> Run<Option<HeldCarrier>> {
        let mut dict = AttrDict::default();
        for (key, value) in attrs {
            dict.insert((*key).into(), value.clone());
        }
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![ValueId(0)],
            attrs: dict,
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        };
        let lower_one = kernel!(op: TirOp = op; |b, inputs| -> Option<HeldCarrier> {
            let func = TirFunction::new(
                "f".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(1);
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &super::super::differential::every_runtime_entry(),
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &std::cell::Cell::new(0),
            };
            lower(&mut cx, b, op);
            assert!(diagnostics.borrow().is_empty());
            (values.repr(ValueId(0)), vec![values.value(ValueId(0)).unwrap()])
        });
        both(&lower_one, &[])
    }

    #[test]
    fn a_scalar_literal_is_a_machine_constant_in_its_raw_carrier() {
        for value in [0, -5, i64::MIN, i64::MAX] {
            let run = constant(OpCode::ConstInt, &[("value", AttrValue::Int(value))]);
            assert_eq!(run.fact, Some(HeldCarrier::RawI64));
            assert_eq!(run.results, [Value::Word(value as u64)]);
        }
        for value in [1.5, -0.0, f64::INFINITY, f64::NAN] {
            for key in ["f_value", "value"] {
                let run = constant(OpCode::ConstFloat, &[(key, AttrValue::Float(value))]);
                assert_eq!(run.fact, Some(HeldCarrier::RawF64));
                let [Value::Float(bound)] = run.results[..] else {
                    panic!("{:?} is not a float", run.results);
                };
                assert_eq!(bound.to_bits(), value.to_bits());
            }
        }
        for (attr, expected) in [
            (AttrValue::Bool(true), true),
            (AttrValue::Bool(false), false),
            (AttrValue::Int(2), true),
            (AttrValue::Int(0), false),
        ] {
            let run = constant(OpCode::ConstBool, &[("value", attr)]);
            assert_eq!(run.fact, Some(HeldCarrier::RawBool));
            assert_eq!(run.results, [Value::Bit(expected)]);
        }
        let run = constant(OpCode::ConstNone, &[]);
        assert_eq!(run.fact, Some(HeldCarrier::Boxed));
        assert_eq!(run.results, [Value::Word(NONE_WORD)]);
        assert!(run.runtime_calls.is_empty());
    }

    /// Lower `ops` (each binding its own result) on the machine; the results
    /// are their words in order. A literal's cell is machine memory, which the
    /// interpreting builder does not model, so these run on the machine alone.
    #[cfg(feature = "native-backend")]
    fn literals(ops: Vec<TirOp>) -> Run<Vec<Option<HeldCarrier>>> {
        let count = ops.len();
        let lower_all = kernel!(ops: Vec<TirOp> = ops; |b, inputs| -> Vec<Option<HeldCarrier>> {
            use molt_ir::function_table::FunctionRowKind;

            let functions = molt_ir::FunctionTable::from_rows([
                ("f", FunctionRowKind::Body),
                ("elsewhere", FunctionRowKind::Extern),
            ]);
            let func = TirFunction::new(
                "f".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(ops.len() as u32);
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &super::super::differential::every_runtime_entry(),
                linkage: &Default::default(),
                functions: &functions,
                call_sites: &std::cell::Cell::new(0),
            };
            for op in ops {
                lower(&mut cx, b, op);
            }
            assert!(diagnostics.borrow().is_empty());
            let ids = (0..ops.len() as u32).map(ValueId);
            (
                ids.clone().map(|id| values.repr(id)).collect(),
                ids.map(|id| values.value(id).unwrap()).collect(),
            )
        });
        let run = crate::native_backend::codegen_builder::machine::run(&lower_all, &[], false);
        assert_eq!(run.results.len(), count);
        run
    }

    #[cfg(feature = "native-backend")]
    fn literal_op(opcode: OpCode, payload: &[u8], result: u32) -> TirOp {
        let mut attrs = AttrDict::default();
        attrs.insert("bytes".into(), AttrValue::Bytes(payload.to_vec()));
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        }
    }

    /// A heap literal's first evaluation fills its cell through one runtime
    /// call; a later evaluation of the same literal loads the cell and calls
    /// nothing. Each literal is the object of exactly its kind and bytes.
    #[cfg(feature = "native-backend")]
    #[test]
    fn a_heap_literal_fills_its_cell_once_then_loads_it() {
        for (opcode, constructor, payload) in [
            (
                OpCode::ConstStr,
                RuntimeFn::StringFromBytes,
                b"h\xc3\xa9llo".to_vec(),
            ),
            (OpCode::ConstStr, RuntimeFn::StringFromBytes, Vec::new()),
            (
                OpCode::ConstBytes,
                RuntimeFn::BytesFromBytes,
                vec![0xff, 0, 0x80],
            ),
            (
                OpCode::ConstBigInt,
                RuntimeFn::BigintFromBytes,
                vec![0x01, 0x8e, 0xe9, 0x0f, 0xf6, 0xc3, 0x73, 0xe0, 0xee],
            ),
        ] {
            let run = literals(vec![
                literal_op(opcode, &payload, 0),
                literal_op(opcode, &payload, 1),
            ]);
            assert_eq!(run.fact, [Some(HeldCarrier::Boxed); 2], "{opcode:?}");
            let word = Value::Word(literal_word(constructor, &payload));
            assert_eq!(run.results, [word, word], "{opcode:?} {payload:?}");
            assert_eq!(
                run.runtime_calls,
                [RuntimeFn::LiteralCellFill],
                "{opcode:?}"
            );
        }
    }

    /// Distinct literals, and one payload under two kinds, own distinct
    /// cells: each fills its own and answers its own object.
    #[cfg(feature = "native-backend")]
    #[test]
    fn distinct_literals_own_distinct_cells() {
        let run = literals(vec![
            literal_op(OpCode::ConstStr, b"one", 0),
            literal_op(OpCode::ConstStr, b"two", 1),
            literal_op(OpCode::ConstBytes, b"one", 2),
        ]);
        assert_eq!(
            run.results,
            [
                Value::Word(literal_word(RuntimeFn::StringFromBytes, b"one")),
                Value::Word(literal_word(RuntimeFn::StringFromBytes, b"two")),
                Value::Word(literal_word(RuntimeFn::BytesFromBytes, b"one")),
            ]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::LiteralCellFill; 3]);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn probing_a_literal_cell_only_sees_an_actual_fill_of_its_kind_and_payload() {
        let probe = |bytes: &[u8], result, provider| {
            let mut op = literal_op(OpCode::StringLiteralCellReady, bytes, result);
            op.callee = Some(molt_ir::FunctionId::from_index(provider));
            op
        };
        let run = literals(vec![
            probe(b"context", 0, 0),
            literal_op(OpCode::ConstStr, b"context", 1),
            probe(b"context", 2, 0),
            literal_op(OpCode::ConstBytes, b"other", 3),
            probe(b"other", 4, 0),
            probe(b"context", 5, 1),
        ]);
        assert_eq!(run.results[0], Value::Bit(false));
        assert_eq!(
            &run.results[2..],
            &[
                Value::Bit(true),
                Value::Word(literal_word(RuntimeFn::BytesFromBytes, b"other")),
                Value::Bit(false),
                Value::Bit(false),
            ]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::LiteralCellFill; 2]);
    }

    #[test]
    fn a_module_slot_reads_the_word_its_owner_binds() {
        let run = constant(
            OpCode::ModuleSlot,
            &[("s_value", AttrValue::Str("pkg.mod".into()))],
        );
        assert_eq!(run.fact, Some(HeldCarrier::RawI64));
        let symbol = molt_ir::code_slots::program_slot_symbol(ProgramSlotKind::Module, "pkg.mod");
        assert_eq!(run.results, [Value::Word(program_slot_word(&symbol))]);
        assert!(run.runtime_calls.is_empty());
    }
}
