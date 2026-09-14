//! CPython-semantic teeth for SCCP builtin and operation folding.

use super::super::ConstVal;
use super::builtins::eval_concrete_builtin;
use super::ops::evaluate_op;
use crate::tir::ops::OpCode;

#[test]
fn immutable_tuple_constants_share_nested_storage() {
    let leaf = ConstVal::Tuple(vec![ConstVal::Int(1)].into());
    let nested = evaluate_op(OpCode::BuildTuple, &[Some(&leaf), Some(&leaf)]).unwrap();
    let ConstVal::Tuple(children) = nested else {
        panic!("immutable constructor must preserve tuple identity in the domain");
    };
    let (ConstVal::Tuple(first), ConstVal::Tuple(second)) = (&children[0], &children[1]) else {
        panic!("nested tuple elements must remain immutable tuple constants");
    };
    assert!(
        std::sync::Arc::ptr_eq(first, second),
        "nested values must share storage instead of cloning the value graph"
    );
}

#[test]
fn empty_immutable_sequence_repeat_is_independent_of_repeat_count() {
    let count = ConstVal::Int(i64::MAX);
    for empty in [ConstVal::Tuple(Vec::new().into()), s("")] {
        assert_eq!(
            evaluate_op(OpCode::Mul, &[Some(&empty), Some(&count)]),
            Some(empty.clone())
        );
        assert_eq!(
            evaluate_op(OpCode::Mul, &[Some(&count), Some(&empty)]),
            Some(empty)
        );
    }
}

#[test]
fn immutable_sequence_repeat_does_not_truncate_counts_to_host_usize() {
    // On a 32-bit compiler host this count must not truncate to one.
    let count = ConstVal::Int((1_i64 << 32) + 1);
    for value in [ConstVal::Tuple(vec![ConstVal::Int(1)].into()), s("x")] {
        assert_eq!(
            evaluate_op(OpCode::Mul, &[Some(&value), Some(&count)]),
            None
        );
        assert_eq!(
            evaluate_op(OpCode::Mul, &[Some(&count), Some(&value)]),
            None
        );
    }
}

fn s(v: &str) -> ConstVal {
    ConstVal::Str(v.to_string())
}

fn builtin(name: &str, args: &[ConstVal]) -> Option<ConstVal> {
    let ops: Vec<Option<&ConstVal>> = args.iter().map(Some).collect();
    eval_concrete_builtin(name, &ops)
}

#[test]
fn len_counts_code_points_not_bytes() {
    assert_eq!(builtin("len", &[s("café")]), Some(ConstVal::Int(4)));
    assert_eq!(builtin("len", &[s("héllo")]), Some(ConstVal::Int(5)));
    assert_eq!(builtin("len", &[s("a😀b")]), Some(ConstVal::Int(3)));
    assert_eq!(builtin("len", &[s("abc")]), Some(ConstVal::Int(3)));
}

#[test]
fn str_repr_fold_matches_cpython_or_refuses() {
    for v in [
        0.0_f64,
        -0.0,
        1.5,
        100.0,
        0.1,
        1234.5678,
        0.0001,
        9.5e15,
        f64::from_bits(0x4289368ec8725340),
        1e-5_f64,
        1e16,
        1e17,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ] {
        assert_eq!(
            builtin("str", &[ConstVal::Float(v)]),
            None,
            "str({v}) must defer"
        );
        assert_eq!(
            builtin("repr", &[ConstVal::Float(v)]),
            None,
            "repr({v}) must defer"
        );
    }

    for (input, expected) in [
        ("abc", "'abc'"),
        ("a b c", "'a b c'"),
        ("a\"b", "'a\"b'"),
        ("x!@#$%", "'x!@#$%'"),
    ] {
        assert_eq!(
            builtin("repr", &[s(input)]),
            Some(ConstVal::Str(expected.to_string())),
            "repr({input:?})"
        );
    }
    for input in ["it's", "a\\b", "a\nb", "café"] {
        assert_eq!(
            builtin("repr", &[s(input)]),
            None,
            "repr({input:?}) must defer"
        );
    }
    assert_eq!(builtin("str", &[s("café")]), None);
}

#[test]
fn builtin_argument_forms_reject_missing_and_extra_operands() {
    let int = ConstVal::Int(2);
    let cases = vec![
        ("len", vec![s("abc")]),
        ("abs", vec![int.clone()]),
        ("repr", vec![int.clone()]),
        ("chr", vec![int.clone()]),
        ("ord", vec![s("a")]),
        ("hex", vec![int.clone()]),
        ("oct", vec![int.clone()]),
        ("bin", vec![int.clone()]),
        ("sum", vec![ConstVal::Tuple(vec![int.clone()].into())]),
        ("min", vec![int.clone(), int.clone()]),
        ("max", vec![int.clone(), int.clone()]),
    ];
    for (name, args) in cases {
        assert!(builtin(name, &args).is_some(), "valid {name} {args:?}");
        assert_eq!(builtin(name, &args[..args.len() - 1]), None, "short {name}");
        let mut extra = args;
        extra.push(ConstVal::Int(999));
        assert_eq!(builtin(name, &extra), None, "extra {name}");
    }
    for count in 0..=4 {
        let args = vec![int.clone(); count];
        assert_eq!(builtin("range_new", &args).is_some(), count == 3);
        assert_eq!(builtin("range", &args), None);
    }
}

#[test]
fn opcode_evaluator_uses_generated_operand_and_result_admission() {
    use crate::tir::op_kinds_generated::{
        ALL_OPCODES, SccpConstantEvalRule, opcode_accepts_shape,
        opcode_sccp_constant_eval_rule_table,
    };
    for &opcode in ALL_OPCODES {
        if opcode_sccp_constant_eval_rule_table(opcode) == SccpConstantEvalRule::None {
            continue;
        }
        for count in 0..=4 {
            if opcode_accepts_shape(opcode, count, 1) {
                continue;
            }
            let values = vec![ConstVal::Int(1); count];
            let operands: Vec<_> = values.iter().map(Some).collect();
            assert_eq!(
                super::ops::evaluate_op(opcode, &operands),
                None,
                "{opcode:?}/{count}"
            );
        }
    }
}

#[test]
fn integer_magnitude_folds_cover_the_signed_minimum_without_overflow() {
    assert_eq!(
        builtin("hex", &[ConstVal::Int(i64::MIN)]),
        Some(s("-0x8000000000000000"))
    );
    assert_eq!(
        builtin("oct", &[ConstVal::Int(i64::MIN)]),
        Some(s("-0o1000000000000000000000"))
    );
    assert_eq!(
        builtin("bin", &[ConstVal::Int(i64::MIN)]),
        Some(s(&format!("-0b1{}", "0".repeat(63))))
    );
    assert_eq!(builtin("abs", &[ConstVal::Int(i64::MIN)]), None);
}

#[test]
fn float_min_max_preserve_python_selected_operand_bits() {
    let nan = f64::from_bits(0x7ff8_0000_0000_0042);
    for name in ["min", "max"] {
        for (left, right, expected) in [
            (nan, 1.0, nan.to_bits()),
            (1.0, nan, 1.0_f64.to_bits()),
            (-0.0, 0.0, (-0.0_f64).to_bits()),
            (0.0, -0.0, 0.0_f64.to_bits()),
        ] {
            let Some(ConstVal::Float(value)) =
                builtin(name, &[ConstVal::Float(left), ConstVal::Float(right)])
            else {
                panic!("valid {name} must fold");
            };
            assert_eq!(value.to_bits(), expected, "{name}({left:?}, {right:?})");
        }
        assert_eq!(
            builtin(name, &[ConstVal::Float(2.0), ConstVal::Float(1.0)]),
            Some(ConstVal::Float(if name == "min" { 1.0 } else { 2.0 }))
        );
    }
}

#[test]
fn host_libm_results_do_not_establish_target_semantic_admission() {
    for name in [
        "math.sqrt",
        "math.log",
        "math.exp",
        "math.sin",
        "math.cos",
        "math.tan",
        "math.asin",
        "math.acos",
        "math.atan",
    ] {
        for value in [0.5, -1.0, 0.0, 1000.0, f64::INFINITY, f64::NAN] {
            assert_eq!(
                builtin(name, &[ConstVal::Float(value)]),
                None,
                "{name}({value:?})"
            );
        }
    }
    for name in ["math.pow", "math.atan2", "math.hypot"] {
        for (left, right) in [(2.0, 3.0), (-1.0, 0.5), (0.0, -1.0), (1e308, 2.0)] {
            assert_eq!(
                builtin(name, &[ConstVal::Float(left), ConstVal::Float(right)]),
                None,
                "{name}({left}, {right})"
            );
        }
    }
}

#[test]
fn float_power_and_divmod_defer_without_target_semantic_primitives() {
    use crate::tir::ops::OpCode;
    for opcode in [OpCode::Pow, OpCode::FloorDiv, OpCode::Mod] {
        for (left, right) in [
            (2.0, 3.0),
            (1.0, 0.1),
            (-0.0, 3.0),
            (0.0, -3.0),
            (-1.0, 0.5),
            (0.0, -1.0),
            (1.0, 0.0),
            (1e308, 2.0),
            (f64::INFINITY, 1.0),
            (1.0, f64::NAN),
        ] {
            let values = [ConstVal::Float(left), ConstVal::Float(right)];
            assert_eq!(
                super::ops::evaluate_op(opcode, &[Some(&values[0]), Some(&values[1])]),
                None,
                "{opcode:?}({left:?}, {right:?})"
            );
        }
    }
    let values = [ConstVal::Int(-7), ConstVal::Int(3)];
    let operands = [Some(&values[0]), Some(&values[1])];
    assert_eq!(
        super::ops::evaluate_op(OpCode::FloorDiv, &operands),
        Some(ConstVal::Int(-3))
    );
    assert_eq!(
        super::ops::evaluate_op(OpCode::Mod, &operands),
        Some(ConstVal::Int(2))
    );
    assert_eq!(
        super::ops::evaluate_op(OpCode::Pow, &operands),
        Some(ConstVal::Int(-343))
    );
}

#[test]
fn integer_true_division_never_rounds_its_operands_before_the_ratio() {
    use crate::tir::ops::OpCode;
    for (left, right) in [
        (9_007_199_254_740_993, 3),
        (-9_007_199_254_740_993, 3),
        (3, 9_007_199_254_740_993),
        (i64::MAX, 1),
        (1, i64::MAX),
        (1, 0),
    ] {
        let values = [ConstVal::Int(left), ConstVal::Int(right)];
        assert_eq!(
            super::ops::evaluate_op(OpCode::Div, &[Some(&values[0]), Some(&values[1])]),
            None,
            "{left}/{right}"
        );
    }
    for (left, right, expected) in [
        (7, 2, 3.5),
        (-7, 2, -3.5),
        (9_007_199_254_740_994, 2, 4_503_599_627_370_497.0),
        (i64::MIN, 1, -9_223_372_036_854_775_808.0),
        (0, -1, -0.0),
    ] {
        let values = [ConstVal::Int(left), ConstVal::Int(right)];
        let Some(ConstVal::Float(actual)) =
            super::ops::evaluate_op(OpCode::Div, &[Some(&values[0]), Some(&values[1])])
        else {
            panic!("exact operands {left}/{right} must fold");
        };
        assert_eq!(
            actual.to_bits(),
            (expected as f64).to_bits(),
            "{left}/{right}"
        );
    }
}

#[test]
fn truthiness_uses_real_boolean_operations_not_mutable_builtin_lookup() {
    use crate::tir::ops::OpCode;
    for (value, expected) in [
        (ConstVal::None, false),
        (ConstVal::Bool(true), true),
        (ConstVal::Int(0), false),
        (ConstVal::Int(-1), true),
        (ConstVal::Float(-0.0), false),
        (ConstVal::Float(f64::NAN), true),
        (s(""), false),
        (s("é"), true),
        (ConstVal::Tuple(Vec::new().into()), false),
        (ConstVal::Tuple(vec![ConstVal::None].into()), true),
        (
            ConstVal::Range {
                start: 10,
                stop: 0,
                step: 1,
            },
            false,
        ),
        (
            ConstVal::Range {
                start: i64::MIN,
                stop: i64::MAX,
                step: 1,
            },
            true,
        ),
    ] {
        for opcode in [OpCode::Bool, OpCode::Not] {
            assert_eq!(
                super::ops::evaluate_op(opcode, &[Some(&value)]),
                Some(ConstVal::Bool(if opcode == OpCode::Not {
                    !expected
                } else {
                    expected
                }))
            );
        }
        assert_eq!(builtin("bool", &[value]), None);
    }
}

#[test]
fn compound_materialization_checks_recursive_cost_before_allocation() {
    use super::super::MAX_COMPOUND_ELEMENTS;
    use crate::tir::ops::OpCode;
    let eval = |opcode, values: &[ConstVal]| {
        super::ops::evaluate_op(opcode, &values.iter().map(Some).collect::<Vec<_>>())
    };
    let large = s(&"x".repeat(MAX_COMPOUND_ELEMENTS));
    assert_eq!(eval(OpCode::Add, &[large.clone(), s("y")]), None);
    assert_eq!(builtin("repr", &[large.clone()]), None);
    assert_eq!(eval(OpCode::Mul, &[s("x"), ConstVal::Int(i64::MAX)]), None);
    assert_eq!(
        eval(
            OpCode::Mul,
            &[
                ConstVal::Tuple(vec![large.clone()].into()),
                ConstVal::Int(2)
            ]
        ),
        None
    );
    assert_eq!(eval(OpCode::BuildTuple, &[large.clone()]), None);
    let half = ConstVal::Tuple(vec![s(&"x".repeat(501))].into());
    assert_eq!(eval(OpCode::Add, &[half.clone(), half.clone()]), None);
    assert_eq!(eval(OpCode::BuildTuple, &[half.clone(), half]), None);
    for empty in [s(""), ConstVal::Tuple(Vec::new().into())] {
        // Even i64::MAX repeats of an empty sequence take constant work.
        assert_eq!(
            eval(OpCode::Mul, &[empty.clone(), ConstVal::Int(i64::MAX)]),
            Some(empty)
        );
    }
    assert_eq!(eval(OpCode::Add, &[s("a"), s("b")]), Some(s("ab")));
    assert_eq!(
        eval(OpCode::BuildTuple, &[ConstVal::Int(1)]),
        Some(ConstVal::Tuple(vec![ConstVal::Int(1)].into()))
    );
}

#[test]
fn arithmetic_never_embeds_host_selected_nan_payloads() {
    use crate::tir::ops::OpCode;
    for (opcode, left, right) in [
        (OpCode::Add, f64::INFINITY, f64::NEG_INFINITY),
        (OpCode::Sub, f64::INFINITY, f64::INFINITY),
        (OpCode::Mul, 0.0, f64::INFINITY),
        (OpCode::Div, f64::INFINITY, f64::INFINITY),
        (OpCode::Add, f64::from_bits(0x7ff8_0000_0000_0042), 1.0),
    ] {
        let values = [ConstVal::Float(left), ConstVal::Float(right)];
        assert_eq!(
            super::ops::evaluate_op(opcode, &[Some(&values[0]), Some(&values[1])]),
            None
        );
    }
    let values = [ConstVal::Float(1.5), ConstVal::Float(2.5)];
    assert_eq!(
        super::ops::evaluate_op(OpCode::Add, &[Some(&values[0]), Some(&values[1])]),
        Some(ConstVal::Float(4.0))
    );
}
