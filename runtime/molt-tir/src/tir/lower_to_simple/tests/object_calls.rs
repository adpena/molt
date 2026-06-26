use super::*;

#[test]
fn tir_round_trip_preserves_object_argument_call_sequence() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::passes::run_pipeline;
    use crate::tir::type_refine::refine_types;

    let callee_ir = FunctionIR {
        name: "func_objarg__g".into(),
        params: vec!["x".into()],
        ops: vec![
            OpIR {
                kind: "store_var".into(),
                var: Some("x".into()),
                args: Some(vec!["x".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".into(),
                value: Some(5),
                col_offset: Some(4),
                end_col_offset: Some(18),
                ..OpIR::default()
            },
            OpIR {
                kind: "type_of".into(),
                args: Some(vec!["x".into()]),
                out: Some("v99".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "print".into(),
                args: Some(vec!["v99".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".into(),
                out: Some("v100".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("v100".into()),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["i64".into()]),
        source_file: None,
        is_extern: false,
    };

    let caller_ir = FunctionIR {
        name: "func_objarg__molt_module_chunk_1".into(),
        params: vec!["__molt_module_obj__".into()],
        ops: vec![
            OpIR {
                kind: "line".into(),
                value: Some(1),
                col_offset: Some(0),
                end_col_offset: Some(8),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(100),
                out: Some("v63".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "builtin_type".into(),
                args: Some(vec!["v63".into()]),
                out: Some("v64".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("C".into()),
                out: Some("v65".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("C".into()),
                out: Some("v66".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("__main__".into()),
                out: Some("v67".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("v68".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("__name__".into()),
                out: Some("v69".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("__qualname__".into()),
                out: Some("v70".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("__module__".into()),
                out: Some("v71".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("__firstlineno__".into()),
                out: Some("v72".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "class_def".into(),
                args: Some(vec![
                    "v65".into(),
                    "v64".into(),
                    "v69".into(),
                    "v65".into(),
                    "v70".into(),
                    "v66".into(),
                    "v71".into(),
                    "v67".into(),
                    "v72".into(),
                    "v68".into(),
                ]),
                s_value: Some("1,4,8,1,1".into()),
                out: Some("v73".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("C".into()),
                out: Some("v74".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "v74".into(),
                    "v73".into(),
                ]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".into(),
                value: Some(4),
                col_offset: Some(0),
                end_col_offset: Some(18),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "func_new".into(),
                s_value: Some("func_objarg__g".into()),
                value: Some(1),
                out: Some("v75".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("g".into()),
                out: Some("v76".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v76".into()]),
                s_value: Some("__name__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("g".into()),
                out: Some("v77".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v77".into()]),
                s_value: Some("__qualname__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("func_objarg".into()),
                out: Some("v78".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v78".into()]),
                s_value: Some("__module__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("x".into()),
                out: Some("v79".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "tuple_new".into(),
                args: Some(vec!["v79".into()]),
                out: Some("v80".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v80".into()]),
                s_value: Some("__molt_arg_names__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("v81".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v81".into()]),
                s_value: Some("__molt_posonly__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "tuple_new".into(),
                args: Some(vec![]),
                out: Some("v82".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v82".into()]),
                s_value: Some("__molt_kwonly_names__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".into(),
                out: Some("v83".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__molt_vararg__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__molt_varkw__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__defaults__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__kwdefaults__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__doc__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("/tmp/func_objarg.py".into()),
                out: Some("v88".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(4),
                out: Some("v89".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("g".into()),
                out: Some("v90".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("x".into()),
                out: Some("v92".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "tuple_new".into(),
                args: Some(vec!["v92".into()]),
                out: Some("v93".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "tuple_new".into(),
                args: Some(vec![]),
                out: Some("v94".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "code_new".into(),
                args: Some(vec![
                    "v88".into(),
                    "v90".into(),
                    "v89".into(),
                    "v83".into(),
                    "v93".into(),
                    "v94".into(),
                    "v68".into(),
                    "v81".into(),
                    "v81".into(),
                ]),
                out: Some("v97".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "set_attr_generic_obj".into(),
                args: Some(vec!["v75".into(), "v97".into()]),
                s_value: Some("__code__".into()),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "code_slot_set".into(),
                value: Some(0),
                args: Some(vec!["v97".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("g".into()),
                out: Some("v98".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "v98".into(),
                    "v75".into(),
                ]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".into(),
                value: Some(7),
                col_offset: Some(0),
                end_col_offset: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("C".into()),
                out: Some("v101".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_attr".into(),
                args: Some(vec!["__molt_module_obj__".into(), "v101".into()]),
                out: Some("v102".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "callargs_new".into(),
                out: Some("v103".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_bind".into(),
                args: Some(vec!["v102".into(), "v103".into()]),
                out: Some("v104".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("c".into()),
                out: Some("v105".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "v105".into(),
                    "v104".into(),
                ]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".into(),
                value: Some(8),
                col_offset: Some(0),
                end_col_offset: Some(4),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".into(),
                s_value: Some("func_objarg__g".into()),
                args: Some(vec!["v104".into()]),
                value: Some(0),
                out: Some("v106".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".into(),
                out: Some("v107".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".into(),
                out: Some("v108".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".into(),
                out: Some("v108".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "is".into(),
                args: Some(vec!["v107".into(), "v108".into()]),
                out: Some("v109".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "not".into(),
                args: Some(vec!["v109".into()]),
                out: Some("v110".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".into(),
                args: Some(vec!["v110".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("func_objarg".into()),
                out: Some("v111".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_del".into(),
                args: Some(vec!["v111".into()]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("__main__".into()),
                out: Some("v112".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_del".into(),
                args: Some(vec!["v112".into()]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["i64".into()]),
        source_file: None,
        is_extern: false,
    };

    for func_ir in [callee_ir, caller_ir] {
        let mut tir_func = lower_to_tir(&func_ir);
        refine_types(&mut tir_func);
        run_pipeline(
            &mut tir_func,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        refine_types(&mut tir_func);
        let round_tripped = lower_to_simple_ir(&tir_func);

        for op in &round_tripped {
            assert!(
                op.fast_float.is_none(),
                "roundtrip must not mark object-arg call path as fast_float: {round_tripped:?}"
            );
        }

        if func_ir.name == "func_objarg__g" {
            let type_of = round_tripped
                .iter()
                .find(|op| op.kind == "type_of")
                .expect("callee must preserve type_of");
            assert_eq!(type_of.args.as_ref().map(Vec::len), Some(1));
            let arg_name = type_of.args.as_ref().unwrap()[0].clone();
            let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
                .iter()
                .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
                .collect();
            let arg_op = producer_by_out
                .get(&arg_name)
                .expect("type_of operand must come from a copy_var");
            assert_eq!(arg_op.kind, "copy_var");
            assert_eq!(arg_op.var.as_deref(), Some("x"));
        } else {
            let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
                .iter()
                .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
                .collect();
            let call = round_tripped
                .iter()
                .find(|op| op.kind == "call" && op.s_value.as_deref() == Some("func_objarg__g"))
                .expect("caller must preserve direct call to func_objarg__g");
            let call_args = call
                .args
                .as_ref()
                .expect("direct call must keep its argument");
            assert_eq!(call_args.len(), 1);
            let arg_op = producer_by_out
                .get(&call_args[0])
                .expect("direct call argument must come from an op");
            assert_eq!(arg_op.kind, "call_bind");
            assert_eq!(arg_op.s_value, None);
        }
    }
}
