//! Constants (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "constants"): literal values and program-slot reads, written once.
//!
//! A scalar literal is a machine constant in its raw carrier. A string,
//! bytes or big-integer literal is a private byte constant the runtime
//! materializes into one owned object per evaluation, even for equal
//! payloads.

use molt_codegen_abi as nanbox;
use molt_ir::code_slots::ProgramSlotKind;
use molt_ir::literal_payload::required_tir_literal_bytes;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::CodegenBuilder;
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;

/// A string or bytes literal's runtime constructor and debug names.
struct ByteLiteral {
    constructor: RuntimeFn,
    global_prefix: &'static str,
    out_name: &'static str,
    call_name: &'static str,
    load_name: &'static str,
}

impl ByteLiteral {
    fn of(literal_type: &TirType) -> Self {
        match literal_type {
            TirType::Str => Self {
                constructor: RuntimeFn::StringFromBytes,
                global_prefix: "__const_str_",
                out_name: "str_out",
                call_name: "sfb",
                load_name: "str_bits",
            },
            TirType::Bytes => Self {
                constructor: RuntimeFn::BytesFromBytes,
                global_prefix: "__const_bytes_",
                out_name: "bytes_out",
                call_name: "bfb",
                load_name: "bytes_bits",
            },
            other => panic!("byte literal requires Str or Bytes, got {other:?}"),
        }
    }
}

/// One owned str or bytes object holding `bytes`, and the constructor's
/// status (zero on success; on failure the word is not an owner and an
/// exception is pending). The constructor writes the object through an
/// entry slot, so a literal in a loop never grows the stack.
pub(crate) fn byte_literal<B: CodegenBuilder>(
    b: &mut B,
    bytes: &[u8],
    literal_type: &TirType,
) -> (B::Word, B::Value) {
    let literal = ByteLiteral::of(literal_type);
    let data = b.bytes_constant(bytes, literal.global_prefix, "");
    let constructor = b.runtime_callee(literal.constructor);
    let out = b.entry_slot(literal.out_name);
    let len = b.constant(bytes.len() as u64);
    let len = b.word_value(len);
    let out_pointer = b.slot_pointer(out);
    let status = b
        .call(constructor, &[data, len, out_pointer])
        .expect("a byte-literal constructor returns its status");
    b.set_name(status, literal.call_name);
    let word = b.load_slot(out);
    let named = b.word_value(word);
    b.set_name(named, literal.load_name);
    (word, status)
}

/// Lower a constant op: `ConstInt`, `ConstFloat`, `ConstBool`, `ConstNone`,
/// `ConstStr`, `ConstBytes`, `ConstBigInt` or `ModuleSlot`.
pub(crate) fn lower<B: CodegenBuilder>(cx: &mut FunctionCx<'_, B>, b: &mut B, op: &TirOp) {
    let result = op.results[0];
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
        OpCode::ConstStr | OpCode::ConstBytes => {
            let ty = if op.opcode == OpCode::ConstStr {
                TirType::Str
            } else {
                TirType::Bytes
            };
            let (word, _) = byte_literal(b, required_tir_literal_bytes(op), &ty);
            (b.word_value(word), ty)
        }
        OpCode::ConstBigInt => {
            let digits = required_tir_literal_bytes(op);
            let data = b.bytes_constant(digits, "__const_bigint_", "");
            let constructor = b.runtime_callee(RuntimeFn::BigintFromBytes);
            let len = b.constant(digits.len() as u64);
            let len = b.word_value(len);
            let value = b
                .call(constructor, &[data, len])
                .expect("molt_bigint_from_bytes returns a word");
            b.set_name(value, "bigint_bits");
            (value, TirType::DynBox)
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

    #[test]
    fn a_heap_literal_is_one_new_object_of_exactly_its_bytes() {
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
            let run = constant(opcode, &[("bytes", AttrValue::Bytes(payload.clone()))]);
            assert_eq!(run.fact, Some(HeldCarrier::Boxed), "{opcode:?}");
            assert_eq!(
                run.results,
                [Value::Word(literal_word(constructor, &payload))],
                "{opcode:?} {payload:?}"
            );
            assert_eq!(run.runtime_calls, [constructor]);
        }
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

    #[test]
    fn a_byte_literal_reports_the_constructors_status() {
        let literal = kernel!(; |b, inputs| -> () {
            let (word, status) = byte_literal(b, b"name", &TirType::Str);
            let word = b.word_value(word);
            ((), vec![word, status])
        });
        let run = both(&literal, &[]);
        assert_eq!(
            run.results,
            [
                Value::Word(literal_word(RuntimeFn::StringFromBytes, b"name")),
                Value::Narrow(0),
            ]
        );
    }
}
