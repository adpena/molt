//! CPython-semantic teeth for SCCP builtin and method folding.

use super::super::ConstVal;
use super::builtins::eval_concrete_builtin;
use super::methods::eval_concrete_method;

fn s(v: &str) -> ConstVal {
    ConstVal::Str(v.to_string())
}

fn builtin(name: &str, args: &[ConstVal]) -> Option<ConstVal> {
    let ops: Vec<Option<&ConstVal>> = args.iter().map(Some).collect();
    eval_concrete_builtin(name, &ops)
}

fn method(receiver_type: &str, name: &str, args: &[ConstVal]) -> Option<ConstVal> {
    let ops: Vec<Option<&ConstVal>> = args.iter().map(Some).collect();
    eval_concrete_method(receiver_type, name, &ops)
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
    assert_eq!(builtin("str", &[s("café")]), Some(s("café")));
}

#[test]
fn find_returns_code_point_index() {
    assert_eq!(
        method("str", "find", &[s("héllo"), s("llo")]),
        Some(ConstVal::Int(2))
    );
    assert_eq!(
        method("str", "find", &[s("a😀b"), s("b")]),
        Some(ConstVal::Int(2))
    );
    assert_eq!(
        method("str", "find", &[s("héllo"), s("z")]),
        Some(ConstVal::Int(-1))
    );
    assert_eq!(
        method("str", "find", &[s("abc"), s("bc")]),
        Some(ConstVal::Int(1))
    );
}

#[test]
fn rfind_returns_code_point_index() {
    assert_eq!(
        method("str", "rfind", &[s("héllo"), s("l")]),
        Some(ConstVal::Int(3))
    );
    assert_eq!(
        method("str", "rfind", &[s("héllo"), s("z")]),
        Some(ConstVal::Int(-1))
    );
}

#[test]
fn count_empty_needle_is_code_point_len_plus_one() {
    assert_eq!(
        method("str", "count", &[s("café"), s("")]),
        Some(ConstVal::Int(5))
    );
    assert_eq!(
        method("str", "count", &[s("abc"), s("")]),
        Some(ConstVal::Int(4))
    );
}

#[test]
fn zfill_pads_to_code_point_width() {
    assert_eq!(
        method("str", "zfill", &[s("é"), ConstVal::Int(3)]),
        Some(s("00é"))
    );
    assert_eq!(
        method("str", "zfill", &[s("-é"), ConstVal::Int(4)]),
        Some(s("-00é"))
    );
}

#[test]
fn zfill_non_positive_width_returns_unchanged_without_panic() {
    assert_eq!(
        method("str", "zfill", &[s("5"), ConstVal::Int(-3)]),
        Some(s("5"))
    );
    assert_eq!(
        method("str", "zfill", &[s("café"), ConstVal::Int(0)]),
        Some(s("café"))
    );
}
