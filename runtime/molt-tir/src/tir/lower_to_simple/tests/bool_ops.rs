use super::*;

fn op_const_bool(out: &str, val: bool) -> OpIR {
    OpIR {
        kind: "const_bool".to_string(),
        out: Some(out.to_string()),
        value: Some(if val { 1 } else { 0 }),
        ..OpIR::default()
    }
}

fn op_not(arg: &str, out: &str) -> OpIR {
    OpIR {
        kind: "not".to_string(),
        args: Some(vec![arg.to_string()]),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

fn op_ret(arg: &str) -> OpIR {
    OpIR {
        kind: "ret".to_string(),
        args: Some(vec![arg.to_string()]),
        var: Some(arg.to_string()),
        ..OpIR::default()
    }
}

/// Regression: `__bool__` returning a literal `False`/`True` must round-trip
/// through TIR with the const_bool value preserved as `AttrValue::Bool` (not
/// `AttrValue::Int`).  When the SSA lift stored ConstBool as `AttrValue::Int`,
/// downstream codegen at the function-return path silently TAG_INT-boxed
/// the 0/1 value, producing a boxed int instead of a boxed bool.  The
/// runtime's `as_bool()` predicate then rejected the value, raising
/// `TypeError: __bool__ should return bool, returned int`.
///
/// This test exercises the exact `__bool__`-method shape: `const_bool;
/// ret`.  After the fix in commit 8662b45f and the matching
/// `ensure_boxed_primitive_safe` bool-aware repath, the const_bool's
/// `value` attribute must arrive at lower_to_simple_ir as
/// `AttrValue::Bool(false)`/`AttrValue::Bool(true)` and the resulting
/// const_bool OpIR must carry a 0/1 value field intact.
#[test]
fn bool_method_return_preserves_const_bool_value() {
    for (return_value, expected_int) in [(false, 0i64), (true, 1i64)] {
        let func = FunctionIR {
            name: "Falsy___bool__".to_string(),
            params: vec!["self".to_string()],
            ops: vec![op_const_bool("retv", return_value), op_ret("retv")],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func);
        // SSA lift must store ConstBool's value as AttrValue::Bool, not Int.
        let mut found_const_bool = false;
        for block in tir.blocks.values() {
            for op in &block.ops {
                if op.opcode == crate::tir::ops::OpCode::ConstBool {
                    found_const_bool = true;
                    match op.attrs.get("value") {
                        Some(crate::tir::ops::AttrValue::Bool(b)) => {
                            assert_eq!(
                                *b, return_value,
                                "const_bool value attribute must match the literal"
                            );
                        }
                        other => panic!(
                            "const_bool value attribute must be AttrValue::Bool({return_value}), got {other:?}"
                        ),
                    }
                }
            }
        }
        assert!(
            found_const_bool,
            "TIR must contain a const_bool op for the __bool__ return"
        );

        // Roundtrip: TIR → SimpleIR.
        let roundtripped = super::lower_to_simple_ir(&tir);

        // The roundtripped const_bool must carry value=0 for False, value=1
        // for True.  If the ssa lift stored AttrValue::Int(0) instead of
        // AttrValue::Bool(false), the downstream None branch would fall
        // through to value=Some(0) — masking the bug at this layer but
        // failing at the cranelift box site.  Asserting on the roundtripped
        // value pins the contract end-to-end.
        let const_bool_op = roundtripped
            .iter()
            .find(|op| op.kind == "const_bool")
            .expect("const_bool must survive roundtrip");
        assert_eq!(
            const_bool_op.value,
            Some(expected_int),
            "const_bool value field must be {expected_int} for return_value={return_value}"
        );

        // The ret op must reference the const_bool variable directly, not
        // an int copy or coerced value.
        let ret_op = roundtripped
            .iter()
            .find(|op| op.kind == "ret")
            .expect("ret op must survive roundtrip");
        let ret_args = ret_op.args.as_ref().expect("ret must have args");
        assert_eq!(ret_args.len(), 1, "ret must have exactly 1 arg");
        let const_bool_out = const_bool_op
            .out
            .as_ref()
            .expect("const_bool must have out var");
        assert_eq!(
            &ret_args[0], const_bool_out,
            "ret must consume the const_bool variable directly"
        );
    }
}

#[test]
fn not_true_roundtrip_preserves_operand() {
    let func = FunctionIR {
        name: "test_not".to_string(),
        params: vec![],
        ops: vec![op_const_bool("x", true), op_not("x", "y"), op_ret("y")],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func);
    // Roundtrip: TIR → SimpleIR
    let roundtripped = super::lower_to_simple_ir(&tir);

    // Find the "not" op
    let not_op = roundtripped.iter().find(|op| op.kind == "not");
    assert!(not_op.is_some(), "not op must survive roundtrip");

    let not_op = not_op.unwrap();
    let not_args = not_op.args.as_ref().expect("not must have args");
    assert_eq!(not_args.len(), 1, "not must have exactly 1 arg");

    // The arg must reference a variable that is defined by const_bool
    let arg_name = &not_args[0];
    let const_op = roundtripped
        .iter()
        .find(|op| op.kind == "const_bool" && op.out.as_deref() == Some(arg_name));
    assert!(
        const_op.is_some(),
        "not's operand '{}' must be defined by a const_bool op. ops: {:?}",
        arg_name,
        roundtripped
            .iter()
            .map(|op| format!("{} out={:?} args={:?}", op.kind, op.out, op.args))
            .collect::<Vec<_>>()
    );
}
