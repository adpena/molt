use super::peephole::peephole_set_get_to_tee;
use super::runtime_calls::LirRuntimeCall;
use super::{lower_lir_to_wasm, lower_tir_to_wasm, lower_tir_to_wasm_boxed_i64_abi};
use crate::repr::Repr;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::lower_to_lir::lower_function_to_lir_with_inline_proof;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use crate::wasm::body::{WasmBodyOps, WasmLirFallbackReason};
use molt_codegen_abi::{CANONICAL_NAN_BITS, INT_MASK, QNAN_TAG_INT_I64, QNAN_TAG_MASK_I64};
use std::collections::HashMap;
use wasm_encoder::{Instruction, ValType};

const F64_EXPONENT_MASK: i64 = 0x7ff0_0000_0000_0000u64 as i64;
const F64_FRACTION_MASK: i64 = 0x000f_ffff_ffff_ffffu64 as i64;

fn peephole_instrs(input: Vec<Instruction<'static>>) -> Vec<Instruction<'static>> {
    peephole_set_get_to_tee(WasmBodyOps::from_instructions(input)).into_instructions_for_tests()
}

#[test]
fn lir_runtime_calls_are_manifest_registered_imports() {
    let manifest_imports: std::collections::BTreeSet<&'static str> =
        crate::wasm_imports::IMPORT_REGISTRY
            .iter()
            .map(|&(name, _)| name)
            .collect();

    for call in LirRuntimeCall::ALL {
        let import_name = call.import_name();
        assert!(
            manifest_imports.contains(import_name),
            "LIR fast runtime call {call:?} must be registered in wasm_abi_manifest.toml"
        );
    }
}

/// Build a trivial function: returns a constant i64.
fn make_const_return_func(val: i64) -> TirFunction {
    let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(val));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_scalar_const_return_func(
    name: &str,
    opcode: OpCode,
    return_type: TirType,
    attrs: AttrDict,
) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], return_type);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![result_id],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

#[test]
fn binding_alias_copy_retains_before_forwarding_bits() {
    let mut func = TirFunction::new(
        "binding_alias_copy".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let alias = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![ValueId(0)],
        results: vec![alias],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str("binding_alias".into()),
            );
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![alias],
    };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"inc_ref_obj"),
        "binding_alias Copy must retain its forwarded source: {:?}",
        output.runtime_calls
    );
}

#[test]
fn trivial_const_return() {
    let func = make_const_return_func(42);
    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![]);
    assert_eq!(output.result_types, vec![ValType::I64]);

    // Should contain i64.const 42 somewhere.
    let has_const = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Const(42)));
    assert!(has_const, "expected i64.const 42 in output");

    // Should end with `end`.
    assert!(matches!(output.instructions.last(), Some(Instruction::End)));
}

#[test]
#[should_panic(expected = "WASM const policy const requires int scalar payload")]
fn lir_const_int_missing_payload_fails_closed() {
    let func = make_scalar_const_return_func(
        "bad_const_int",
        OpCode::ConstInt,
        TirType::I64,
        AttrDict::new(),
    );

    let _ = lower_tir_to_wasm(&func);
}

#[test]
#[should_panic(expected = "WASM const policy const_float requires float scalar payload")]
fn lir_const_float_missing_payload_fails_closed() {
    let func = make_scalar_const_return_func(
        "bad_const_float",
        OpCode::ConstFloat,
        TirType::F64,
        AttrDict::new(),
    );

    let _ = lower_tir_to_wasm(&func);
}

#[test]
#[should_panic(expected = "WASM const policy const_bool requires bool scalar payload")]
fn lir_const_bool_mismatched_payload_fails_closed() {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(1));
    let func =
        make_scalar_const_return_func("bad_const_bool", OpCode::ConstBool, TirType::Bool, attrs);

    let _ = lower_tir_to_wasm(&func);
}

#[test]
fn lir_literal_consts_materialize_without_generic_bail() {
    let cases = [
        {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("hello".into()));
            (
                "const_str_literal",
                OpCode::ConstStr,
                TirType::Str,
                attrs,
                "string_from_bytes",
            )
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "s_value".into(),
                AttrValue::Str("9223372036854775808".into()),
            );
            (
                "const_bigint_literal",
                OpCode::ConstBigInt,
                TirType::DynBox,
                attrs,
                "bigint_from_str",
            )
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert("bytes".into(), AttrValue::Bytes(vec![0, 1, 2, 255]));
            (
                "const_bytes_literal",
                OpCode::ConstBytes,
                TirType::Bytes,
                attrs,
                "bytes_from_bytes",
            )
        },
    ];

    for (name, opcode, return_type, attrs, import_name) in cases {
        let output = lower_tir_to_wasm(&make_scalar_const_return_func(
            name,
            opcode,
            return_type,
            attrs,
        ))
        .test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must materialize in the LIR fast body instead of bailing"
        );
        assert_eq!(output.bail_to_generic_reason, None);
        assert!(
            output.runtime_calls.contains(&import_name),
            "{name} must call {import_name}; got {:?}",
            output.runtime_calls
        );
        assert!(
            output.locals.len() >= 3,
            "{name} must declare result plus ptr/len scratch locals for materialization"
        );
    }
}

#[test]
#[should_panic(expected = "generated WASM const policy requires a result for ConstStr")]
fn lir_literal_const_without_result_fails_closed() {
    let mut func = TirFunction::new("bad_const_str".into(), vec![], TirType::None);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("orphan".into()));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let _ = lower_tir_to_wasm(&func);
}

#[test]
fn lir_fast_lane_dec_ref_emits_named_runtime_call() {
    let mut func = TirFunction::new("drop_ref".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![owned],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"dec_ref_obj"),
        "WASM LIR fast lane must consume shared DecRef through dec_ref_obj; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn lir_fast_lane_del_boundary_emits_named_dec_ref_runtime_call() {
    let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![owned],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelBoundary,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"dec_ref_obj"),
        "WASM LIR fast lane must consume DelBoundary through dec_ref_obj; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn add_two_i64s() {
    let func = make_add_two_consts_func(20, 22);

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, Vec::<ValType>::new());

    // Should contain i64.add.
    let has_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Add));
    assert!(has_add, "expected i64.add instruction");
}

#[test]
fn bool1_and_stays_raw_without_selected_ref_retain() {
    let mut func = TirFunction::new(
        "and_bool1".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::Bool,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::And,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I32And)),
        "raw Bool1 and must stay a native i32.and: {:?}",
        output.instructions
    );
    assert!(
        !output.runtime_calls.contains(&"inc_ref_obj"),
        "raw Bool1 and must not retain a selected boxed operand: {:?}",
        output.runtime_calls
    );
}

#[test]
fn dynbox_or_retains_selected_operand_result() {
    let mut func = TirFunction::new(
        "or_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Or,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "boxed or must test Python truthiness: {:?}",
        output.runtime_calls
    );
    assert!(
        output.runtime_calls.contains(&"inc_ref_obj"),
        "boxed or must retain the selected borrowed operand result: {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::LocalTee(_))),
        "boxed or must tee the selected result before retaining it: {:?}",
        output.instructions
    );
}

#[test]
fn dynbox_unary_scalar_helpers_stay_lir_fast_runtime_calls() {
    let cases = [
        ("neg_dynbox", OpCode::Neg, "neg"),
        ("pos_dynbox", OpCode::Pos, "pos"),
        ("invert_dynbox", OpCode::BitNot, "invert"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func = TirFunction::new(name.into(), vec![TirType::DynBox], TirType::DynBox);
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn raw_unary_pos_stays_noop_without_runtime_call() {
    let mut func = TirFunction::new("pos_raw_i64".into(), vec![TirType::I64], TirType::I64);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Pos,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let repr = HashMap::from([
        (ValueId(0), Repr::RawI64Safe),
        (result_id, Repr::RawI64Safe),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    let output = lower_lir_to_wasm(&lir).test_view();

    assert!(
        !output.bails_to_generic_path,
        "proven raw unary plus must stay in the LIR fast lane"
    );
    assert_eq!(output.param_types, vec![ValType::I64]);
    assert_eq!(output.result_types, vec![ValType::I64]);
    assert!(
        !output.runtime_calls.contains(&"pos"),
        "proven raw unary plus must remain a no-op, not call pos: {:?}",
        output.runtime_calls
    );
}

#[test]
fn dynbox_pow_stays_lir_fast_runtime_call() {
    let mut func = TirFunction::new(
        "pow_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Pow,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "DynBox pow must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"pow"),
        "DynBox pow must dispatch through the typed runtime helper; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn dynbox_binary_bitwise_and_shift_helpers_stay_lir_fast_runtime_calls() {
    let cases = [
        ("bit_and_dynbox", OpCode::BitAnd, "bit_and"),
        ("bit_or_dynbox", OpCode::BitOr, "bit_or"),
        ("bit_xor_dynbox", OpCode::BitXor, "bit_xor"),
        ("lshift_dynbox", OpCode::Shl, "lshift"),
        ("rshift_dynbox", OpCode::Shr, "rshift"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn add_two_f64s() {
    let mut func = TirFunction::new(
        "add_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
    let has_f64_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::F64Add));
    assert!(has_f64_add, "expected f64.add instruction");
}

#[test]
fn f64_mod_declares_emission_scratch_locals() {
    let mut func = TirFunction::new(
        "mod_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Mod,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
    assert_eq!(output.result_types, vec![ValType::F64]);
    assert_eq!(
        output.locals,
        vec![ValType::F64, ValType::F64, ValType::F64],
        "f64 modulo needs the result local plus two scratch locals declared"
    );
}

#[test]
fn conditional_branch() {
    let mut func = TirFunction::new("cond_branch".into(), vec![TirType::Bool], TirType::I64);

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();

    let ret_then = func.fresh_value();
    let ret_else = func.fresh_value();

    // Patch entry block to branch on param.
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    };

    let then_block = TirBlock {
        id: then_id,
        args: vec![],
        ops: vec![TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ret_then],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(1));
                m
            },
            source_span: None,
        }],
        terminator: Terminator::Return {
            values: vec![ret_then],
        },
    };

    let else_block = TirBlock {
        id: else_id,
        args: vec![],
        ops: vec![TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ret_else],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(0));
                m
            },
            source_span: None,
        }],
        terminator: Terminator::Return {
            values: vec![ret_else],
        },
    };

    func.blocks.insert(then_id, then_block);
    func.blocks.insert(else_id, else_block);

    let output = lower_tir_to_wasm(&func).test_view();

    // Should contain br_if for the conditional branch.
    let has_br_if = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::BrIf(_)));
    assert!(
        has_br_if,
        "expected br_if instruction for conditional branch"
    );
}

#[test]
fn dynbox_bool_uses_lir_truthiness_without_generic_bail() {
    let mut func = TirFunction::new("bool_dynbox".into(), vec![TirType::DynBox], TirType::Bool);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Bool,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "boxed bool() must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "boxed bool() must dispatch non-bool objects through is_truthy; got {:?}",
        output.runtime_calls
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == QNAN_TAG_MASK_I64)
        ),
        "boxed truthiness must retain the inline boxed-bool path"
    );
}

#[test]
fn dynbox_conditional_branch_uses_lir_truthiness_without_generic_bail() {
    let mut func = TirFunction::new(
        "cond_branch_dynbox".into(),
        vec![TirType::DynBox],
        TirType::I64,
    );

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();
    let ret_then = func.fresh_value();
    let ret_else = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    };

    func.blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_then],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_then],
            },
        },
    );
    func.blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_else],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_else],
            },
        },
    );

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "boxed conditional branch must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "boxed conditional branch must dispatch non-bool objects through is_truthy; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::BrIf(_))),
        "conditional branch must still emit br_if"
    );
}

#[test]
fn comparison_i64_emits_native() {
    let func = make_lt_two_consts_func(20, 22);

    let output = lower_tir_to_wasm(&func).test_view();

    let has_lt = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64LtS));
    assert!(has_lt, "expected i64.lt_s instruction");
}

#[test]
fn dynbox_add_falls_back_to_call() {
    let mut func = TirFunction::new(
        "add_dyn".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.runtime_calls.contains(&"add"),
        "expected typed runtime import for DynBox add"
    );

    let has_i64_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Add));
    assert!(!has_i64_add, "should NOT emit i64.add for DynBox operands");
}

#[test]
fn mixed_f64_dynbox_add_boxes_float_without_generic_bail() {
    let mut func = TirFunction::new(
        "add_float_dyn".into(),
        vec![TirType::F64, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "F64 operand boxing must not poison typed LIR-fast runtime dispatch with a generic bail"
    );
    assert!(
        output.runtime_calls.contains(&"add"),
        "mixed F64/DynBox add must dispatch through the typed boxed runtime helper"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64ReinterpretF64)),
        "F64 operand must be boxed by reinterpreting its IEEE payload"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == F64_EXPONENT_MASK)
        ),
        "F64 boxing must use the shared all-NaN exponent mask"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == F64_FRACTION_MASK)
        ),
        "F64 boxing must use the shared all-NaN fraction mask"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(bits) if *bits == CANONICAL_NAN_BITS as i64)
        ),
        "F64 boxing must canonicalize NaN payloads to the shared canonical bits"
    );
}

#[test]
fn dynbox_identity_comparisons_stay_lir_fast_runtime_calls() {
    let cases = [
        ("is_dynbox", OpCode::Is, false),
        ("is_not_dynbox", OpCode::IsNot, true),
    ];

    for (name, opcode, expect_invert) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&"is"),
            "{name} must dispatch through the identity helper; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&"not"),
            "{name} should project/invert the boxed bool locally for Bool1 results"
        );
        assert_eq!(
            output
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::I32Eqz)),
            expect_invert,
            "{name} local Bool1 projection invert mismatch: {:?}",
            output.instructions
        );
    }
}

#[test]
fn alloc_task_bails_to_generic_emission() {
    let mut func = TirFunction::new("alloc_task".into(), vec![TirType::DynBox], TirType::DynBox);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::AllocTask,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("s_value".into(), AttrValue::Str("task_poll".into()));
            m.insert("task_kind".into(), AttrValue::Str("future".into()));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "alloc_task must bail to generic WASM emission"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}

#[test]
fn state_switch_bails_to_generic_emission() {
    let mut func = TirFunction::new(
        "state_switch".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StateSwitch,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "state_switch must bail to generic WASM emission"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}

// -----------------------------------------------------------------------
// Peephole pass tests
// -----------------------------------------------------------------------

#[test]
fn peephole_collapses_set_get_to_tee() {
    let input = vec![
        Instruction::I64Const(42),
        Instruction::LocalSet(3),
        Instruction::LocalGet(3),
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 3);
    assert!(
        matches!(output[0], Instruction::I64Const(42)),
        "const preserved"
    );
    assert!(
        matches!(output[1], Instruction::LocalTee(3)),
        "set+get collapsed to tee"
    );
    assert!(matches!(output[2], Instruction::End), "end preserved");
}

#[test]
fn peephole_preserves_mismatched_set_get() {
    let input = vec![
        Instruction::LocalSet(1),
        Instruction::LocalGet(2), // different local
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 3);
    assert!(
        matches!(output[0], Instruction::LocalSet(1)),
        "set preserved"
    );
    assert!(
        matches!(output[1], Instruction::LocalGet(2)),
        "get preserved"
    );
}

#[test]
fn peephole_handles_consecutive_tee_chains() {
    // Pattern: set(1) get(1) set(2) get(2) → tee(1) tee(2)
    let input = vec![
        Instruction::I64Const(10),
        Instruction::LocalSet(1),
        Instruction::LocalGet(1),
        Instruction::LocalSet(2),
        Instruction::LocalGet(2),
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 4);
    assert!(matches!(output[1], Instruction::LocalTee(1)));
    assert!(matches!(output[2], Instruction::LocalTee(2)));
}

#[test]
fn peephole_empty_and_single() {
    assert!(peephole_instrs(vec![]).is_empty());
    let single = vec![Instruction::End];
    assert_eq!(peephole_instrs(single).len(), 1);
}

#[test]
fn peephole_applied_in_const_return() {
    // A const-return function should have tee instead of set+get.
    let func = make_const_return_func(99);
    let output = lower_tir_to_wasm(&func).test_view();

    // After peephole, the pattern: i64.const 99; local.set X; local.get X; return
    // becomes: i64.const 99; local.tee X; return
    let has_tee = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::LocalTee(_)));
    assert!(has_tee, "expected local.tee from peephole optimization");

    // Should have no set+get pairs for the same local.
    for window in output.instructions.windows(2) {
        if let (Instruction::LocalSet(s), Instruction::LocalGet(g)) = (&window[0], &window[1]) {
            assert_ne!(
                s, g,
                "found redundant set+get pair for local {s} that peephole should have eliminated"
            );
        }
    }
}

// -----------------------------------------------------------------------
// Phase 1: mixed-repr integer arithmetic (the delicate correctness core)
// -----------------------------------------------------------------------

/// Build `f(a: int, b: int) -> int = a + b` with two i64-typed params and a
/// single Add. The caller supplies the `Repr` override.
fn make_add_two_params_func() -> TirFunction {
    let mut func = TirFunction::new(
        "add_two_params".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );
    let result_id = func.fresh_value(); // ValueId(2)
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_add_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new("add_two_consts".into(), vec![], TirType::I64);
    let lhs_id = func.fresh_value();
    let rhs_id = func.fresh_value();
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs_id, lhs), (rhs_id, rhs)] {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![id],
            attrs,
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![lhs_id, rhs_id],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_lt_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new("lt_two_consts".into(), vec![], TirType::Bool);
    let lhs_id = func.fresh_value();
    let rhs_id = func.fresh_value();
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs_id, lhs), (rhs_id, rhs)] {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![id],
            attrs,
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Lt,
        operands: vec![lhs_id, rhs_id],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

#[test]
fn generic_tir_to_wasm_uses_value_repr_not_type_floor_for_int_params() {
    let func = make_add_two_params_func();
    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.runtime_calls.contains(&"add"),
        "unproven int params must lower through boxed runtime dispatch, not a type-floor raw i64 op"
    );
    for (idx, inst) in output.instructions.iter().enumerate() {
        if matches!(inst, Instruction::I64Add) {
            assert!(
                matches!(
                    output.instructions.get(idx + 1),
                    Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                ),
                "generic lower_tir_to_wasm emitted a bare operand i64.add at {idx}"
            );
        }
    }
}

/// Full-range raw carriers must box through the OVERFLOW-SAFE path: a
/// full-range raw value without an inline-window range proof (the CheckedAdd
/// sum / overflow_peel accumulator case) boxed at a runtime-call or
/// return site must emit the fits-check + named `int_from_i64` cold
/// call, never the bare 47-bit mask (which truncates mod 2^47).
#[test]
fn full_range_raw_carrier_boxes_overflow_safe_with_named_call() {
    let func = make_add_two_params_func();
    // The values are raw full-deopt carriers, and the value-range proof for
    // opaque params does not prove the 47-bit inline window. The checked
    // triple is therefore refused and the add takes the boxed runtime path,
    // boxing both raw operands through the overflow-safe cold call.
    let repr: HashMap<ValueId, Repr> = HashMap::from([
        (ValueId(0), Repr::RawI64FullDeopt),
        (ValueId(1), Repr::RawI64FullDeopt),
        (ValueId(2), Repr::RawI64FullDeopt),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = crate::tir::lower_to_lir::lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    // Triple refused without an inline-window proof: no op carries
    // lir.checked_overflow.
    let has_triple = lir.blocks.values().flat_map(|b| b.ops.iter()).any(|op| {
        matches!(
            op.tir_op.attrs.get("lir.checked_overflow"),
            Some(crate::tir::ops::AttrValue::Bool(true))
        )
    });
    assert!(
        !has_triple,
        "checked-i64 triple must be refused without a value-range proof"
    );

    let output = lower_lir_to_wasm(&lir).test_view();
    // The raw operands are boxed overflow-safely: the cold arm is a
    // NAMED int_from_i64 runtime call recorded in runtime_calls.
    assert!(
        output
            .runtime_calls
            .iter()
            .filter(|name| **name == "int_from_i64")
            .count()
            >= 2,
        "both full-range raw operands must box through the int_from_i64 cold path; got {:?}",
        output.runtime_calls
    );
}

/// Count occurrences of the inline-int NaN-box packing
/// (`emit_box_inline_i64`): `i64.const INT_MASK; i64.and; i64.const
/// (QNAN|TAG_INT); i64.or`. This is how a proven raw-i64 operand is boxed
/// before a runtime helper call in the mixed-repr boxed arm.
fn count_inline_int_boxes(instrs: &[Instruction<'static>]) -> usize {
    instrs
        .windows(4)
        .filter(|w| {
            matches!(w[0], Instruction::I64Const(m) if m == INT_MASK as i64)
                && matches!(w[1], Instruction::I64And)
                && matches!(w[2], Instruction::I64Const(t) if t == QNAN_TAG_INT_I64)
                && matches!(w[3], Instruction::I64Or)
        })
        .count()
}

/// THE regression guard for finding #3: an integer `add` with one proven
/// `RawI64Safe` operand and one `MaybeBigInt` operand must NOT emit a bare
/// `i64.add` (the unsound op on a NaN-boxed word). Both operands must be
/// NaN-boxed before the runtime `Call` (`molt_add`): the proven operand via
/// the inline-int box, the unproven operand passed through already-boxed.
#[test]
fn mixed_repr_int_add_boxes_both_operands_no_bare_i64_add() {
    let func = make_add_two_params_func();
    // a (ValueId 0) is proven RawI64Safe; b (ValueId 1) is an unproven
    // MaybeBigInt; the result (ValueId 2) is therefore MaybeBigInt too (it
    // cannot be proven from an unproven operand). This forces the generic
    // boxed path (NOT the checked-overflow triple, which requires all three
    // to be RawI64Safe).
    let repr: HashMap<ValueId, Repr> = HashMap::from([
        (ValueId(0), Repr::RawI64Safe),
        (ValueId(1), Repr::MaybeBigInt),
        (ValueId(2), Repr::MaybeBigInt),
    ]);
    let lir = lower_function_to_lir_with_inline_proof(
        &func,
        &repr,
        &crate::representation_plan::value_range_for(&func),
    );
    let output = lower_lir_to_wasm(&lir).test_view();

    // No bare OPERAND i64.add: a raw machine add on a possibly-heap-BigInt
    // operand is exactly the truncation bug-class this phase makes
    // un-emittable. The overflow-safe box legitimately contains an
    // `i64.add` (the `src + 2^46` fits-inline bias), so the precise
    // invariant is: every I64Add in the stream is a fits-check add —
    // immediately followed by the `2^47` window-limit const — never an
    // operand-pair add.
    for (idx, inst) in output.instructions.iter().enumerate() {
        if matches!(inst, Instruction::I64Add) {
            assert!(
                matches!(
                    output.instructions.get(idx + 1),
                    Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                ),
                "mixed-repr add emitted a bare operand i64.add at {idx} (operand may be a heap BigInt)"
            );
        }
    }
    // Runtime dispatch through the typed boxed helper import.
    assert!(
        output.runtime_calls.contains(&"add"),
        "mixed-repr add must dispatch through the boxed runtime helper"
    );
    // The proven RawI64Safe operand `a` is NaN-boxed (inline-int box) before
    // the call. (`b` is already a DynBox word and passes through, so exactly
    // one inline-int box is emitted for the operands of this add.)
    assert!(
        count_inline_int_boxes(&output.instructions) >= 1,
        "the proven raw-i64 operand must be NaN-boxed before the runtime call"
    );
}

/// The perf-preservation direction: when BOTH operands are proven
/// `RawI64Safe`, the fast `i64.add` is still emitted (the checked-overflow
/// triple), and no boxed runtime `Call` is needed for the add itself.
#[test]
fn proven_raw_i64_add_still_emits_native_i64_add() {
    let func = make_add_two_consts_func(20, 22);
    let output = lower_tir_to_wasm(&func).test_view();

    let has_operand_add = output.instructions.iter().enumerate().any(|(idx, inst)| {
        matches!(inst, Instruction::I64Add)
            && !matches!(
                output.instructions.get(idx + 1),
                Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
            )
    });
    assert!(
        has_operand_add,
        "range-proven const add must emit an operand-pair native i64.add, got {:?}",
        output.instructions
    );
}

/// On the production boxed-i64 ABI path, a function whose integer params are
/// proven `RawI64Safe` keeps the fast path (entry args lower to `I64`); a
/// `MaybeBigInt` param forces the entry arg to `DynBox`, so the boxed-i64 ABI
/// (which requires all-`I64` entry args) bails to `None` — falling back to
/// the IntFastLane-guarded slow path. This is the structural gate that keeps
/// the unsound bare op un-emittable for unproven ints.
#[test]
fn boxed_i64_abi_bails_when_param_is_maybe_bigint() {
    let proven = make_add_two_consts_func(20, 22);
    assert!(
        lower_tir_to_wasm_boxed_i64_abi(&proven).is_some(),
        "range-proven raw-i64 values keep the boxed-i64 ABI fast path"
    );

    let unproven = make_add_two_params_func();
    assert!(
        lower_tir_to_wasm_boxed_i64_abi(&unproven).is_none(),
        "a MaybeBigInt param must bail the boxed-i64 ABI (entry arg is DynBox)"
    );
}
