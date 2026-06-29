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
use molt_codegen_abi::{
    CANONICAL_NAN_BITS, INT_MASK, QNAN_TAG_INT_I64, QNAN_TAG_MASK_I64, box_int_bits, box_none_bits,
    stable_ic_site_id,
};
use std::collections::HashMap;
use wasm_encoder::{Instruction, ValType};

const F64_EXPONENT_MASK: i64 = 0x7ff0_0000_0000_0000u64 as i64;
const F64_FRACTION_MASK: i64 = 0x000f_ffff_ffff_ffffu64 as i64;

fn peephole_instrs(input: Vec<Instruction<'static>>) -> Vec<Instruction<'static>> {
    peephole_set_get_to_tee(WasmBodyOps::from_instructions(input)).into_instructions_for_tests()
}

#[test]
fn lir_runtime_calls_are_manifest_registered_imports() {
    let manifest_imports: std::collections::BTreeSet<_> = crate::wasm_imports::IMPORT_REGISTRY
        .iter()
        .map(|spec| spec.import)
        .collect();

    for call in LirRuntimeCall::ALL {
        let import = call.import();
        assert!(
            manifest_imports.contains(&import),
            "LIR fast runtime call {call:?} must register {} in wasm_abi_manifest.toml",
            import.name()
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
fn dynbox_index_store_and_delete_stay_lir_fast_runtime_calls() {
    let mut index_func = TirFunction::new(
        "index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let index_result = index_func.fresh_value();
    index_func.value_types.insert(index_result, TirType::DynBox);
    let entry = index_func.blocks.get_mut(&index_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![index_result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![index_result],
    };

    let output = lower_tir_to_wasm(&index_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"index"),
        "DynBox index must dispatch through the boxed index helper; got {:?}",
        output.runtime_calls
    );

    let mut store_func = TirFunction::new(
        "store_index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let entry = store_func.blocks.get_mut(&store_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreIndex,
        operands: vec![ValueId(0), ValueId(1), ValueId(2)],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };

    let output = lower_tir_to_wasm(&store_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox store_index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"store_index"),
        "DynBox store_index must dispatch through the boxed store helper; got {:?}",
        output.runtime_calls
    );

    let mut del_func = TirFunction::new(
        "del_index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let entry = del_func.blocks.get_mut(&del_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelIndex,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };

    let output = lower_tir_to_wasm(&del_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox del_index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"del_index"),
        "DynBox del_index must dispatch through the boxed delete helper; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn typed_dict_and_tuple_index_select_specialized_runtime_calls() {
    let cases = [
        (
            "dict_index",
            TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
            "dict_getitem",
        ),
        (
            "tuple_index",
            TirType::Tuple(vec![TirType::DynBox]),
            "tuple_getitem",
        ),
    ];

    for (name, container_type, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Index,
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
            "{name} must use {runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&"index"),
            "{name} must not fall back to the generic index helper"
        );
    }
}

#[test]
fn typed_index_store_without_manifest_selector_rows_use_generic_runtime_calls() {
    let index_cases = [
        (
            "list_index_generic",
            TirType::List(Box::new(TirType::DynBox)),
        ),
        ("set_index_generic", TirType::Set(Box::new(TirType::DynBox))),
        ("str_index_generic", TirType::Str),
    ];

    for (name, container_type) in index_cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Index,
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
            output.runtime_calls.contains(&"index"),
            "{name} must use generic index; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output
                .runtime_calls
                .iter()
                .any(|call| matches!(*call, "dict_getitem" | "tuple_getitem" | "list_int_getitem")),
            "{name} must not use an unsupported specialized index helper"
        );
    }

    let store_cases = [
        (
            "list_store_generic",
            TirType::List(Box::new(TirType::DynBox)),
        ),
        ("set_store_generic", TirType::Set(Box::new(TirType::DynBox))),
        ("tuple_store_generic", TirType::Tuple(vec![TirType::DynBox])),
        ("str_store_generic", TirType::Str),
    ];

    for (name, container_type) in store_cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::StoreIndex,
            operands: vec![ValueId(0), ValueId(1), ValueId(2)],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&"store_index"),
            "{name} must use generic store_index; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output
                .runtime_calls
                .iter()
                .any(|call| matches!(*call, "dict_setitem" | "list_int_setitem")),
            "{name} must not use an unsupported specialized store helper"
        );
    }
}

#[test]
fn dynbox_iterator_helpers_stay_lir_fast_runtime_calls() {
    let cases = [
        ("get_iter_dynbox", OpCode::GetIter, "iter"),
        ("iter_next_dynbox", OpCode::IterNext, "iter_next"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func = TirFunction::new(name.into(), vec![TirType::DynBox], TirType::DynBox);
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
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
fn membership_uses_contains_runtime_call_and_not_in_inverts_bool() {
    let cases = [
        ("in_dynbox", OpCode::In, false),
        ("not_in_dynbox", OpCode::NotIn, true),
    ];

    for (name, opcode, expect_inversion) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::Bool);
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
            output.runtime_calls.contains(&"contains"),
            "{name} must call contains(container, item); got {:?}",
            output.runtime_calls
        );
        assert_eq!(
            output
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::I32Eqz)),
            expect_inversion,
            "{name} must invert only for NotIn"
        );
    }
}

#[test]
fn typed_membership_selects_specialized_contains_runtime_calls() {
    let cases = [
        (
            "dict_contains",
            TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
            "dict_contains",
        ),
        (
            "list_contains",
            TirType::List(Box::new(TirType::DynBox)),
            "list_contains",
        ),
        (
            "set_contains",
            TirType::Set(Box::new(TirType::DynBox)),
            "set_contains",
        ),
        ("str_contains", TirType::Str, "str_contains"),
    ];

    for (name, container_type, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::Bool);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::In,
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
        assert!(
            !output.runtime_calls.contains(&"contains"),
            "{name} must not fall back to the generic contains helper"
        );
    }
}

#[test]
fn build_slice_stays_lir_fast_and_pads_missing_bounds_with_none() {
    let mut func = TirFunction::new(
        "build_slice_missing_step".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSlice,
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
        "BuildSlice must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"slice_new"),
        "BuildSlice must call the slice_new ABI helper; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Const(bits) if *bits == box_none_bits())),
        "BuildSlice must pad a missing step operand with the ABI None bits"
    );
}

#[test]
fn ord_at_stays_lir_fast_with_boxed_result_carrier() {
    let mut func = TirFunction::new(
        "ord_at_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::OrdAt,
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
        "OrdAt must stay in the LIR fast lane after boxed carrier authority is fixed"
    );
    assert!(
        output.runtime_calls.contains(&"ord_at"),
        "OrdAt must call the ord_at ABI helper; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn raw_index_result_refuses_boxed_runtime_bits() {
    let mut func = TirFunction::new(
        "raw_index_result".into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::I64,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::I64);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let repr = HashMap::from([
        (ValueId(0), Repr::DynBox),
        (ValueId(1), Repr::RawI64Safe),
        (result_id, Repr::RawI64Safe),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    let output = lower_lir_to_wasm(&lir).test_view();

    assert!(
        output.bails_to_generic_path,
        "raw index result must not store boxed runtime bits into an I64 carrier"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}

fn make_fixed_runtime_service_func(
    name: &str,
    opcode: OpCode,
    operand_count: usize,
    has_result: bool,
) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox; operand_count],
        if has_result {
            TirType::DynBox
        } else {
            TirType::None
        },
    );
    let result_id = has_result.then(|| {
        let id = func.fresh_value();
        func.value_types.insert(id, TirType::DynBox);
        id
    });
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
        results: result_id.into_iter().collect(),
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: result_id.into_iter().collect(),
    };
    func
}

fn make_copy_original_kind_runtime_func(
    name: &str,
    original_kind: &str,
    operand_count: usize,
    has_result: bool,
) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox; operand_count],
        if has_result {
            TirType::DynBox
        } else {
            TirType::None
        },
    );
    let result_id = has_result.then(|| {
        let id = func.fresh_value();
        func.value_types.insert(id, TirType::DynBox);
        id
    });
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
        results: result_id.into_iter().collect(),
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str(original_kind.into()),
            );
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: result_id.into_iter().collect(),
    };
    func
}

#[test]
fn exception_pending_stays_lir_fast_with_bool_result_adapter() {
    let mut func = TirFunction::new("exception_pending".into(), vec![], TirType::Bool);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::Bool);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ExceptionPending,
        operands: vec![],
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
        "exception_pending must stay in the LIR fast lane"
    );
    assert_eq!(output.result_types, vec![ValType::I32]);
    assert!(
        output.runtime_calls.contains(&"exception_pending"),
        "exception_pending must call the typed runtime import; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Ne)),
        "exception_pending raw i64 flag must be adapted to a Bool1 result"
    );
}

#[test]
fn fixed_runtime_service_and_module_ops_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "function_defaults_version",
            OpCode::FunctionDefaultsVersion,
            1,
            true,
            "function_defaults_version",
        ),
        ("module_import", OpCode::Import, 1, true, "module_import"),
        (
            "module_cache_get",
            OpCode::ModuleCacheGet,
            1,
            true,
            "module_cache_get",
        ),
        (
            "module_cache_set",
            OpCode::ModuleCacheSet,
            2,
            false,
            "module_cache_set",
        ),
        (
            "module_cache_del",
            OpCode::ModuleCacheDel,
            1,
            false,
            "module_cache_del",
        ),
        (
            "module_get_attr",
            OpCode::ModuleGetAttr,
            2,
            true,
            "module_get_attr",
        ),
        (
            "module_import_from",
            OpCode::ModuleImportFrom,
            2,
            true,
            "module_import_from",
        ),
        (
            "module_get_global",
            OpCode::ModuleGetGlobal,
            2,
            true,
            "module_get_global",
        ),
        (
            "module_get_name",
            OpCode::ModuleGetName,
            2,
            true,
            "module_get_name",
        ),
        (
            "module_set_attr",
            OpCode::ModuleSetAttr,
            3,
            false,
            "module_set_attr",
        ),
        (
            "module_del_global",
            OpCode::ModuleDelGlobal,
            2,
            false,
            "module_del_global",
        ),
        (
            "module_del_global_if_present",
            OpCode::ModuleDelGlobalIfPresent,
            2,
            false,
            "module_del_global_if_present",
        ),
    ];

    for (name, opcode, operand_count, has_result, runtime_call) in cases {
        let func = make_fixed_runtime_service_func(name, opcode, operand_count, has_result);
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
        assert_eq!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Drop)),
            !has_result,
            "{name} must drop the runtime sentinel exactly when TIR has no result"
        );
    }
}

#[test]
fn preserved_copy_runtime_service_imports_stay_lir_fast_runtime_calls() {
    let cases = [
        ("module_new", "module_new", 1, true, "module_new"),
        (
            "module_import_star",
            "module_import_star",
            2,
            true,
            "module_import_star",
        ),
        (
            "bridge_unavailable",
            "bridge_unavailable",
            1,
            true,
            "bridge_unavailable",
        ),
        ("context_null", "context_null", 1, true, "context_null"),
        ("context_enter", "context_enter", 1, true, "context_enter"),
        ("context_exit", "context_exit", 2, true, "context_exit"),
        (
            "context_unwind",
            "context_unwind",
            1,
            true,
            "context_unwind",
        ),
        ("context_depth", "context_depth", 0, true, "context_depth"),
        (
            "context_unwind_to",
            "context_unwind_to",
            2,
            true,
            "context_unwind_to",
        ),
        (
            "context_closing",
            "context_closing",
            1,
            true,
            "context_closing",
        ),
    ];

    for (name, original_kind, operand_count, has_result, runtime_call) in cases {
        let func =
            make_copy_original_kind_runtime_func(name, original_kind, operand_count, has_result);
        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} preserved Copy runtime service must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn unsupported_preserved_copy_runtime_service_bails_instead_of_aliasing_operand() {
    let func = make_copy_original_kind_runtime_func(
        "exception_new_builtin_empty",
        "exception_new_builtin_empty",
        0,
        true,
    );
    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "unsupported preserved Copy runtime service must fail closed to generic emission"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
    assert!(
        !output
            .runtime_calls
            .contains(&"exception_new_builtin_empty"),
        "unsupported preserved Copy runtime service must not fake a partial LIR runtime call"
    );
}

#[test]
fn heap_alloc_stays_lir_fast_through_immediate_runtime_call() {
    let mut func = TirFunction::new("heap_alloc".into(), vec![], TirType::DynBox);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Alloc,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(32));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "ordinary heap Alloc must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"alloc"),
        "heap Alloc must call alloc; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Const(32))),
        "heap Alloc must pass its size attr as an immediate"
    );
}

#[test]
fn arena_eligible_alloc_stays_explicit_generic_fallback() {
    let mut func = TirFunction::new("arena_alloc".into(), vec![], TirType::DynBox);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Alloc,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(32));
            m.insert("arena_eligible".into(), AttrValue::Bool(true));
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
        "arena-eligible Alloc must not be heap-lowered until LIR owns arena locals"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
    assert!(
        !output.runtime_calls.contains(&"alloc"),
        "arena-eligible Alloc must not silently fall back to heap alloc"
    );
}

#[test]
fn object_new_bound_stays_lir_fast_and_selects_sized_helper_from_payload_attr() {
    let cases = [
        (
            "object_new_bound_unsized",
            None,
            "object_new_bound",
            "object_new_bound_sized",
        ),
        (
            "object_new_bound_sized",
            Some(24),
            "object_new_bound_sized",
            "object_new_bound",
        ),
    ];

    for (name, payload_size, expected_call, absent_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox],
            TirType::UserClass("Point".into()),
        );
        let result_id = func.fresh_value();
        func.value_types
            .insert(result_id, TirType::UserClass("Point".into()));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("_type_hint".into(), AttrValue::Str("Point".into()));
                if let Some(size) = payload_size {
                    m.insert("value".into(), AttrValue::Int(size));
                }
                m
            },
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
            output.runtime_calls.contains(&expected_call),
            "{name} must call {expected_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&absent_call),
            "{name} must not also call {absent_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn closure_offset_ops_stay_lir_fast_runtime_calls() {
    let cases = [
        ("closure_load", OpCode::ClosureLoad, 1, true, "closure_load"),
        (
            "closure_store",
            OpCode::ClosureStore,
            2,
            false,
            "closure_store",
        ),
    ];

    for (name, opcode, operand_count, has_result, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            if has_result {
                TirType::DynBox
            } else {
                TirType::None
            },
        );
        let result_id = has_result.then(|| {
            let id = func.fresh_value();
            func.value_types.insert(id, TirType::DynBox);
            id
        });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results: result_id.into_iter().collect(),
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(16));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: result_id.into_iter().collect(),
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&"handle_resolve"),
            "{name} must resolve closure object bits before offset access; got {:?}",
            output.runtime_calls
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            output
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::I64Const(16))),
            "{name} must pass the closure offset attr as an immediate"
        );
    }
}

#[test]
fn aggregate_builders_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "build_list",
            OpCode::BuildList,
            3,
            vec![
                ("list_builder_new", 1),
                ("list_builder_append", 3),
                ("list_builder_finish", 1),
            ],
        ),
        (
            "build_tuple",
            OpCode::BuildTuple,
            2,
            vec![
                ("list_builder_new", 1),
                ("list_builder_append", 2),
                ("tuple_builder_finish", 1),
            ],
        ),
        (
            "build_dict",
            OpCode::BuildDict,
            4,
            vec![("dict_new", 1), ("dict_set", 2)],
        ),
        (
            "build_set",
            OpCode::BuildSet,
            3,
            vec![("set_new", 1), ("set_add", 3)],
        ),
    ];

    for (name, opcode, operand_count, expected_calls) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
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
        for (runtime_call, expected_count) in expected_calls {
            let actual_count = output
                .runtime_calls
                .iter()
                .filter(|&&call| call == runtime_call)
                .count();
            assert_eq!(
                actual_count, expected_count,
                "{name} runtime call {runtime_call} count mismatch: {:?}",
                output.runtime_calls
            );
        }
    }
}

#[test]
fn operand_name_attrs_stay_lir_fast_runtime_calls() {
    let cases = [
        ("get_attr_name", OpCode::LoadAttr, 2, true, "get_attr_name"),
        (
            "set_attr_name",
            OpCode::StoreAttr,
            3,
            false,
            "set_attr_name",
        ),
        ("del_attr_name", OpCode::DelAttr, 2, false, "del_attr_name"),
    ];

    for (name, opcode, operand_count, has_result, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            if has_result {
                TirType::DynBox
            } else {
                TirType::None
            },
        );
        let result_id = has_result.then(|| {
            let id = func.fresh_value();
            func.value_types.insert(id, TirType::DynBox);
            id
        });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results: result_id.into_iter().collect(),
            attrs: {
                let mut m = AttrDict::new();
                m.insert("_original_kind".into(), AttrValue::Str(name.into()));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: result_id.into_iter().collect(),
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
fn literal_name_attrs_without_site_id_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "get_attr_generic_ptr",
            OpCode::LoadAttr,
            1,
            true,
            vec!["handle_resolve", "get_attr_ptr"],
        ),
        (
            "get_attr_special_obj",
            OpCode::LoadAttr,
            1,
            true,
            vec!["get_attr_special"],
        ),
        (
            "set_attr_generic_ptr",
            OpCode::StoreAttr,
            2,
            false,
            vec!["set_attr_object"],
        ),
        (
            "set_attr_generic_obj",
            OpCode::StoreAttr,
            2,
            false,
            vec!["set_attr_object"],
        ),
        (
            "del_attr_generic_ptr",
            OpCode::DelAttr,
            1,
            false,
            vec!["del_attr_object"],
        ),
        (
            "del_attr_generic_obj",
            OpCode::DelAttr,
            1,
            false,
            vec!["del_attr_object"],
        ),
    ];

    for (name, opcode, operand_count, has_result, runtime_calls) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            if has_result {
                TirType::DynBox
            } else {
                TirType::None
            },
        );
        let result_id = has_result.then(|| {
            let id = func.fresh_value();
            func.value_types.insert(id, TirType::DynBox);
            id
        });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results: result_id.into_iter().collect(),
            attrs: {
                let mut m = AttrDict::new();
                m.insert("_original_kind".into(), AttrValue::Str(name.into()));
                m.insert("name".into(), AttrValue::Str("field".into()));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: result_id.into_iter().collect(),
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.data_ptr_i32_literals.contains(&b"field".to_vec()),
            "{name} must carry the literal name as a LIR data pointer; got {:?}",
            output.data_ptr_i32_literals
        );
        for runtime_call in runtime_calls {
            assert!(
                output.runtime_calls.contains(&runtime_call),
                "{name} must call {runtime_call}; got {:?}",
                output.runtime_calls
            );
        }
    }
}

#[test]
fn generic_obj_literal_name_attr_uses_source_site_ic_id() {
    let func_name = "generic_obj_literal_name_attr";
    let source_op_idx = 23usize;
    let mut func = TirFunction::new(func_name.into(), vec![TirType::DynBox], TirType::DynBox);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_generic_obj".into()),
            );
            m.insert("name".into(), AttrValue::Str("field".into()));
            m.insert(
                "_source_op_idx".into(),
                AttrValue::Int(source_op_idx as i64),
            );
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "get_attr_generic_obj must stay in the LIR fast lane when source op identity is carried"
    );
    assert!(
        output.runtime_calls.contains(&"get_attr_object_ic"),
        "get_attr_generic_obj must call get_attr_object_ic; got {:?}",
        output.runtime_calls
    );
    assert!(
        output.data_ptr_i32_literals.contains(&b"field".to_vec()),
        "get_attr_generic_obj must carry the literal name as a LIR data pointer; got {:?}",
        output.data_ptr_i32_literals
    );
    let expected_site_bits = box_int_bits(stable_ic_site_id(
        func_name,
        source_op_idx,
        "get_attr_generic_obj",
    ));
    assert!(
        output
            .instructions
            .iter()
            .any(|inst| matches!(inst, Instruction::I64Const(bits) if *bits == expected_site_bits)),
        "get_attr_generic_obj must use source op identity for the IC site id"
    );
}

#[test]
#[should_panic(expected = "get_attr_generic_obj requires source op index")]
fn generic_obj_literal_name_attr_without_source_op_index_fails_closed() {
    let mut func = TirFunction::new(
        "get_attr_generic_obj_without_source".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_generic_obj".into()),
            );
            m.insert("name".into(), AttrValue::Str("field".into()));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let _ = lower_tir_to_wasm(&func);
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
fn dynbox_inplace_arithmetic_uses_generated_numeric_lir_helpers() {
    let cases = [
        (
            "inplace_add_dynbox",
            OpCode::InplaceAdd,
            "inplace_add",
            "add",
        ),
        (
            "inplace_sub_dynbox",
            OpCode::InplaceSub,
            "inplace_sub",
            "sub",
        ),
        (
            "inplace_mul_dynbox",
            OpCode::InplaceMul,
            "inplace_mul",
            "mul",
        ),
    ];

    for (name, opcode, expected_runtime_call, rejected_runtime_call) in cases {
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
            output.runtime_calls.contains(&expected_runtime_call),
            "{name} must call {expected_runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&rejected_runtime_call),
            "{name} must not collapse to {rejected_runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn preserved_copy_numeric_helpers_use_generated_fixed_runtime_selector() {
    let cases = [
        ("inplace_div", 2, "inplace_div"),
        ("inplace_floordiv", 2, "inplace_floordiv"),
        ("inplace_mod", 2, "inplace_mod"),
        ("inplace_pow", 2, "inplace_pow"),
        ("matmul", 2, "matmul"),
        ("inplace_matmul", 2, "inplace_matmul"),
        ("pow_mod", 3, "pow_mod"),
        ("round", 3, "round"),
        ("trunc", 1, "trunc"),
        ("string_eq", 2, "string_eq"),
        ("shl", 2, "lshift"),
        ("shr", 2, "rshift"),
        ("bit_not", 1, "invert"),
        ("unary_neg", 1, "neg"),
        ("unary_pos", 1, "pos"),
    ];

    for (original_kind, operand_count, runtime_call) in cases {
        let func =
            make_copy_original_kind_runtime_func(original_kind, original_kind, operand_count, true);
        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{original_kind} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{original_kind} must call {runtime_call}; got {:?}",
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

fn make_binary_two_consts_func(name: &str, opcode: OpCode, lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], TirType::I64);
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
        opcode,
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

fn make_add_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    make_binary_two_consts_func("add_two_consts", OpCode::Add, lhs, rhs)
}

fn make_checked_mul_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new("checked_mul_two_consts".into(), vec![], TirType::I64);
    let lhs_id = func.fresh_value();
    let rhs_id = func.fresh_value();
    let product_id = func.fresh_value();
    let flag_id = func.fresh_value();
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
        opcode: OpCode::CheckedMul,
        operands: vec![lhs_id, rhs_id],
        results: vec![product_id, flag_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![product_id],
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

fn has_native_binary_instruction(instructions: &[Instruction<'static>], opcode: OpCode) -> bool {
    instructions.iter().any(|instruction| match opcode {
        OpCode::Add => matches!(instruction, Instruction::I64Add),
        OpCode::Sub => matches!(instruction, Instruction::I64Sub),
        OpCode::Mul => matches!(instruction, Instruction::I64Mul),
        other => panic!("unsupported native binary assertion for {other:?}"),
    })
}

#[test]
fn checked_overflow_triples_use_opcode_specific_runtime_helpers_without_generic_bail() {
    let cases = [
        ("checked_add_overflow_triple", OpCode::Add, 20, 22, "add"),
        ("checked_sub_overflow_triple", OpCode::Sub, 42, 20, "sub"),
        ("checked_mul_overflow_triple", OpCode::Mul, 6, 7, "mul"),
    ];

    for (name, opcode, lhs, rhs, runtime_call) in cases {
        let func = make_binary_two_consts_func(name, opcode, lhs, rhs);
        let output = lower_tir_to_wasm(&func).test_view();

        assert_eq!(
            output.bail_to_generic_reason, None,
            "{name} must stay in the LIR fast body"
        );
        assert!(
            has_native_binary_instruction(&output.instructions, opcode),
            "{name} must emit the hot raw WASM instruction for {opcode:?}"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} overflow side channel must dispatch through {runtime_call}; got {:?}",
            output.runtime_calls
        );
        for other in ["add", "sub", "mul"] {
            if other != runtime_call {
                assert!(
                    !output.runtime_calls.contains(&other),
                    "{name} must not call the {other} helper for a {opcode:?} overflow side channel; got {:?}",
                    output.runtime_calls
                );
            }
        }
    }
}

/// The perf-preservation direction: when BOTH operands are proven
/// `RawI64Safe`, the fast `i64.add` is still emitted (the checked-overflow
/// triple). The cold overflow-box side channel is a typed runtime call, not a
/// generic body bail.
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
    assert_eq!(output.bail_to_generic_reason, None);
    assert!(
        output.runtime_calls.contains(&"add"),
        "checked-overflow cold side channel must use the typed add helper"
    );
}

#[test]
fn checked_mul_raw_i64_emits_exact_wasm_overflow_flag_without_generic_bail() {
    let func = make_checked_mul_two_consts_func(6, 7);
    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(
        output.bail_to_generic_reason, None,
        "raw checked_mul must stay in the LIR fast body"
    );
    assert!(
        !output.runtime_calls.contains(&"mul"),
        "raw CheckedMul produces a raw carrier and must not route through boxed molt_mul"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Mul)),
        "raw CheckedMul must emit the wrapping i64.mul product"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64DivS)),
        "raw CheckedMul must emit the exact product/lhs overflow check"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(value) if *value == i64::MIN)
        ),
        "raw CheckedMul must guard the i64::MIN / -1 division trap"
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
