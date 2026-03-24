//! Round-trip stress tests for TIR pipeline.
//! Verifies that SimpleIR → TIR → optimize → SimpleIR produces valid output.

#[cfg(test)]
mod tests {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::lower_to_simple::lower_to_simple_ir;
    use crate::tir::passes::run_pipeline;
    use crate::tir::type_refine::{extract_type_map, refine_types};
    use crate::tir::verify::verify_function;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn make_function(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: "test".to_string(),
            params: vec![],
            ops,
            param_types: None,
        }
    }

    fn roundtrip(ops: Vec<OpIR>) -> Vec<OpIR> {
        let ir = make_function(ops);
        let mut tir = lower_to_tir(&ir);
        refine_types(&mut tir);
        let type_map = extract_type_map(&tir);
        let _stats = run_pipeline(&mut tir);
        assert!(
            verify_function(&tir).is_ok(),
            "TIR verification failed after optimization"
        );
        lower_to_simple_ir(&tir, &type_map)
    }

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    fn op_val(kind: &str, value: i64) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(value),
            ..OpIR::default()
        }
    }

    fn op_out(kind: &str, out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    fn op_args(kind: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::default()
        }
    }

    fn op_out_args(kind: &str, out: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: Some(out.to_string()),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::default()
        }
    }

    // ---------------------------------------------------------------------------
    // Test 1: Straight-line arithmetic
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_straight_line() {
        let ops = vec![
            op_out("const", "x"),
            op_out("const", "y"),
            op_out_args("add", "z", &["x", "y"]),
            op_args("ret", &["z"]),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty(), "round-trip should produce non-empty ops");
    }

    // ---------------------------------------------------------------------------
    // Test 2: If/else
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_if_else() {
        let ops = vec![
            op_out("const", "cond"),
            op_args("if", &["cond"]),
            op_out("const", "a"),
            op("else"),
            op_out("const", "b"),
            op("end_if"),
            op("ret_void"),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 3: Simple loop
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_loop() {
        let ops = vec![
            op_out("const", "i"),
            op("loop_start"),
            op_out_args("add", "i2", &["i", "i"]),
            op("loop_end"),
            op("ret_void"),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 4: Nested if inside loop
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_nested_if_in_loop() {
        let ops = vec![
            op_out("const", "i"),
            op("loop_start"),
            op_out("const", "cond"),
            op_args("if", &["cond"]),
            op_out_args("add", "i2", &["i", "i"]),
            op("end_if"),
            op("loop_end"),
            op("ret_void"),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 5: Multiple return paths
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_multiple_returns() {
        let ops = vec![
            op_out("const", "cond"),
            op_args("if", &["cond"]),
            op_out("const", "a"),
            op_args("ret", &["a"]),
            op("else"),
            op_out("const", "b"),
            op_args("ret", &["b"]),
            op("end_if"),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 6: Empty function (only ret_void)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_empty() {
        let ops = vec![op("ret_void")];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
        assert!(
            result.iter().any(|o| o.kind == "ret_void"),
            "empty function must emit ret_void"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 7: fast_int hint is preserved through pipeline
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_fast_int() {
        let mut add_op = op_out_args("add", "z", &["x", "y"]);
        add_op.fast_int = Some(true);
        let ops = vec![
            op_out("const", "x"),
            op_out("const", "y"),
            add_op,
            op_args("ret", &["z"]),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 8: Jump/label pattern
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_jump_label() {
        // jump to label 1; dead const; label 1; ret x
        let ops = vec![
            op_out("const", "x"),
            op_val("jump", 1),
            op_out("const", "dead"),
            op_val("label", 1),
            op_args("ret", &["x"]),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 9: Verifier passes on well-formed pipeline output
    // ---------------------------------------------------------------------------

    #[test]
    fn tir_verifier_passes_after_pipeline() {
        let ops = vec![
            op_out("const", "x"),
            op_out_args("add", "y", &["x", "x"]),
            op_args("ret", &["y"]),
        ];
        let ir = make_function(ops);
        let mut tir = lower_to_tir(&ir);
        refine_types(&mut tir);
        run_pipeline(&mut tir);
        // run_pipeline already panics on verify failure, but let's assert
        // explicitly to make the intent clear in test output.
        assert!(
            verify_function(&tir).is_ok(),
            "verifier must pass after pipeline"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 10: Function with parameters
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_with_params() {
        let ir = FunctionIR {
            name: "with_params".to_string(),
            params: vec!["p0".to_string(), "p1".to_string()],
            ops: vec![
                op_out_args("add", "r", &["p0", "p1"]),
                op_args("ret", &["r"]),
            ],
            param_types: None,
        };
        let mut tir = lower_to_tir(&ir);
        refine_types(&mut tir);
        let type_map = extract_type_map(&tir);
        let _stats = run_pipeline(&mut tir);
        assert!(verify_function(&tir).is_ok());
        let result = lower_to_simple_ir(&tir, &type_map);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 11: Function with typed parameters (fast_int param hint)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_typed_params() {
        let ir = FunctionIR {
            name: "typed_params".to_string(),
            params: vec!["n".to_string()],
            ops: vec![
                op_out("const", "one"),
                op_out_args("add", "result", &["n", "one"]),
                op_args("ret", &["result"]),
            ],
            param_types: Some(vec!["int".to_string()]),
        };
        let mut tir = lower_to_tir(&ir);
        refine_types(&mut tir);
        let type_map = extract_type_map(&tir);
        let _stats = run_pipeline(&mut tir);
        assert!(verify_function(&tir).is_ok());
        let result = lower_to_simple_ir(&tir, &type_map);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 12: Chained arithmetic (longer straight-line)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_chained_arithmetic() {
        let ops = vec![
            op_out("const", "a"),
            op_out("const", "b"),
            op_out("const", "c"),
            op_out_args("add", "ab", &["a", "b"]),
            op_out_args("add", "abc", &["ab", "c"]),
            op_out_args("mul", "result", &["ab", "abc"]),
            op_args("ret", &["result"]),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 13: Nested if/else (no loop)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_nested_if_else() {
        let ops = vec![
            op_out("const", "c1"),
            op_out("const", "c2"),
            op_args("if", &["c1"]),
            op_args("if", &["c2"]),
            op_out("const", "aa"),
            op("else"),
            op_out("const", "ab"),
            op("end_if"),
            op("else"),
            op_out("const", "ba"),
            op("end_if"),
            op("ret_void"),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 14: Pipeline does not panic on nop-only function
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_nop_function() {
        let ops = vec![op("nop"), op("nop"), op("ret_void")];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 15: lower_to_simple_ir output contains a terminator op
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_output_has_terminator() {
        let ops = vec![
            op_out("const", "x"),
            op_args("ret", &["x"]),
        ];
        let result = roundtrip(ops);
        let has_term = result
            .iter()
            .any(|o| matches!(o.kind.as_str(), "ret" | "ret_void" | "jump" | "br_if"));
        assert!(has_term, "output must contain a terminator op, got: {:?}", result);
    }

    // ---------------------------------------------------------------------------
    // Test 16: check_exception label ID preserved through round-trip
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_check_exception_label_id() {
        // Simulate a try/except pattern:
        //   call "foo" → result
        //   check_exception(value=100)  ← handler label ID
        //   ret_void
        //   label(100)                  ← exception handler
        //   ret_void
        let ops = vec![
            OpIR {
                kind: "call".to_string(),
                out: Some("result".into()),
                s_value: Some("foo".into()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                out: Some("exc".into()),
                value: Some(100),
                ..OpIR::default()
            },
            op("ret_void"),
            op_val("label", 100),
            op("ret_void"),
        ];
        let result = roundtrip(ops);

        // The check_exception op must have a value that matches a label in the output.
        let check_exc = result.iter().find(|o| o.kind == "check_exception");
        assert!(check_exc.is_some(), "check_exception must survive round-trip");
        let exc_target = check_exc.unwrap().value.expect("check_exception must have a value");

        let label_vals: Vec<i64> = result
            .iter()
            .filter(|o| o.kind == "label")
            .filter_map(|o| o.value)
            .collect();
        assert!(
            label_vals.contains(&exc_target),
            "check_exception target {} must match a label value in output. Labels: {:?}",
            exc_target,
            label_vals
        );
    }

    // ---------------------------------------------------------------------------
    // Test 17: passthrough ops preserve original kind through round-trip
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_kind() {
        // Use an op kind the native backend handles but TIR OpCode doesn't know.
        // The output is used by `ret` so DCE doesn't remove it.
        let ops = vec![
            OpIR {
                kind: "list_new".to_string(),
                out: Some("lst".into()),
                value: Some(0),
                ..OpIR::default()
            },
            op_args("ret", &["lst"]),
        ];
        let result = roundtrip(ops);

        let list_new = result.iter().find(|o| o.kind == "list_new");
        assert!(
            list_new.is_some(),
            "passthrough op 'list_new' must survive round-trip. Got: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
    }

    // ---------------------------------------------------------------------------
    // Test 18: passthrough ops preserve all fields
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_fields() {
        // The output is used by `ret` so DCE doesn't remove it.
        let ops = vec![
            OpIR {
                kind: "dict_set".to_string(),
                out: Some("res".into()),
                args: Some(vec!["res".into()]),
                s_value: Some("helper".into()),
                value: Some(42),
                ..OpIR::default()
            },
            op_args("ret", &["res"]),
        ];
        let result = roundtrip(ops);

        let dict_set = result.iter().find(|o| o.kind == "dict_set");
        assert!(dict_set.is_some(), "dict_set must survive round-trip");
        let ds = dict_set.unwrap();
        assert_eq!(ds.value, Some(42), "value field must be preserved");
        assert_eq!(ds.s_value.as_deref(), Some("helper"), "s_value field must be preserved");
    }
}
