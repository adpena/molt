use super::*;
use molt_tir::target_admission::PENDING_CALL_EVAL_BREAKER_REQUIREMENT_REASON;

#[test]
fn test_compile_checked_keeps_ordinary_programs_available() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };

    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("ordinary Luau programs must not require the native pending-call boundary");
    assert!(source.contains("molt_main"));
}

#[test]
fn test_compile_checked_rejects_async_work_poll_runtime_requirement_without_boundary() {
    let cases = [
        (
            "async_work_poll",
            OpIR {
                kind: "async_work_poll".to_string(),
                value: Some(0),
                ..OpIR::default()
            },
        ),
        (
            "exception_finally_pending_observer",
            OpIR {
                kind: "exception_finally_pending_observer".to_string(),
                out: Some("pending".to_string()),
                async_work_poll: true,
                ..OpIR::default()
            },
        ),
    ];

    for (kind, op) in cases {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: format!("{kind}_test"),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: ExecutionContextPolicy::None,
                ops: vec![op],
            }],
            profile: None,
        };

        let err = LuauBackend::new()
            .compile_checked(&ir)
            .expect_err("Luau must not erase an async-work observation");
        assert!(
            err.contains(kind),
            "diagnostic must name the carrier `{kind}`: {err}"
        );
        assert!(
            err.contains(PENDING_CALL_EVAL_BREAKER_REQUIREMENT_REASON),
            "diagnostic must name the missing target capability: {err}"
        );
    }
}

fn exception_op(kind: &str, args: &[&str], out: Option<&str>, value: Option<i64>) -> OpIR {
    OpIR {
        kind: kind.into(),
        args: Some(args.iter().map(|arg| (*arg).into()).collect()),
        out: out.map(str::to_string),
        value,
        ..OpIR::default()
    }
}

#[test]
fn exception_captures_wrap_throwing_operations_not_try_intervals() {
    let ops = vec![
        exception_op("try_start", &[], None, Some(5)),
        exception_op("const_int", &[], Some("v0"), Some(1)),
        exception_op("call_func", &["callback", "v0"], Some("v1"), None),
        exception_op("check_exception", &[], None, Some(2)),
        exception_op("try_end", &[], None, Some(5)),
        exception_op("exception_last_pending", &[], Some("caught"), None),
        exception_op("try_end", &[], None, Some(5)),
    ];
    let lowered = lower_exception_captures(&ops);
    let begin = lowered
        .iter()
        .position(|op| op.kind == "pcall_wrap_begin")
        .unwrap();
    assert_eq!(
        lowered[begin].args.as_deref(),
        Some(&["v1".to_string()][..])
    );
    assert_eq!(lowered[begin + 1].kind, "call_func");
    assert_eq!(lowered[begin + 2].kind, "pcall_wrap_end");
    assert_eq!(lowered[begin + 2].value, lowered[begin].value);
    assert_eq!(
        lowered
            .iter()
            .filter(|op| op.kind == "pcall_wrap_begin")
            .count(),
        1
    );
    assert!(
        !lowered
            .iter()
            .any(|op| matches!(op.kind.as_str(), "try_start" | "try_end"))
    );
}

#[test]
fn exception_captures_preserve_explicit_handler_edges_and_pending_reads() {
    let ops = vec![
        exception_op("try_start", &[], None, Some(5)),
        exception_op("call_func", &["callback"], Some("result"), None),
        exception_op("check_exception", &[], None, Some(2)),
        exception_op("raise", &["exc"], None, None),
        exception_op("jump", &[], None, Some(2)),
        exception_op("try_end", &[], None, Some(5)),
        exception_op("label", &[], None, Some(2)),
        exception_op("exception_last_pending", &[], Some("caught"), None),
        exception_op("try_end", &[], None, Some(5)),
    ];
    let lowered = lower_exception_captures(&ops);
    assert!(
        lowered
            .iter()
            .any(|op| op.kind == "check_exception" && op.value == Some(2))
    );
    assert!(
        lowered
            .iter()
            .any(|op| op.kind == "jump" && op.value == Some(2))
    );
    let pending = lowered
        .iter()
        .find(|op| op.kind == "exception_last_pending")
        .unwrap();
    assert_eq!(
        pending.value, None,
        "handler reads runtime pending state, not a capture identity"
    );
    assert_eq!(lowered.iter().filter(|op| op.kind == "label").count(), 1);
}

pub(super) fn luau_tir_roundtrip_function(mut func: FunctionIR) -> FunctionIR {
    if func.ops.iter().any(|op| op.kind == "phi") {
        molt_tir::ir_rewrites::rewrite_phi_to_store_load(&mut func.ops);
    }
    let target_info = crate::tir::target_info::TargetInfo::luau_release_fast();
    let mut tir_func = crate::tir::lower_from_simple::lower_to_tir_for_target(&func, &target_info);
    crate::tir::type_refine::refine_types(&mut tir_func);
    let _stats = crate::tir::passes::run_pipeline(&mut tir_func, &target_info);
    let _drop_changed =
        crate::tir::drop_phase::finalize_function_drops(&mut tir_func, &target_info);
    crate::tir::type_refine::refine_types(&mut tir_func);
    func.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func);
    func
}

fn path_local_exception_fixture() -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: "path_local_catch".into(),
            params: vec!["callback".into(), "condition".into()],
            ops: vec![
                exception_op("exception_stack_enter", &[], Some("baseline"), None),
                exception_op("exception_push", &[], None, None),
                exception_op("try_start", &[], None, Some(10)),
                exception_op("loop_start", &[], None, None),
                exception_op("if", &["condition"], None, None),
                exception_op("call_func", &["callback"], Some("left"), None),
                exception_op("check_exception", &[], None, Some(10)),
                exception_op("try_end", &[], None, Some(10)),
                exception_op("else", &[], None, None),
                exception_op("call_func", &["callback"], Some("right"), None),
                exception_op("check_exception", &[], None, Some(10)),
                exception_op("try_end", &[], None, Some(10)),
                exception_op("end_if", &[], None, None),
                exception_op("loop_break", &[], None, None),
                exception_op("loop_end", &[], None, None),
                exception_op("exception_pop", &[], None, None),
                exception_op("exception_stack_exit", &["baseline"], None, None),
                exception_op("const_none", &[], Some("success"), None),
                exception_op("ret", &["success"], None, None),
                exception_op("label", &[], None, Some(10)),
                exception_op("try_end", &[], None, Some(10)),
                exception_op("exception_last_pending", &[], Some("caught"), None),
                exception_op("exception_context_set", &["caught"], None, None),
                exception_op("exception_clear", &[], None, None),
                exception_op("exception_pop", &[], None, None),
                exception_op("exception_stack_exit", &["baseline"], None, None),
                exception_op("ret", &["caught"], None, None),
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    }
}

#[test]
fn checked_path_local_closes_keep_operation_captures_and_explicit_observers() {
    let source = LuauBackend::new()
        .compile_checked(&path_local_exception_fixture())
        .expect("path-local closes inside IF/LOOP must produce valid checked source");
    assert!(source.contains("pcall(function()"), "{source}");
    assert!(
        source.contains("molt_exception_capture(__molt_pcall_frame_context_"),
        "{source}"
    );
    assert!(
        source.contains("caught = molt_exception_last_pending()"),
        "{source}"
    );
    assert!(
        source.contains("if molt_exception_pending() then"),
        "{source}"
    );
    assert!(!source.contains("caught = __err_"), "{source}");
    assert!(source.contains("molt_exception_push()") && source.contains("molt_exception_pop()"));
}

#[test]
fn capture_results_remain_in_the_original_lexical_scope() {
    let ops = vec![
        exception_op("try_start", &[], None, Some(10)),
        exception_op("if", &["condition"], None, None),
        exception_op("call_func", &["callback"], Some("result"), None),
        exception_op("check_exception", &[], None, Some(10)),
        exception_op("call_func", &["consume", "result"], None, None),
        exception_op("try_end", &[], None, Some(10)),
        exception_op("end_if", &[], None, None),
    ];
    let lowered = lower_exception_captures(&ops);
    let begin = lowered
        .iter()
        .position(|op| op.kind == "pcall_wrap_begin")
        .unwrap();
    assert_eq!(lowered[begin - 1].kind, "if");
    assert_eq!(
        lowered[begin].args.as_deref(),
        Some(&["result".to_string()][..])
    );
    assert_eq!(lowered[begin + 2].kind, "pcall_wrap_end");
    assert_eq!(lowered[begin + 3].kind, "check_exception");
    assert_eq!(lowered.last().unwrap().kind, "end_if");
}

#[test]
fn operation_capture_preserves_explicit_exception_edge_store_order() {
    let ops = vec![
        OpIR {
            kind: "call".into(),
            out: Some("v_call".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb1_arg0".into()),
            args: Some(vec!["module_obj".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            value: Some(5),
            ..OpIR::default()
        },
    ];

    let captured = lower_exception_captures(&ops);

    assert_eq!(captured[0].kind, "pcall_wrap_begin");
    assert_eq!(captured[1].kind, "call");
    assert_eq!(captured[2].kind, "pcall_wrap_end");
    assert_eq!(captured[3].kind, "store_var");
    assert_eq!(captured[4].kind, "check_exception");
}

#[test]
fn pending_raise_preserves_explicit_exception_edge_store_order() {
    let ops = vec![
        OpIR {
            kind: "raise".into(),
            args: Some(vec!["exc".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb5_arg0".into()),
            args: Some(vec!["caught".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb5_arg1".into()),
            args: Some(vec!["limit".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".into(),
            value: Some(5),
            ..OpIR::default()
        },
    ];

    let captured = lower_exception_captures(&ops);

    assert_eq!(captured[0].kind, "raise");
    assert_eq!(captured[1].kind, "store_var");
    assert_eq!(captured[1].var.as_deref(), Some("_bb5_arg0"));
    assert_eq!(captured[2].kind, "store_var");
    assert_eq!(captured[2].var.as_deref(), Some("_bb5_arg1"));
    assert_eq!(captured[3].kind, "jump");
}

#[test]
fn captured_result_store_executes_before_exception_observer() {
    let ops = vec![
        OpIR {
            kind: "call".into(),
            out: Some("v_call".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb1_arg0".into()),
            args: Some(vec!["v_call".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            value: Some(5),
            ..OpIR::default()
        },
    ];

    let captured = lower_exception_captures(&ops);

    assert_eq!(captured[1].kind, "call");
    assert_eq!(captured[2].kind, "pcall_wrap_end");
    assert_eq!(captured[3].kind, "store_var");
    assert_eq!(captured[4].kind, "check_exception");
}

#[test]
fn test_luau_exception_region_module_global_ops_use_module_dict_helpers() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "module_global_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_str".into(),
                    out: Some("name".into()),
                    s_value: Some("exc".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".into(),
                    out: Some("module".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_global".into(),
                    args: Some(vec!["module".into(), "name".into()]),
                    out: Some("value".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_del_global_if_present".into(),
                    args: Some(vec!["module".into(), "name".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("molt_module_get_global(module, name)"),
        "module_get_global must read the supplied module dict:\n{output}"
    );
    assert!(
        output.contains("molt_module_del_global(module, name, true)"),
        "module_del_global_if_present must delete from the supplied module dict:\n{output}"
    );
    assert!(
        !output.contains("local value = molt_module_cache[name]"),
        "module_get_global must not read import cache directly:\n{output}"
    );
}

#[test]
fn test_luau_exception_region_type_of_uses_python_descriptor_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "type_descriptor_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "exception_new_builtin_empty".into(),
                    out: Some("exc".into()),
                    s_value: Some("NameError".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "type_of".into(),
                    args: Some(vec!["exc".into()]),
                    out: Some("typ".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".into(),
                    args: Some(vec!["typ".into()]),
                    out: Some("name".into()),
                    s_value: Some("__name__".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("local typ = molt_type_of(exc)")
            && output.contains("if type(x) == \"table\" and x.__type then"),
        "type_of must preserve Python exception class identity:\n{output}"
    );
}

#[test]
fn test_pcall_try_except_compile() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "try_except_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "try_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(1),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(0),
                    out: Some("v1".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "binary_op".into(),
                    s_value: Some("/".into()),
                    args: Some(vec!["v0".into(), "v1".into()]),
                    out: Some("v2".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".into(),
                    out: Some("v3".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(42),
                    out: Some("v4".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_function".into(),
                    s_value: Some("print".into()),
                    args: Some(vec!["print".into(), "v4".into()]),
                    out: Some("v5".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(
        output.contains("pcall(function()"),
        "Expected pcall wrapper, got:\n{output}"
    );
    assert!(
        output.contains("__ok_0") && output.contains("__err_0"),
        "Expected __ok_0/__err_0, got:\n{output}"
    );
    assert!(
        !output.contains("= nil -- [exception_last]"),
        "exception_last should NOT emit nil inside pcall, got:\n{output}"
    );
}

#[test]
fn test_no_duplicate_local_declarations() {
    // When the same variable name appears as `out` in multiple ops,
    // only the first should emit `local`.  Subsequent uses should be
    // plain assignment to avoid Luau syntax errors.
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dup_local_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                // First definition of v0 — should get `local v0 = 1`
                OpIR {
                    kind: "const_int".into(),
                    value: Some(1),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                // Second definition of v0 — must NOT emit `local` again
                OpIR {
                    kind: "const_int".into(),
                    value: Some(2),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_function".into(),
                    s_value: Some("print".into()),
                    args: Some(vec!["print".into(), "v0".into()]),
                    out: Some("v1".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    // Count occurrences of `local v0` — should be exactly 1.
    let local_v0_count = output.matches("local v0").count();
    assert_eq!(
        local_v0_count, 1,
        "Expected exactly 1 `local v0`, found {local_v0_count} in:\n{output}"
    );
}

#[test]
fn alternative_nested_try_closes_never_capture_lexical_control() {
    let ops = path_local_exception_fixture().functions.remove(0).ops;
    let lowered = lower_exception_captures(&ops);
    let captures: Vec<_> = lowered
        .iter()
        .enumerate()
        .filter(|(_, op)| op.kind == "pcall_wrap_begin")
        .collect();
    assert_eq!(
        captures.len(),
        2,
        "two callback operations, not a lexical TRY count"
    );
    for (index, begin) in captures {
        assert_eq!(lowered[index + 1].kind, "call_func");
        assert_eq!(lowered[index + 2].kind, "pcall_wrap_end");
        assert_eq!(lowered[index + 2].value, begin.value);
    }
    let control_kinds = |ops: &[OpIR]| {
        ops.iter()
            .filter_map(|op| {
                matches!(
                    op.kind.as_str(),
                    "loop_start" | "loop_end" | "if" | "else" | "end_if" | "ret"
                )
                .then_some(op.kind.clone())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(control_kinds(&lowered), control_kinds(&ops));
}

#[test]
fn structured_return_keeps_explicit_exception_cleanup() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "stored_return".into(),
            params: vec![
                "slot".into(),
                "index".into(),
                "value".into(),
                "baseline".into(),
            ],
            ops: vec![
                exception_op("store_index", &["slot", "index", "value"], None, None),
                exception_op("exception_stack_exit", &["baseline"], None, None),
                exception_op("index", &["slot", "index"], Some("loaded"), None),
                exception_op("ret", &["loaded"], None, None),
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);
    assert!(
        source.contains("molt_exception_stack_exit(baseline)"),
        "{source}"
    );
    assert!(source.contains("molt_exception_propagate()"), "{source}");
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn checked_exception_flow_executes_nested_edges_calls_cleanup_and_coroutine_custody() {
    let mut ir = path_local_exception_fixture();
    ir.functions.extend([
        FunctionIR {
            name: "loop_pending_observer".into(),
            params: vec!["callback".into(), "cleanup".into()],
            ops: vec![
                exception_op("loop_start", &[], None, None),
                exception_op("call_func", &["callback"], Some("called"), None),
                exception_op("loop_break_if_exception", &[], None, None),
                exception_op("loop_break", &[], None, None),
                exception_op("loop_end", &[], None, None),
                exception_op("call_func", &["cleanup"], Some("cleaned"), None),
                exception_op("ret_void", &[], None, None),
            ],
            ..FunctionIR::default()
        },
        FunctionIR {
            name: "terminal_observer".into(),
            params: vec!["handled".into()],
            ops: vec![
                exception_op("jump", &[], None, Some(20)),
                exception_op("label", &[], None, Some(10)),
                exception_op("exception_clear", &[], None, None),
                exception_op("ret", &["handled"], None, None),
                exception_op("label", &[], None, Some(20)),
                exception_op("check_exception", &[], None, Some(10)),
            ],
            ..FunctionIR::default()
        },
        FunctionIR {
            name: "bare_reraise".into(),
            ops: vec![
                exception_op("raise", &[], None, None),
                exception_op("ret_void", &[], None, None),
            ],
            ..FunctionIR::default()
        },
        FunctionIR {
            name: "cleanup_override".into(),
            params: vec!["saved".into(), "replacement".into()],
            ops: vec![
                exception_op("exception_stack_enter", &[], Some("baseline"), None),
                exception_op("exception_push", &[], None, None),
                exception_op("exception_context_set", &["saved"], None, None),
                exception_op("raise", &["replacement"], None, None),
                exception_op("exception_pop", &[], None, None),
                exception_op("exception_stack_exit", &["baseline"], None, None),
                exception_op("ret_void", &[], None, None),
            ],
            ..FunctionIR::default()
        },
        FunctionIR {
            name: "loop_resume_index".into(),
            ops: vec![
                exception_op("const_int", &[], Some("zero"), Some(0)),
                exception_op("const_int", &[], Some("one"), Some(1)),
                exception_op("loop_start", &[], None, None),
                exception_op("loop_index_start", &["zero"], Some("index"), None),
                exception_op("loop_break_if_true", &["index"], None, None),
                exception_op("check_exception", &[], None, Some(40)),
                exception_op("loop_index_next", &["one"], Some("index"), None),
                exception_op("loop_continue", &[], None, None),
                exception_op("loop_end", &[], None, None),
                exception_op("ret", &["index"], None, None),
                exception_op("label", &[], None, Some(40)),
                exception_op("ret_void", &[], None, None),
            ],
            ..FunctionIR::default()
        },
    ]);
    let mut source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("explicit exception protocol must pass checked source admission");
    source.push_str(r#"
local outer = {__type="ValueError", __msg="outer"}
local inner = {__type="TypeError", __msg="inner"}
local replacement = {__type="RuntimeError", __msg="cleanup"}
local baseline = molt_exception_stack_enter()
molt_exception_push()
molt_exception_context_set(outer)
local owned_depth = molt_exception_stack_depth()
local owned_baseline = molt_frame_context().exceptions.baseline
assert(molt_exception_active() == outer and not molt_exception_pending())
assert(terminal_observer("handled") == nil)
molt_exception_set_last(inner)
assert(terminal_observer("handled") == "handled")
assert(molt_exception_active() == outer and not molt_exception_pending())
local loop_cleaned = false
assert(path_local_catch(function()
    loop_pending_observer(function() error(inner, 0) end, function() loop_cleaned = true end)
end, true) == inner)
assert(loop_cleaned and not molt_exception_pending())

for _, condition in {false, true} do
    local called = 0
    local caught = path_local_catch(function()
        called += 1
        error(inner, 0)
    end, condition)
    assert(called == 1 and caught == inner)
    assert(inner.__context__ == outer)
    assert(molt_exception_active() == outer and not molt_exception_pending())
    assert(molt_exception_stack_depth() == owned_depth)
    assert(molt_frame_context().exceptions.baseline == owned_baseline)
    assert(path_local_catch(function() called += 1; return 7 end, condition) == nil)
    assert(called == 2)
end

-- The callee does not own a lexical handler: bare raise sees its caller's
-- dynamically handled exception, and the explicit caller check receives it.
assert(path_local_catch(bare_reraise, true) == outer)
assert(molt_exception_active() == outer and not molt_exception_pending())
molt_exception_set_last(inner)
assert(molt_exception_current() == outer and molt_exception_last_pending() == inner)
molt_exception_clear()
assert(molt_exception_active() == outer and molt_exception_last_pending() == nil)

assert(path_local_catch(function() cleanup_override(outer, replacement) end, false) == replacement)
assert(replacement.__context__ == outer)
assert(molt_exception_stack_depth() == owned_depth and molt_exception_active() == outer)

local abandoned_inner = {__type="ValueError", __msg="abandoned inner handler"}
local host_failure = {__type="RuntimeError", __msg="host throw"}
assert(path_local_catch(function()
    molt_exception_stack_enter()
    molt_exception_push()
    molt_exception_context_set(abandoned_inner)
    error(host_failure, 0)
end, true) == host_failure)
assert(host_failure.__context__ == abandoned_inner)
assert(molt_exception_stack_depth() == owned_depth and molt_exception_active() == outer)
assert(molt_frame_context().exceptions.baseline == owned_baseline)
assert(loop_resume_index() == 1)

-- Each coroutine owns handled state across suspension, while unhandled
-- coroutines inherit only the active exception of the current resumer.
local generator_context: any = nil
local resume, close = molt_coroutine_execution_wrap(function()
    generator_context = molt_frame_context()
    assert(molt_exception_active() == outer)
    local previous = molt_exception_stack_enter()
    molt_exception_push()
    molt_exception_context_set(inner)
    coroutine.yield("held")
    assert(molt_exception_active() == inner and not molt_exception_pending())
    assert(path_local_catch(bare_reraise, false) == inner)
    molt_exception_pop()
    molt_exception_stack_exit(previous)
    assert(molt_exception_active() == replacement)
    return "done"
end)
assert(resume() == "held")
assert(generator_context ~= molt_frame_context() and molt_exception_active() == outer)
molt_exception_context_set(replacement)
assert(resume() == "done")
assert(generator_context.exceptions.inherited == nil)
assert(#generator_context.exceptions.handlers == 0)
close()
assert(molt_exception_active() == replacement and not molt_exception_pending())
molt_exception_pop()
molt_exception_stack_exit(baseline)
assert(molt_exception_active() == nil and not molt_exception_pending())
local no_handler_ok, no_handler_error = pcall(bare_reraise)
assert(not no_handler_ok and no_handler_error.__type == "RuntimeError")
molt_exception_clear()
local poisoned = coroutine.create(function()
    local context, owner = molt_frame_context()
    molt_frame_enter({code={co_filename="capture.py", co_name="poisoned", co_firstlineno=1}, globals={}})
    table.freeze(context.codes)
    local ok, failure = pcall(molt_exception_capture, context, owner, 0, 0, 0, inner)
    assert(not ok and failure.__msg == "execution-frame restoration failed")
    assert(molt_frame_owned_context(owner) == nil)
    return true
end)
local poisoned_ok, verified = coroutine.resume(poisoned)
assert(poisoned_ok and verified)
print("luau-path-local-exception-flow-ok")
"#);
    let output = execute_lune_oracle("path_local_exception_flow", &source);
    assert!(String::from_utf8_lossy(&output.stdout).contains("luau-path-local-exception-flow-ok"));
}
