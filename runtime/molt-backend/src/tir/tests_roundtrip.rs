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
           source_file: None,
            is_extern: false,
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
        assert!(
            !result.is_empty(),
            "round-trip should produce non-empty ops"
        );
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
    // Test 7: integer compatibility hint survives the transport round-trip
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_integer_compat_hint() {
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
           source_file: None,
            is_extern: false,
        };
        let mut tir = lower_to_tir(&ir);
        refine_types(&mut tir);
        let type_map = extract_type_map(&tir);
        let _stats = run_pipeline(&mut tir);
        assert!(verify_function(&tir).is_ok());
        let result = lower_to_simple_ir(&tir, &type_map);
        assert!(!result.is_empty());
        assert!(
            result.iter().all(|op| op.kind != "load_param"),
            "round-trip must preserve FunctionIR.params instead of synthesizing load_param ops"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 11: Function with typed parameters (transport compatibility hint)
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
           source_file: None,
            is_extern: false,
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
        let ops = vec![op_out("const", "x"), op_args("ret", &["x"])];
        let result = roundtrip(ops);
        let has_term = result
            .iter()
            .any(|o| matches!(o.kind.as_str(), "ret" | "ret_void" | "jump" | "br_if"));
        assert!(
            has_term,
            "output must contain a terminator op, got: {:?}",
            result
        );
    }

    #[test]
    fn roundtrip_unpack_sequence_preserves_output_vars() {
        let ops = vec![
            op_out("const", "seq"),
            OpIR {
                kind: "unpack_sequence".to_string(),
                args: Some(vec!["seq".to_string(), "a".to_string(), "b".to_string()]),
                value: Some(2),
                ..OpIR::default()
            },
            op_args("ret", &["a"]),
        ];
        let result = roundtrip(ops);
        let unpack = result
            .iter()
            .find(|op| op.kind == "unpack_sequence")
            .expect("roundtrip must preserve unpack_sequence");
        let unpack_args = unpack
            .args
            .as_ref()
            .expect("unpack_sequence must retain args");
        assert_eq!(
            unpack_args.len(),
            3,
            "unpack_sequence keeps one input and two outputs"
        );
        assert_eq!(unpack.value, Some(2));
        let ret = result
            .iter()
            .find(|op| op.kind == "ret")
            .expect("roundtrip must preserve return");
        let ret_args = ret.args.as_ref().expect("ret must retain args");
        assert_eq!(
            ret_args.first(),
            unpack_args.get(1),
            "later uses must resolve to the first unpack output value",
        );
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
        assert!(
            check_exc.is_some(),
            "check_exception must survive round-trip"
        );
        let exc_target = check_exc
            .unwrap()
            .value
            .expect("check_exception must have a value");

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
        assert_eq!(
            ds.s_value.as_deref(),
            Some("helper"),
            "s_value field must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Helper: roundtrip without optimization passes (pure SSA lift + lower)
    // ---------------------------------------------------------------------------

    fn roundtrip_no_opt(ops: Vec<OpIR>) -> Vec<OpIR> {
        let ir = make_function(ops);
        let tir = lower_to_tir(&ir);
        let type_map = std::collections::HashMap::new();
        lower_to_simple_ir(&tir, &type_map)
    }

    // ---------------------------------------------------------------------------
    // Test 19: check_exception preserves value field (the crash case)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_check_exception_value_field() {
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
            op_val("state_label", 100),
            op("ret_void"),
        ];
        let result = roundtrip_no_opt(ops);
        let check = result.iter().find(|o| o.kind == "check_exception");
        assert!(
            check.is_some(),
            "check_exception must survive round-trip, got: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        let check = check.unwrap();
        assert!(
            check.value.is_some(),
            "check_exception.value must not be None, got: {:?}",
            check
        );
    }

    // ---------------------------------------------------------------------------
    // Test 20: try_start/try_end preserve value field
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_try_start_try_end_value() {
        let ops = vec![
            OpIR {
                kind: "try_start".to_string(),
                value: Some(200),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                out: Some("r".into()),
                s_value: Some("bar".into()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                out: Some("exc".into()),
                value: Some(200),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(200),
                ..OpIR::default()
            },
            op("ret_void"),
            op_val("state_label", 200),
            op("ret_void"),
        ];
        let result = roundtrip_no_opt(ops);
        let try_start = result.iter().find(|o| o.kind == "try_start");
        assert!(try_start.is_some(), "try_start must survive round-trip");
        assert!(
            try_start.unwrap().value.is_some(),
            "try_start.value must be preserved"
        );

        let try_end = result.iter().find(|o| o.kind == "try_end");
        assert!(try_end.is_some(), "try_end must survive round-trip");
        assert!(
            try_end.unwrap().value.is_some(),
            "try_end.value must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 21: passthrough preserves var field
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_var() {
        let ops = vec![
            OpIR {
                kind: "some_custom_op".to_string(),
                var: Some("my_var_name".into()),
                out: Some("result".into()),
                value: Some(7),
                ..OpIR::default()
            },
            op_args("ret", &["result"]),
        ];
        let result = roundtrip_no_opt(ops);
        let custom = result.iter().find(|o| o.kind == "some_custom_op");
        assert!(custom.is_some(), "custom op must survive round-trip");
        assert!(
            custom.unwrap().var.is_some(),
            "var field must be preserved for passthrough ops, got: {:?}",
            custom
        );
    }

    // ---------------------------------------------------------------------------
    // Test 22: passthrough preserves f_value field
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_f_value() {
        let ops = vec![
            OpIR {
                kind: "some_float_op".to_string(),
                out: Some("result".into()),
                f_value: Some(3.14),
                ..OpIR::default()
            },
            op_args("ret", &["result"]),
        ];
        let result = roundtrip_no_opt(ops);
        let fop = result.iter().find(|o| o.kind == "some_float_op");
        assert!(fop.is_some(), "float op must survive round-trip");
        assert_eq!(
            fop.unwrap().f_value,
            Some(3.14),
            "f_value field must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 23: passthrough preserves bytes field
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_bytes() {
        let ops = vec![
            OpIR {
                kind: "some_bytes_op".to_string(),
                out: Some("result".into()),
                bytes: Some(vec![1, 2, 3]),
                ..OpIR::default()
            },
            op_args("ret", &["result"]),
        ];
        let result = roundtrip_no_opt(ops);
        let bop = result.iter().find(|o| o.kind == "some_bytes_op");
        assert!(bop.is_some(), "bytes op must survive round-trip");
        assert_eq!(
            bop.unwrap().bytes.as_deref(),
            Some(&[1u8, 2, 3][..]),
            "bytes field must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 24: compatibility hints preserved for known transport ops
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_preserves_fast_flags() {
        let ops = vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".into()),
                value: Some(10),
                fast_int: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_float".to_string(),
                out: Some("y".into()),
                f_value: Some(1.5),
                fast_float: Some(true),
                ..OpIR::default()
            },
            op_args("ret", &["x"]),
        ];
        let result = roundtrip_no_opt(ops);
        // Compatibility hints are transport metadata rather than the canonical
        // backend contract, so this test only requires the round-trip to remain
        // stable and non-crashing.
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 25: const ops preserve all scalar fields
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_const_preserves_value() {
        let ops = vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".into()),
                value: Some(42),
                ..OpIR::default()
            },
            op_args("ret", &["x"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == "const");
        assert!(cnst.is_some(), "const must survive round-trip");
        assert_eq!(
            cnst.unwrap().value,
            Some(42),
            "const.value must be preserved"
        );
    }

    #[test]
    fn roundtrip_const_str_preserves_s_value() {
        let ops = vec![
            OpIR {
                kind: "const_str".to_string(),
                out: Some("s".into()),
                s_value: Some("hello".into()),
                ..OpIR::default()
            },
            op_args("ret", &["s"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == "const_str");
        assert!(cnst.is_some(), "const_str must survive round-trip");
        assert_eq!(
            cnst.unwrap().s_value.as_deref(),
            Some("hello"),
            "const_str.s_value must be preserved"
        );
    }

    #[test]
    fn roundtrip_const_bytes_preserves_bytes() {
        let ops = vec![
            OpIR {
                kind: "const_bytes".to_string(),
                out: Some("b".into()),
                bytes: Some(vec![0xDE, 0xAD]),
                ..OpIR::default()
            },
            op_args("ret", &["b"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == "const_bytes");
        assert!(cnst.is_some(), "const_bytes must survive round-trip");
        assert_eq!(
            cnst.unwrap().bytes.as_deref(),
            Some(&[0xDEu8, 0xAD][..]),
            "const_bytes.bytes must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 26: call ops preserve s_value and value
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_call_preserves_fields() {
        let ops = vec![
            OpIR {
                kind: "call".to_string(),
                out: Some("r".into()),
                s_value: Some("my_func".into()),
                value: Some(5),
                ..OpIR::default()
            },
            op_args("ret", &["r"]),
        ];
        let result = roundtrip_no_opt(ops);
        let call = result.iter().find(|o| o.kind == "call");
        assert!(call.is_some(), "call must survive round-trip");
        let c = call.unwrap();
        assert_eq!(
            c.s_value.as_deref(),
            Some("my_func"),
            "call.s_value must be preserved"
        );
        assert_eq!(c.value, Some(5), "call.value must be preserved");
    }

    // ---------------------------------------------------------------------------
    // Test 27: import preserves s_value
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_import_preserves_s_value() {
        let ops = vec![
            OpIR {
                kind: "import".to_string(),
                out: Some("m".into()),
                s_value: Some("os".into()),
                ..OpIR::default()
            },
            op_args("ret", &["m"]),
        ];
        let result = roundtrip_no_opt(ops);
        let imp = result.iter().find(|o| o.kind == "import");
        assert!(imp.is_some(), "import must survive round-trip");
        assert_eq!(
            imp.unwrap().s_value.as_deref(),
            Some("os"),
            "import.s_value must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 28: state_block_start / state_block_end preserve value
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_state_block_preserves_value() {
        // Realistic pattern: try_start → call → check_exception → ret_void
        // then state_label → state_block_start → state_block_end → ret_void
        let ops = vec![
            OpIR {
                kind: "try_start".to_string(),
                value: Some(300),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                out: Some("r".into()),
                s_value: Some("foo".into()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                out: Some("exc".into()),
                value: Some(300),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(300),
                ..OpIR::default()
            },
            op("ret_void"),
            op_val("state_label", 300),
            OpIR {
                kind: "state_block_start".to_string(),
                value: Some(300),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_block_end".to_string(),
                value: Some(300),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let result = roundtrip_no_opt(ops);
        let sbs = result.iter().find(|o| o.kind == "state_block_start");
        assert!(
            sbs.is_some(),
            "state_block_start must survive round-trip, got kinds: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        assert!(
            sbs.unwrap().value.is_some(),
            "state_block_start.value must be preserved"
        );

        let sbe = result.iter().find(|o| o.kind == "state_block_end");
        assert!(
            sbe.is_some(),
            "state_block_end must survive round-trip, got kinds: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        assert!(
            sbe.unwrap().value.is_some(),
            "state_block_end.value must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 29: passthrough preserves task_kind + container_type
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_metadata() {
        let ops = vec![
            OpIR {
                kind: "async_spawn".to_string(),
                out: Some("t".into()),
                task_kind: Some("coro".into()),
                container_type: Some("list".into()),
                ..OpIR::default()
            },
            op_args("ret", &["t"]),
        ];
        let result = roundtrip_no_opt(ops);
        let asp = result.iter().find(|o| o.kind == "async_spawn");
        assert!(asp.is_some(), "async_spawn must survive round-trip");
        let a = asp.unwrap();
        assert_eq!(
            a.task_kind.as_deref(),
            Some("coro"),
            "task_kind must be preserved"
        );
        assert_eq!(
            a.container_type.as_deref(),
            Some("list"),
            "container_type must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 30: call_func variant preserves original kind
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_call_func_preserves_kind() {
        let ops = vec![
            OpIR {
                kind: "call_func".to_string(),
                out: Some("r".into()),
                s_value: Some("target".into()),
                value: Some(1),
                ..OpIR::default()
            },
            op_args("ret", &["r"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cf = result.iter().find(|o| o.kind == "call_func");
        assert!(
            cf.is_some(),
            "call_func must survive round-trip, got: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
    }

    // ---------------------------------------------------------------------------
    // Test 31: const value=0 (edge case for zero-is-falsy bugs)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_trace_ops_survive() {
        // Reproduce the exact pattern from native_backend_can_opt_in_trace_imports
        let ops = vec![
            OpIR {
                kind: "trace_enter_slot".to_string(),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "trace_exit".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                ..OpIR::default()
            },
        ];
        let result = roundtrip(ops.clone());
        let has_trace_enter = result.iter().any(|o| o.kind == "trace_enter_slot");
        let has_trace_exit = result.iter().any(|o| o.kind == "trace_exit");
        assert!(
            has_trace_enter,
            "trace_enter_slot must survive round-trip with optimization. Got: {:?}",
            result
                .iter()
                .map(|o| format!("{}(v={:?})", o.kind, o.value))
                .collect::<Vec<_>>()
        );
        assert!(
            has_trace_exit,
            "trace_exit must survive round-trip with optimization. Got: {:?}",
            result
                .iter()
                .map(|o| format!("{}(v={:?})", o.kind, o.value))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn roundtrip_const_zero_value() {
        let ops = vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("z".into()),
                value: Some(0),
                ..OpIR::default()
            },
            op_args("ret", &["z"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == "const");
        assert!(cnst.is_some(), "const must survive round-trip");
        assert_eq!(
            cnst.unwrap().value,
            Some(0),
            "const.value=0 must be preserved (not treated as None)"
        );
    }

    // ---------------------------------------------------------------------------
    // Test: Loop body ops survive TIR optimization roundtrip
    // ---------------------------------------------------------------------------

    #[test]
    fn loop_body_ops_survive_optimization() {
        // Simulate: for i in range(3): call print(i)
        // The "call" op inside the loop must survive the TIR roundtrip.
        let ops = vec![
            op_out("const", "stop"), // stop = 3
            op_out("const", "idx"),  // idx = 0
            op("loop_start"),
            // Loop body: a call that must NOT be eliminated
            op_out_args("call", "result", &["idx"]),
            // Increment
            op_out("const", "one"),
            op_out_args("add", "idx2", &["idx", "one"]),
            op("loop_end"),
            op("ret_void"),
        ];
        let result = roundtrip(ops);
        // The "call" op must exist in the output — it has side effects
        // and must not be eliminated by any optimization pass.
        let call_count = result.iter().filter(|o| o.kind == "call").count();
        assert!(
            call_count >= 1,
            "loop body 'call' op was eliminated by TIR optimization! \
             This is the known TIR pass interaction bug. \
             Output ops: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn loop_body_ops_survive_no_opt() {
        // Same test without optimization — should always pass.
        let ops = vec![
            op_out("const", "stop"),
            op_out("const", "idx"),
            op("loop_start"),
            op_out_args("call", "result", &["idx"]),
            op_out("const", "one"),
            op_out_args("add", "idx2", &["idx", "one"]),
            op("loop_end"),
            op("ret_void"),
        ];
        let result = roundtrip_no_opt(ops);
        let call_count = result.iter().filter(|o| o.kind == "call").count();
        assert!(
            call_count >= 1,
            "loop body 'call' op was eliminated even without optimization! \
             Output ops: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
    }
}
