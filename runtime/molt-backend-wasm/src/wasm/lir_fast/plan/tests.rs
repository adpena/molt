use super::{
    WasmFunctionLoweringPlan, compute_lir_wasm_lowering_plans_from_final_ir_with_escaped,
    prepare_lir_wasm_fast_plan,
};
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use crate::wasm::body::WasmLirFallbackReason;
use crate::{FunctionIR, SimpleIR};
use std::collections::BTreeSet;
use wasm_encoder::Instruction;

fn const_i64_return_func(value: i64) -> TirFunction {
    let mut func = TirFunction::new("const_i64_return".into(), vec![], TirType::I64);
    let result = func.fresh_value();
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn literal_const_return_func(
    name: &str,
    opcode: OpCode,
    return_type: TirType,
    attrs: AttrDict,
) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], return_type);
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn add_two_i64_params_func() -> TirFunction {
    let mut func = TirFunction::new(
        "add_two_i64_params".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn checked_mul_i64_consts_func() -> TirFunction {
    let mut func = TirFunction::new("checked_mul_i64_consts".into(), vec![], TirType::I64);
    let lhs = func.fresh_value();
    let rhs = func.fresh_value();
    let product = func.fresh_value();
    let overflow = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs, 6), (rhs, 7)] {
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
        operands: vec![lhs, rhs],
        results: vec![product, overflow],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![product],
    };
    func
}

#[test]
fn wasm_lir_fast_plan_accepts_proven_i64_body() {
    let plan = prepare_lir_wasm_fast_plan(&const_i64_return_func(42));
    let body = plan
        .lir_fast_body()
        .expect("proven i64 const return must keep the LIR fast plan");

    assert_eq!(plan.generic_reason(), None);
    assert_eq!(body.bail_to_generic_reason(), None);
}

#[test]
fn wasm_lir_fast_plan_accepts_materialized_literal_consts() {
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
        let plan = prepare_lir_wasm_fast_plan(&literal_const_return_func(
            name,
            opcode,
            return_type,
            attrs,
        ));
        let body = plan
            .lir_fast_body()
            .unwrap_or_else(|| panic!("{name} must stay on the LIR fast plan"));
        let view = body.test_view();

        assert_eq!(plan.generic_reason(), None);
        assert_eq!(body.bail_to_generic_reason(), None);
        assert!(
            view.runtime_calls.contains(&import_name),
            "{name} must materialize through {import_name}; got {:?}",
            view.runtime_calls
        );
    }
}

#[test]
fn wasm_lir_fast_plan_accepts_boxed_single_return_carriers() {
    let cases = [
        {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Bool(true));
            ("const_bool_return", OpCode::ConstBool, TirType::Bool, attrs)
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert("f_value".into(), AttrValue::Float(1.25));
            (
                "const_float_return",
                OpCode::ConstFloat,
                TirType::F64,
                attrs,
            )
        },
    ];

    for (name, opcode, return_type, attrs) in cases {
        let plan = prepare_lir_wasm_fast_plan(&literal_const_return_func(
            name,
            opcode,
            return_type,
            attrs,
        ));
        let body = plan
            .lir_fast_body()
            .unwrap_or_else(|| panic!("{name} must stay on the boxed-i64 fast plan"));

        assert_eq!(plan.generic_reason(), None);
        assert_eq!(body.result_types, vec![wasm_encoder::ValType::I64]);
        assert_eq!(body.bail_to_generic_reason(), None);
    }
}

#[test]
fn wasm_lir_fast_plan_records_boxed_i64_abi_rejection() {
    let plan = prepare_lir_wasm_fast_plan(&add_two_i64_params_func());

    assert_eq!(
        plan.generic_reason(),
        Some(WasmLirFallbackReason::BoxedI64AbiUnsupported)
    );
}

#[test]
fn wasm_lir_fast_plan_accepts_raw_checked_mul_body() {
    let plan = prepare_lir_wasm_fast_plan(&checked_mul_i64_consts_func());
    let body = plan.lir_fast_body().unwrap_or_else(|| {
        panic!(
            "raw checked_mul const body must keep the LIR fast plan; reason={:?}",
            plan.generic_reason()
        )
    });
    let view = body.test_view();

    assert_eq!(plan.generic_reason(), None);
    assert_eq!(body.bail_to_generic_reason(), None);
    assert!(
        view.instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Mul)),
        "checked_mul plan body must emit wrapping i64.mul"
    );
    assert!(
        view.instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64DivS)),
        "checked_mul plan body must emit exact overflow division check"
    );
}

#[test]
fn wasm_lir_fast_plan_records_escaped_callable_reason() {
    let name = "pkg____molt_globals_builtin__escaped".to_string();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: name.clone(),
            params: vec![],
            ops: vec![],
            param_types: Some(vec![]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let escaped = BTreeSet::from([name.clone()]);
    let plans = compute_lir_wasm_lowering_plans_from_final_ir_with_escaped(&ir, &escaped);

    assert_eq!(
        plans
            .get(&name)
            .and_then(WasmFunctionLoweringPlan::generic_reason),
        Some(WasmLirFallbackReason::EscapedCallableTarget)
    );
}
