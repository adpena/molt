//! Round-trip stress tests for TIR pipeline.
//! Verifies that SimpleIR → TIR → optimize → SimpleIR produces valid output.

#[cfg(test)]
mod tests {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::{lower_to_tir, lower_to_tir_for_target};
    use crate::tir::lower_to_simple::lower_to_simple_ir;

    use crate::tir::passes::run_pipeline;
    use crate::tir::target_info::TargetInfo;
    use crate::tir::type_refine::refine_types;
    use crate::tir::verify::verify_function;
    use std::f64::consts::PI;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn make_function(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "test".to_string(),
            params: vec![],
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }
    }

    fn roundtrip(ops: Vec<OpIR>) -> Vec<OpIR> {
        roundtrip_function(make_function(ops))
    }

    fn roundtrip_function(ir: FunctionIR) -> Vec<OpIR> {
        let target = TargetInfo::native_release_fast();
        let mut tir = lower_to_tir_for_target(&ir, &target);
        refine_types(&mut tir);
        let _stats = run_pipeline(&mut tir, &target);
        assert!(
            verify_function(&tir).is_ok(),
            "TIR verification failed after optimization"
        );
        lower_to_simple_ir(&tir)
    }

    fn op(kind: molt_ir::OpKind) -> OpIR {
        OpIR {
            kind: kind,
            ..OpIR::new(kind)
        }
    }

    fn op_val(kind: molt_ir::OpKind, value: i64) -> OpIR {
        OpIR {
            kind: kind,
            value: Some(value),
            ..OpIR::new(kind)
        }
    }

    fn op_out(kind: molt_ir::OpKind, out: &str) -> OpIR {
        OpIR {
            kind: kind,
            out: Some(out.to_string()),
            ..OpIR::new(kind)
        }
    }

    fn int_literal(out: &str, value: i64) -> OpIR {
        OpIR {
            value: Some(value),
            ..op_out(molt_ir::OpKind::Const, out)
        }
    }

    fn str_literal(out: &str, value: &str) -> OpIR {
        OpIR {
            s_value: Some(value.to_string()),
            ..op_out(molt_ir::OpKind::ConstStr, out)
        }
    }

    fn bool_literal(out: &str, value: bool) -> OpIR {
        OpIR {
            value: Some(i64::from(value)),
            ..op_out(molt_ir::OpKind::ConstBool, out)
        }
    }

    fn roundtrip_parameterized(ops: Vec<OpIR>, params: &[&str]) -> Vec<OpIR> {
        let mut function = make_function(ops);
        function.params = params.iter().map(|name| (*name).to_string()).collect();
        roundtrip_function(function)
    }

    fn op_args(kind: molt_ir::OpKind, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind,
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::new(kind)
        }
    }

    fn op_out_args(kind: molt_ir::OpKind, out: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind,
            out: Some(out.to_string()),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::new(kind)
        }
    }

    /// A side-effecting direct call; the TIR verifier requires a named callee.
    fn call_out(out: &str, callee: &str, args: &[&str]) -> OpIR {
        OpIR {
            s_value: Some(callee.to_string()),
            ..op_out_args(molt_ir::OpKind::Call, out, args)
        }
    }

    // ---------------------------------------------------------------------------
    // Test 1: Straight-line arithmetic
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_straight_line() {
        let ops = vec![
            int_literal("x", 7),
            int_literal("y", 11),
            op_out_args(molt_ir::OpKind::Add, "z", &["x", "y"]),
            op_args(molt_ir::OpKind::Ret, &["z"]),
        ];
        let result = roundtrip(ops);
        assert!(
            !result.is_empty(),
            "round-trip should produce non-empty ops"
        );
    }

    #[test]
    fn roundtrip_preserves_source_site_coordinates() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Line,
                value: Some(12),
                source_line: Some(12),
                col_offset: Some(4),
                end_col_offset: Some(9),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::Const,
                value: Some(7),
                out: Some("x".to_string()),
                source_op_idx: Some(42),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            op_args(molt_ir::OpKind::Ret, &["x"]),
        ];
        let result = roundtrip(ops);
        assert!(
            result.iter().any(|op| {
                op.source_line == Some(12)
                    && op.col_offset == Some(4)
                    && op.end_col_offset == Some(9)
            }),
            "SimpleIR -> TIR -> SimpleIR must preserve source-site coordinates"
        );
        assert!(
            result.iter().any(|op| op.source_op_idx == Some(42)),
            "SimpleIR -> TIR -> SimpleIR must preserve transported source op identity"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 2: If/else
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_if_else() {
        let ops = vec![
            op_args(molt_ir::OpKind::If, &["cond"]),
            int_literal("a", 2),
            op(molt_ir::OpKind::Else),
            int_literal("b", 3),
            op(molt_ir::OpKind::EndIf),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_parameterized(ops, &["cond"]);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 3: Simple loop
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_loop() {
        let ops = vec![
            int_literal("i", 0),
            op(molt_ir::OpKind::LoopStart),
            op_out_args(molt_ir::OpKind::Add, "i2", &["i", "i"]),
            op(molt_ir::OpKind::LoopEnd),
            op(molt_ir::OpKind::RetVoid),
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
            int_literal("i", 0),
            op(molt_ir::OpKind::LoopStart),
            op_args(molt_ir::OpKind::If, &["cond"]),
            op_out_args(molt_ir::OpKind::Add, "i2", &["i", "i"]),
            op(molt_ir::OpKind::EndIf),
            op(molt_ir::OpKind::LoopEnd),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_parameterized(ops, &["cond"]);
        assert!(!result.is_empty());
    }

    #[test]
    fn roundtrip_iter_loop_with_inner_return_keeps_post_loop_control() {
        let ops = vec![
            op_out(molt_ir::OpKind::Missing, "v86"),
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some("v".to_string()),
                args: Some(vec!["v86".to_string()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
            OpIR {
                kind: molt_ir::OpKind::Iter,
                args: Some(vec!["xs".to_string()]),
                out: Some("it".to_string()),
                ..OpIR::new(molt_ir::OpKind::Iter)
            },
            op_out(molt_ir::OpKind::ConstNone, "none0"),
            OpIR {
                kind: molt_ir::OpKind::Is,
                args: Some(vec!["it".to_string(), "none0".to_string()]),
                out: Some("not_iterable".to_string()),
                ..OpIR::new(molt_ir::OpKind::Is)
            },
            op_args(molt_ir::OpKind::If, &["not_iterable"]),
            str_literal("msg", "not iterable"),
            str_literal("etype", "TypeError"),
            op_out_args(molt_ir::OpKind::TupleNew, "emsg", &["msg"]),
            op_out_args(molt_ir::OpKind::ExceptionNew, "exc", &["etype", "emsg"]),
            op_args(molt_ir::OpKind::Raise, &["exc"]),
            op(molt_ir::OpKind::EndIf),
            int_literal("idx0", 0),
            int_literal("idx1", 1),
            bool_literal("ret_true", true),
            op(molt_ir::OpKind::LoopStart),
            OpIR {
                kind: molt_ir::OpKind::IterNext,
                args: Some(vec!["it".to_string()]),
                out: Some("pair".to_string()),
                ..OpIR::new(molt_ir::OpKind::IterNext)
            },
            op_out_args(molt_ir::OpKind::Index, "done", &["pair", "idx1"]),
            op_args(molt_ir::OpKind::LoopBreakIfTrue, &["done"]),
            op_out_args(molt_ir::OpKind::Index, "elem", &["pair", "idx0"]),
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some("v".to_string()),
                args: Some(vec!["elem".to_string()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
            OpIR {
                kind: molt_ir::OpKind::LoadVar,
                var: Some("v".to_string()),
                out: Some("cur".to_string()),
                ..OpIR::new(molt_ir::OpKind::LoadVar)
            },
            op_out_args(molt_ir::OpKind::Eq, "cmp", &["cur", "value"]),
            op_args(molt_ir::OpKind::If, &["cmp"]),
            OpIR {
                kind: molt_ir::OpKind::Ret,
                args: Some(vec!["ret_true".to_string()]),
                ..OpIR::new(molt_ir::OpKind::Ret)
            },
            op(molt_ir::OpKind::EndIf),
            op(molt_ir::OpKind::LoopContinue),
            op(molt_ir::OpKind::LoopEnd),
            bool_literal("ret_false", false),
            OpIR {
                kind: molt_ir::OpKind::Ret,
                args: Some(vec!["ret_false".to_string()]),
                ..OpIR::new(molt_ir::OpKind::Ret)
            },
        ];
        let ir = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "loop_if_return_continue_roundtrip".to_string(),
            params: vec!["xs".to_string(), "value".to_string()],
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };

        let target = TargetInfo::native_release_fast();
        let mut tir = lower_to_tir_for_target(&ir, &target);
        refine_types(&mut tir);
        let _stats = run_pipeline(&mut tir, &target);
        assert!(verify_function(&tir).is_ok(), "TIR verification failed");
        let result = lower_to_simple_ir(&tir);

        for expected in ["ret_true", "ret_false"] {
            assert!(
                result.iter().any(|op| {
                    op.kind == molt_ir::OpKind::Ret
                        && op.args.as_ref().is_some_and(|args| args == &[expected])
                }),
                "named return path {expected} must survive roundtrip: {result:?}"
            );
        }
        assert!(
            result
                .iter()
                .any(|op| op.kind == molt_ir::OpKind::AsyncWorkPoll),
            "loop backedges must retain the canonical asynchronous-work observation: {result:?}"
        );
        assert!(
            result.iter().any(|op| op.kind == molt_ir::OpKind::LoopEnd),
            "loop must keep its loop_end marker: {result:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 5: Multiple return paths
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_multiple_returns() {
        let ops = vec![
            op_args(molt_ir::OpKind::If, &["cond"]),
            int_literal("a", 2),
            op_args(molt_ir::OpKind::Ret, &["a"]),
            op(molt_ir::OpKind::Else),
            int_literal("b", 3),
            op_args(molt_ir::OpKind::Ret, &["b"]),
            op(molt_ir::OpKind::EndIf),
        ];
        let result = roundtrip_parameterized(ops, &["cond"]);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 6: Empty function (only ret_void)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_empty() {
        let ops = vec![op(molt_ir::OpKind::RetVoid)];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
        assert!(
            result.iter().any(|o| o.kind == molt_ir::OpKind::RetVoid),
            "empty function must emit ret_void"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 7: integer compatibility hint survives the transport round-trip
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_integer_compat_hint() {
        let add_op = op_out_args(molt_ir::OpKind::Add, "z", &["x", "y"]);
        let ops = vec![
            int_literal("x", 7),
            int_literal("y", 11),
            add_op,
            op_args(molt_ir::OpKind::Ret, &["z"]),
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
            int_literal("x", 7),
            op_val(molt_ir::OpKind::Jump, 1),
            int_literal("dead", 99),
            op_val(molt_ir::OpKind::Label, 1),
            op_args(molt_ir::OpKind::Ret, &["x"]),
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
            int_literal("x", 7),
            op_out_args(molt_ir::OpKind::Add, "y", &["x", "x"]),
            op_args(molt_ir::OpKind::Ret, &["y"]),
        ];
        let ir = make_function(ops);
        let target = TargetInfo::native_release_fast();
        let mut tir = lower_to_tir_for_target(&ir, &target);
        refine_types(&mut tir);
        run_pipeline(&mut tir, &target);
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
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "with_params".to_string(),
            params: vec!["p0".to_string(), "p1".to_string()],
            ops: vec![
                op_out_args(molt_ir::OpKind::Add, "r", &["p0", "p1"]),
                op_args(molt_ir::OpKind::Ret, &["r"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let target = TargetInfo::native_release_fast();
        let mut tir = lower_to_tir_for_target(&ir, &target);
        refine_types(&mut tir);
        let _stats = run_pipeline(&mut tir, &target);
        assert!(verify_function(&tir).is_ok());
        let result = lower_to_simple_ir(&tir);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 11: Function with typed parameters (transport compatibility hint)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_typed_params() {
        let ir = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "typed_params".to_string(),
            params: vec!["n".to_string()],
            ops: vec![
                int_literal("one", 1),
                op_out_args(molt_ir::OpKind::Add, "result", &["n", "one"]),
                op_args(molt_ir::OpKind::Ret, &["result"]),
            ],
            param_types: Some(vec!["int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let target = TargetInfo::native_release_fast();
        let mut tir = lower_to_tir_for_target(&ir, &target);
        refine_types(&mut tir);
        let _stats = run_pipeline(&mut tir, &target);
        assert!(verify_function(&tir).is_ok());
        let result = lower_to_simple_ir(&tir);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 12: Chained arithmetic (longer straight-line)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_chained_arithmetic() {
        let ops = vec![
            int_literal("a", 2),
            int_literal("b", 3),
            int_literal("c", 5),
            op_out_args(molt_ir::OpKind::Add, "ab", &["a", "b"]),
            op_out_args(molt_ir::OpKind::Add, "abc", &["ab", "c"]),
            op_out_args(molt_ir::OpKind::Mul, "result", &["ab", "abc"]),
            op_args(molt_ir::OpKind::Ret, &["result"]),
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
            op_args(molt_ir::OpKind::If, &["c1"]),
            op_args(molt_ir::OpKind::If, &["c2"]),
            int_literal("aa", 2),
            op(molt_ir::OpKind::Else),
            int_literal("ab", 3),
            op(molt_ir::OpKind::EndIf),
            op(molt_ir::OpKind::Else),
            int_literal("ba", 5),
            op(molt_ir::OpKind::EndIf),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_parameterized(ops, &["c1", "c2"]);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 14: Pipeline does not panic on nop-only function
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_nop_function() {
        let ops = vec![
            op(molt_ir::OpKind::Nop),
            op(molt_ir::OpKind::Nop),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip(ops);
        assert!(!result.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Test 15: lower_to_simple_ir output contains a terminator op
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_output_has_terminator() {
        let ops = vec![int_literal("x", 7), op_args(molt_ir::OpKind::Ret, &["x"])];
        let result = roundtrip(ops);
        let has_term = result.iter().any(|op| {
            op.kind.is_return_terminator()
                || matches!(op.kind, molt_ir::OpKind::Jump | molt_ir::OpKind::BrIf)
        });
        assert!(
            has_term,
            "output must contain a terminator op, got: {:?}",
            result
        );
    }

    #[test]
    fn roundtrip_unpack_sequence_preserves_output_vars() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::UnpackSequence,
                args: Some(vec!["seq".to_string(), "a".to_string(), "b".to_string()]),
                value: Some(2),
                ..OpIR::new(molt_ir::OpKind::UnpackSequence)
            },
            op_args(molt_ir::OpKind::Ret, &["a"]),
        ];
        let result = roundtrip_parameterized(ops, &["seq"]);
        let unpack = result
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::UnpackSequence)
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
            .find(|op| op.kind == molt_ir::OpKind::Ret)
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
    fn roundtrip_exception_transfer_label_id() {
        // Simulate a try/except pattern:
        //   call "foo" → result
        //   check_exception(value=100)  ← handler label ID
        //   ret_void
        //   label(100)                  ← exception handler
        //   ret_void
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Call,
                out: Some("result".into()),
                s_value: Some("foo".into()),
                ..OpIR::new(molt_ir::OpKind::Call)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                out: Some("exc".into()),
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            op(molt_ir::OpKind::RetVoid),
            op_val(molt_ir::OpKind::Label, 100),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip(ops);

        // The call-return observation is promoted to the canonical asynchronous-work
        // spelling, and its exceptional target must still match an emitted label.
        let check_exc = result
            .iter()
            .find(|o| o.kind == molt_ir::OpKind::AsyncWorkPoll);
        assert!(
            check_exc.is_some(),
            "the call-return asynchronous-work observation must survive round-trip"
        );
        let exc_target = check_exc
            .unwrap()
            .value
            .expect("check_exception must have a value");

        let label_vals: Vec<i64> = result
            .iter()
            .filter(|o| o.kind == molt_ir::OpKind::Label)
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
                kind: molt_ir::OpKind::ListNew,
                out: Some("lst".into()),
                value: Some(0),
                ..OpIR::new(molt_ir::OpKind::ListNew)
            },
            op_args(molt_ir::OpKind::Ret, &["lst"]),
        ];
        let result = roundtrip(ops);

        let list_new = result.iter().find(|o| o.kind == molt_ir::OpKind::ListNew);
        assert!(
            list_new.is_some(),
            "passthrough op 'list_new' must survive round-trip. Got: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn roundtrip_ord_at_preserves_first_class_kind() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("Aé".into()),
                out: Some("text".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::Const,
                value: Some(1),
                out: Some("idx".into()),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            op_out_args(molt_ir::OpKind::OrdAt, "code", &["text", "idx"]),
            op_args(molt_ir::OpKind::Ret, &["code"]),
        ];
        let result = roundtrip(ops);

        let ord_at = result.iter().find(|o| o.kind == molt_ir::OpKind::OrdAt);
        assert!(
            ord_at.is_some(),
            "first-class ord_at must survive TIR round-trip. Got: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        assert_eq!(
            ord_at.and_then(|op| op.args.as_ref()).cloned(),
            Some(vec!["text".to_string(), "idx".to_string()])
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
                kind: molt_ir::OpKind::DictSet,
                out: Some("res".into()),
                args: Some(vec!["res".into()]),
                s_value: Some("helper".into()),
                value: Some(42),
                ..OpIR::new(molt_ir::OpKind::DictSet)
            },
            op_args(molt_ir::OpKind::Ret, &["res"]),
        ];
        let result = roundtrip(ops);

        let dict_set = result.iter().find(|o| o.kind == molt_ir::OpKind::DictSet);
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
        lower_to_simple_ir(&tir)
    }

    // ---------------------------------------------------------------------------
    // Test 19: check_exception preserves value field (the crash case)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_check_exception_value_field() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Call,
                out: Some("result".into()),
                s_value: Some("foo".into()),
                ..OpIR::new(molt_ir::OpKind::Call)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                out: Some("exc".into()),
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            op(molt_ir::OpKind::RetVoid),
            op_val(molt_ir::OpKind::StateLabel, 100),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_no_opt(ops);
        let check = result
            .iter()
            .find(|o| o.kind == molt_ir::OpKind::CheckException);
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

    #[test]
    fn check_exception_and_try_markers_are_not_structural() {
        // Exception-handling ops must NOT be structural — they carry semantics
        // that must be preserved as TirOps through SSA conversion.  Marking
        // them structural causes SSA to silently drop them.
        assert!(!(molt_ir::OpKind::CheckException).is_structural());
        assert!(!(molt_ir::OpKind::TryStart).is_structural());
        assert!(!(molt_ir::OpKind::TryEnd).is_structural());
        assert!(!(molt_ir::OpKind::StateBlockStart).is_structural());
        assert!(!(molt_ir::OpKind::StateBlockEnd).is_structural());
    }

    // ---------------------------------------------------------------------------
    // Test 20: try_start/try_end preserve value field
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_try_start_try_end_value() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::TryStart,
                value: Some(200),
                ..OpIR::new(molt_ir::OpKind::TryStart)
            },
            OpIR {
                kind: molt_ir::OpKind::Call,
                out: Some("r".into()),
                s_value: Some("bar".into()),
                ..OpIR::new(molt_ir::OpKind::Call)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                out: Some("exc".into()),
                value: Some(200),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::TryEnd,
                value: Some(200),
                ..OpIR::new(molt_ir::OpKind::TryEnd)
            },
            op(molt_ir::OpKind::RetVoid),
            op_val(molt_ir::OpKind::StateLabel, 200),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_no_opt(ops);
        let try_start = result.iter().find(|o| o.kind == molt_ir::OpKind::TryStart);
        assert!(try_start.is_some(), "try_start must survive round-trip");
        assert!(
            try_start.unwrap().value.is_some(),
            "try_start.value must be preserved"
        );

        let try_end = result.iter().find(|o| o.kind == molt_ir::OpKind::TryEnd);
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
                kind: molt_ir::OpKind::Cast,
                var: Some("my_var_name".into()),
                out: Some("result".into()),
                value: Some(7),
                ..OpIR::new(molt_ir::OpKind::Cast)
            },
            op_args(molt_ir::OpKind::Ret, &["result"]),
        ];
        let result = roundtrip_no_opt(ops);
        let custom = result.iter().find(|o| o.kind == molt_ir::OpKind::Cast);
        assert!(custom.is_some(), "cast must survive round-trip");
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
                kind: molt_ir::OpKind::Cast,
                out: Some("result".into()),
                f_value: Some(PI),
                ..OpIR::new(molt_ir::OpKind::Cast)
            },
            op_args(molt_ir::OpKind::Ret, &["result"]),
        ];
        let result = roundtrip_no_opt(ops);
        let fop = result.iter().find(|o| o.kind == molt_ir::OpKind::Cast);
        assert!(fop.is_some(), "cast must survive round-trip");
        assert_eq!(
            fop.unwrap().f_value,
            Some(PI),
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
                kind: molt_ir::OpKind::Cast,
                out: Some("result".into()),
                bytes: Some(vec![1, 2, 3]),
                ..OpIR::new(molt_ir::OpKind::Cast)
            },
            op_args(molt_ir::OpKind::Ret, &["result"]),
        ];
        let result = roundtrip_no_opt(ops);
        let bop = result.iter().find(|o| o.kind == molt_ir::OpKind::Cast);
        assert!(bop.is_some(), "cast must survive round-trip");
        assert_eq!(
            bop.unwrap().bytes.as_deref(),
            Some(&[1u8, 2, 3][..]),
            "bytes field must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 25: const ops preserve all scalar fields
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_const_preserves_value() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Const,
                out: Some("x".into()),
                value: Some(42),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            op_args(molt_ir::OpKind::Ret, &["x"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == molt_ir::OpKind::Const);
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
                kind: molt_ir::OpKind::ConstStr,
                out: Some("s".into()),
                s_value: Some("hello".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            op_args(molt_ir::OpKind::Ret, &["s"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == molt_ir::OpKind::ConstStr);
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
                kind: molt_ir::OpKind::ConstBytes,
                out: Some("b".into()),
                bytes: Some(vec![0xDE, 0xAD]),
                ..OpIR::new(molt_ir::OpKind::ConstBytes)
            },
            op_args(molt_ir::OpKind::Ret, &["b"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result
            .iter()
            .find(|o| o.kind == molt_ir::OpKind::ConstBytes);
        assert!(cnst.is_some(), "const_bytes must survive round-trip");
        assert_eq!(
            cnst.unwrap().bytes.as_deref(),
            Some(&[0xDEu8, 0xAD][..]),
            "const_bytes.bytes must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 26: direct calls preserve their symbolic target and arguments
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_call_preserves_fields() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Call,
                out: Some("r".into()),
                s_value: Some("my_func".into()),
                args: Some(vec!["argument".into()]),
                ..OpIR::new(molt_ir::OpKind::Call)
            },
            op_args(molt_ir::OpKind::Ret, &["r"]),
        ];
        let result = roundtrip_no_opt(ops);
        let call = result.iter().find(|o| o.kind == molt_ir::OpKind::Call);
        assert!(call.is_some(), "call must survive round-trip");
        let c = call.unwrap();
        assert_eq!(
            c.s_value.as_deref(),
            Some("my_func"),
            "call.s_value must be preserved"
        );
        assert_eq!(c.args.as_ref().map(Vec::len), Some(1));
        assert!(
            c.value.is_none(),
            "direct calls do not carry numeric identities"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 27: import preserves s_value
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_import_preserves_s_value() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Import,
                out: Some("m".into()),
                s_value: Some("os".into()),
                ..OpIR::new(molt_ir::OpKind::Import)
            },
            op_args(molt_ir::OpKind::Ret, &["m"]),
        ];
        let result = roundtrip_no_opt(ops);
        let imp = result.iter().find(|o| o.kind == molt_ir::OpKind::Import);
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
                kind: molt_ir::OpKind::TryStart,
                value: Some(300),
                ..OpIR::new(molt_ir::OpKind::TryStart)
            },
            OpIR {
                kind: molt_ir::OpKind::Call,
                out: Some("r".into()),
                s_value: Some("foo".into()),
                ..OpIR::new(molt_ir::OpKind::Call)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                out: Some("exc".into()),
                value: Some(300),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::TryEnd,
                value: Some(300),
                ..OpIR::new(molt_ir::OpKind::TryEnd)
            },
            op(molt_ir::OpKind::RetVoid),
            op_val(molt_ir::OpKind::StateLabel, 300),
            OpIR {
                kind: molt_ir::OpKind::StateBlockStart,
                value: Some(300),
                ..OpIR::new(molt_ir::OpKind::StateBlockStart)
            },
            OpIR {
                kind: molt_ir::OpKind::StateBlockEnd,
                value: Some(300),
                ..OpIR::new(molt_ir::OpKind::StateBlockEnd)
            },
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_no_opt(ops);
        let sbs = result
            .iter()
            .find(|o| o.kind == molt_ir::OpKind::StateBlockStart);
        assert!(
            sbs.is_some(),
            "state_block_start must survive round-trip, got kinds: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        assert!(
            sbs.unwrap().value.is_some(),
            "state_block_start.value must be preserved"
        );

        let sbe = result
            .iter()
            .find(|o| o.kind == molt_ir::OpKind::StateBlockEnd);
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
    // Test 29: passthrough preserves task metadata + container_type
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_passthrough_preserves_metadata() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Cast,
                out: Some("t".into()),
                task_kind: Some("coro".into()),
                container_type: Some("list".into()),
                ..OpIR::new(molt_ir::OpKind::Cast)
            },
            op_args(molt_ir::OpKind::Ret, &["t"]),
        ];
        let result = roundtrip_no_opt(ops);
        let asp = result.iter().find(|o| o.kind == molt_ir::OpKind::Cast);
        assert!(asp.is_some(), "cast must survive round-trip");
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

    #[test]
    fn roundtrip_func_new_preserves_typed_task_constructor_metadata() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::FuncNew,
                s_value: Some("worker_poll".into()),
                value: Some(0),
                out: Some("callable".into()),
                task_kind: Some("async_generator".into()),
                task_closure_size: Some(48),
                ..OpIR::new(molt_ir::OpKind::FuncNew)
            },
            op_args(molt_ir::OpKind::Ret, &["callable"]),
        ];
        let result = roundtrip_no_opt(ops);
        let constructor = result
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::FuncNew)
            .expect("func_new must survive round-trip");
        assert_eq!(constructor.task_kind.as_deref(), Some("async_generator"));
        assert_eq!(constructor.task_closure_size, Some(48));
    }

    #[test]
    fn roundtrip_invoke_ffi_preserves_native_callable_metadata() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::InvokeFfi,
                args: Some(vec!["arg".into()]),
                out: Some("out".into()),
                native_callable_export: Some("nativepkg.ndimage.distance_transform_edt".into()),
                native_callable_binding: Some("direct_symbol".into()),
                native_callable_symbol: Some(
                    "molt_nativepkg_ndimage_distance_transform_edt".into(),
                ),
                native_callable_abi: Some("molt.object_call_v1".into()),
                ..OpIR::new(molt_ir::OpKind::InvokeFfi)
            },
            op_args(molt_ir::OpKind::Ret, &["out"]),
        ];
        let result = roundtrip_no_opt(ops);
        let invoke = result
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::InvokeFfi)
            .expect("invoke_ffi must survive SimpleIR/TIR round-trip");
        assert_eq!(
            invoke.native_callable_export.as_deref(),
            Some("nativepkg.ndimage.distance_transform_edt")
        );
        assert_eq!(
            invoke.native_callable_binding.as_deref(),
            Some("direct_symbol")
        );
        assert_eq!(
            invoke.native_callable_symbol.as_deref(),
            Some("molt_nativepkg_ndimage_distance_transform_edt")
        );
        assert_eq!(
            invoke.native_callable_abi.as_deref(),
            Some("molt.object_call_v1")
        );
    }

    #[test]
    fn roundtrip_alloc_task_preserves_task_metadata() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::AllocTask,
                out: Some("task".into()),
                s_value: Some("__main_____f_poll".into()),
                value: Some(48),
                task_kind: Some("generator".into()),
                args: Some(vec![]),
                ..OpIR::new(molt_ir::OpKind::AllocTask)
            },
            op_args(molt_ir::OpKind::Ret, &["task"]),
        ];
        let result = roundtrip_no_opt(ops);
        let alloc = result.iter().find(|o| o.kind == molt_ir::OpKind::AllocTask);
        assert!(alloc.is_some(), "alloc_task must survive round-trip");
        let alloc = alloc.unwrap();
        assert_eq!(
            alloc.task_kind.as_deref(),
            Some("generator"),
            "alloc_task.task_kind must be preserved"
        );
        assert_eq!(
            alloc.s_value.as_deref(),
            Some("__main_____f_poll"),
            "alloc_task.s_value must be preserved"
        );
        assert_eq!(alloc.value, Some(48), "alloc_task.value must be preserved");
    }

    // ---------------------------------------------------------------------------
    // Phase 5: object_new_bound is a first-class TIR opcode (was Copy
    // fallback with `origin`).  Round-trip must preserve the
    // operand list, output value name, and the SimpleIR `type_hint`
    // field that carries the result's class id.
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_object_new_bound_preserves_class_type_hint() {
        // Note: SSA lift renames operand/result names to internal SSA
        // ids, so we can only check structural properties (kind,
        // arity, attrs that don't go through SSA renaming).  This
        // matches the convention used by other tests in this file
        // (see `roundtrip_alloc_task_preserves_task_metadata` which
        // checks task_kind / s_value / value, not names).
        let ops = vec![
            op_out_args(molt_ir::OpKind::ModuleGetAttr, "cls", &["mod", "name"]),
            OpIR {
                kind: molt_ir::OpKind::ObjectNewBound,
                args: Some(vec!["cls".to_string()]),
                out: Some("inst".into()),
                type_hint: Some("Point".into()),
                ..OpIR::new(molt_ir::OpKind::ObjectNewBound)
            },
            op_args(molt_ir::OpKind::Ret, &["inst"]),
        ];
        let result = roundtrip_no_opt(ops);
        let alloc = result
            .iter()
            .find(|o| o.kind == molt_ir::OpKind::ObjectNewBound)
            .expect("object_new_bound must survive round-trip");
        assert_eq!(
            alloc.args.as_ref().map(|v| v.len()),
            Some(1),
            "object_new_bound must have a single class-ref operand"
        );
        assert_eq!(
            alloc.type_hint.as_deref(),
            Some("Point"),
            "object_new_bound type_hint (class id) must round-trip"
        );
    }

    #[test]
    fn pipeline_alloc_task_preserves_task_metadata() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::AllocTask,
                out: Some("task".into()),
                s_value: Some("__main_____f_poll".into()),
                value: Some(48),
                task_kind: Some("generator".into()),
                args: Some(vec![]),
                ..OpIR::new(molt_ir::OpKind::AllocTask)
            },
            op_args(molt_ir::OpKind::Ret, &["task"]),
        ];
        let result = roundtrip(ops);
        let alloc = result.iter().find(|o| o.kind == molt_ir::OpKind::AllocTask);
        assert!(
            alloc.is_some(),
            "alloc_task must survive optimized round-trip"
        );
        let alloc = alloc.unwrap();
        assert_eq!(
            alloc.task_kind.as_deref(),
            Some("generator"),
            "optimized alloc_task.task_kind must be preserved"
        );
        assert_eq!(
            alloc.s_value.as_deref(),
            Some("__main_____f_poll"),
            "optimized alloc_task.s_value must be preserved"
        );
        assert_eq!(
            alloc.value,
            Some(48),
            "optimized alloc_task.value must be preserved"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 30: dynamic calls preserve their original kind and diagnostic ID
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_call_func_preserves_kind_and_diagnostic_id() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::CallFunc,
                out: Some("r".into()),
                args: Some(vec!["target".into()]),
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CallFunc)
            },
            op_args(molt_ir::OpKind::Ret, &["r"]),
        ];
        let mut source = make_function(ops);
        source.params = vec!["target".into()];
        let tir = lower_to_tir(&source);
        verify_function(&tir).expect("dynamic diagnostic IDs must survive TIR admission");
        let result = lower_to_simple_ir(&tir);
        let cf = result.iter().find(|o| o.kind == molt_ir::OpKind::CallFunc);
        assert!(
            cf.is_some(),
            "call_func must survive round-trip, got: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        assert_eq!(cf.unwrap().value, Some(1));
    }

    // ---------------------------------------------------------------------------
    // Test 31: const value=0 (edge case for zero-is-falsy bugs)
    // ---------------------------------------------------------------------------

    #[test]
    fn roundtrip_checked_frame_entry_preserves_guard_and_attempt_cleanup() {
        for (return_abi, value_payload) in [
            (molt_ir::FunctionReturnAbi::Void, false),
            (molt_ir::FunctionReturnAbi::Value, false),
            (molt_ir::FunctionReturnAbi::Value, true),
        ] {
            let ops = vec![
                OpIR {
                    kind: molt_ir::OpKind::TraceEnterSlot,
                    s_value: Some("test".into()),
                    ..OpIR::new(molt_ir::OpKind::TraceEnterSlot)
                },
                op_val(molt_ir::OpKind::CheckException, 19),
                op_args(molt_ir::OpKind::FrameLocalsSet, &["locals"]),
                OpIR {
                    kind: molt_ir::OpKind::TraceExit,
                    ..OpIR::new(molt_ir::OpKind::TraceExit)
                },
                OpIR {
                    kind: if value_payload {
                        molt_ir::OpKind::Ret
                    } else {
                        molt_ir::OpKind::RetVoid
                    },
                    args: value_payload.then(|| vec!["locals".into()]),
                    ..OpIR::new(if value_payload {
                        molt_ir::OpKind::Ret
                    } else {
                        molt_ir::OpKind::RetVoid
                    })
                },
                op_val(molt_ir::OpKind::Label, 19),
                op(molt_ir::OpKind::TraceExit),
                op(molt_ir::OpKind::RetVoid),
            ];
            let mut source = FunctionIR {
                return_abi,
                params: vec!["locals".into()],
                execution_context: crate::ExecutionContextPolicy::Local,
                ..make_function(ops)
            };
            let signature = source.function_signature().unwrap();
            let result = roundtrip_function(source.clone());
            source.ops = result.clone();
            assert_eq!(source.function_signature().unwrap(), signature);
            crate::validate_simple_ir(&crate::SimpleIR {
                functions: vec![source],
                profile: None,
            })
            .expect("checked frame returns remain schema-valid after optimization");
            let has_trace_enter = result
                .iter()
                .any(|o| o.kind == molt_ir::OpKind::TraceEnterSlot);
            let has_trace_exit = result.iter().any(|o| o.kind == molt_ir::OpKind::TraceExit);
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
            let enter = result
                .iter()
                .position(|op| op.kind == molt_ir::OpKind::TraceEnterSlot)
                .unwrap();
            let check = &result[enter + 1];
            assert_eq!(check.kind, molt_ir::OpKind::CheckException, "{result:?}");
            let failure_label = check.value.expect("entry guard retains a target");
            let failure = result
                .iter()
                .position(|op| op.kind == molt_ir::OpKind::Label && op.value == Some(failure_label))
                .expect("entry cleanup remains reachable");
            assert_eq!(
                result[failure + 1].kind,
                molt_ir::OpKind::TraceExit,
                "{result:?}"
            );
            assert_eq!(
                result[failure + 2].kind,
                molt_ir::OpKind::RetVoid,
                "{result:?}"
            );
            for (index, operation) in result.iter().enumerate() {
                if operation.kind == molt_ir::OpKind::TraceExit {
                    assert!((result[index + 1].kind).is_return_terminator());
                }
            }
        }
    }

    #[test]
    fn roundtrip_const_zero_value() {
        let ops = vec![
            OpIR {
                kind: molt_ir::OpKind::Const,
                out: Some("z".into()),
                value: Some(0),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            op_args(molt_ir::OpKind::Ret, &["z"]),
        ];
        let result = roundtrip_no_opt(ops);
        let cnst = result.iter().find(|o| o.kind == molt_ir::OpKind::Const);
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
            int_literal("stop", 3), // stop = 3
            int_literal("idx", 0),  // idx = 0
            op(molt_ir::OpKind::LoopStart),
            // Loop body: a call that must NOT be eliminated
            call_out("result", "observe", &["idx"]),
            // Increment
            int_literal("one", 1),
            op_out_args(molt_ir::OpKind::Add, "idx2", &["idx", "one"]),
            op(molt_ir::OpKind::LoopEnd),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip(ops);
        // The "call" op must exist in the output — it has side effects
        // and must not be eliminated by any optimization pass.
        let call_count = result
            .iter()
            .filter(|o| o.kind == molt_ir::OpKind::Call)
            .count();
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
            int_literal("stop", 3),
            int_literal("idx", 0),
            op(molt_ir::OpKind::LoopStart),
            call_out("result", "observe", &["idx"]),
            int_literal("one", 1),
            op_out_args(molt_ir::OpKind::Add, "idx2", &["idx", "one"]),
            op(molt_ir::OpKind::LoopEnd),
            op(molt_ir::OpKind::RetVoid),
        ];
        let result = roundtrip_no_opt(ops);
        let call_count = result
            .iter()
            .filter(|o| o.kind == molt_ir::OpKind::Call)
            .count();
        assert!(
            call_count >= 1,
            "loop body 'call' op was eliminated even without optimization! \
             Output ops: {:?}",
            result.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
    }
}
