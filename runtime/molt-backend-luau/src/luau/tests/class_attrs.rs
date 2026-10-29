use super::*;

#[test]
fn test_compile_checked_lowers_type_check_helpers() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "type_check_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    out: Some("int_tag".to_string()),
                    value: Some(1),
                    ..OpIR::new(molt_ir::OpKind::Const)
                },
                OpIR {
                    out: Some("int_cls".to_string()),
                    args: Some(vec!["int_tag".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::BuiltinType)
                },
                OpIR {
                    out: Some("bool_tag".to_string()),
                    value: Some(3),
                    ..OpIR::new(molt_ir::OpKind::Const)
                },
                OpIR {
                    out: Some("bool_cls".to_string()),
                    args: Some(vec!["bool_tag".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::BuiltinType)
                },
                OpIR {
                    out: Some("flag".to_string()),
                    value: Some(1),
                    ..OpIR::new(molt_ir::OpKind::ConstBool)
                },
                OpIR {
                    out: Some("is_int".to_string()),
                    args: Some(vec!["flag".to_string(), "int_cls".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::Isinstance)
                },
                OpIR {
                    out: Some("bool_is_int".to_string()),
                    args: Some(vec!["bool_cls".to_string(), "int_cls".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::Issubclass)
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(
        source.contains("function molt_builtin_type")
            && source.contains("function molt_issubclass")
            && source.contains("function molt_isinstance"),
        "type-check helper authority should be emitted, got:\n{source}"
    );
    assert!(
        source.contains("local int_cls = molt_builtin_type(int_tag)")
            && source.contains("local is_int = molt_isinstance(flag, int_cls)")
            && source.contains("local bool_is_int = molt_issubclass(bool_cls, int_cls)"),
        "type-check ops should use named builtin metadata, got:\n{source}"
    );
    assert!(
        !source.contains("[stub: isinstance]")
            && !source.contains("[unsupported op: isinstance]")
            && !source.contains("[unsupported op: issubclass]")
            && !source.contains("[unsupported op: builtin_type]"),
        "type-check ops must not leave checked-output markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_callable_builtin_through_invocation_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "callable_builtin_test".to_string(),
            params: vec!["candidate".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    out: Some("callable_fn".to_string()),
                    s_value: Some("molt_callable_builtin".to_string()),
                    ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
                },
                OpIR {
                    out: Some("callable_args".to_string()),
                    args: Some(vec!["candidate".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::CallargsNew)
                },
                OpIR {
                    out: Some("is_callable".to_string()),
                    args: Some(vec!["callable_fn".to_string(), "callable_args".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::CallFunc)
                },
                OpIR {
                    args: Some(vec!["is_callable".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::Ret)
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(
        source
            .contains("local callable_fn = function(a, ...) return type(a[1]) == \"function\" end")
            && source.contains("local is_callable = molt_call_checked(callable_fn, callable_args)"),
        "callable builtin should share the Luau function predicate, got:\n{source}"
    );
    assert!(
        !source.contains("molt_callable(")
            && !source.contains("[unsupported op: builtin_func]")
            && !source.contains("[unsupported op: call_func]"),
        "callable builtin must not emit an undefined helper or markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_descriptor_attribute_authority() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_attribute_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![
                    OpIR {
                        out: Some("method_func".to_string()),
                        s_value: Some("descriptor_method".to_string()),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    },
                    OpIR {
                        out: Some("class_func".to_string()),
                        s_value: Some("descriptor_class".to_string()),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    },
                    OpIR {
                        out: Some("static_func".to_string()),
                        s_value: Some("descriptor_static".to_string()),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    },
                    OpIR {
                        out: Some("get_func".to_string()),
                        s_value: Some("descriptor_get".to_string()),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    },
                    OpIR {
                        out: Some("set_func".to_string()),
                        s_value: Some("descriptor_set".to_string()),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    },
                    OpIR {
                        out: Some("del_func".to_string()),
                        s_value: Some("descriptor_del".to_string()),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    },
                    OpIR {
                        out: Some("cls".to_string()),
                        ..OpIR::new(molt_ir::OpKind::ObjectNew)
                    },
                    OpIR {
                        out: Some("obj".to_string()),
                        ..OpIR::new(molt_ir::OpKind::ObjectNew)
                    },
                    OpIR {
                        out: Some("cm_desc".to_string()),
                        args: Some(vec!["class_func".to_string()]),
                        ..OpIR::new(molt_ir::OpKind::ClassmethodNew)
                    },
                    OpIR {
                        out: Some("sm_desc".to_string()),
                        args: Some(vec!["static_func".to_string()]),
                        ..OpIR::new(molt_ir::OpKind::StaticmethodNew)
                    },
                    OpIR {
                        out: Some("prop_desc".to_string()),
                        args: Some(vec![
                            "get_func".to_string(),
                            "set_func".to_string(),
                            "del_func".to_string(),
                        ]),
                        ..OpIR::new(molt_ir::OpKind::PropertyNew)
                    },
                    OpIR {
                        args: Some(vec!["cls".to_string(), "method_func".to_string()]),
                        s_value: Some("method".to_string()),
                        ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
                    },
                    OpIR {
                        args: Some(vec!["cls".to_string(), "cm_desc".to_string()]),
                        s_value: Some("cm".to_string()),
                        ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
                    },
                    OpIR {
                        args: Some(vec!["cls".to_string(), "sm_desc".to_string()]),
                        s_value: Some("sm".to_string()),
                        ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
                    },
                    OpIR {
                        args: Some(vec!["cls".to_string(), "prop_desc".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
                    },
                    OpIR {
                        out: Some("cm_bound".to_string()),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("cm".to_string()),
                        ..OpIR::new(molt_ir::OpKind::GetAttrGenericObj)
                    },
                    OpIR {
                        out: Some("sm_func".to_string()),
                        args: Some(vec!["cls".to_string()]),
                        s_value: Some("sm".to_string()),
                        ..OpIR::new(molt_ir::OpKind::GetAttrGenericObj)
                    },
                    OpIR {
                        out: Some("prop_value".to_string()),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::new(molt_ir::OpKind::GetAttrGenericObj)
                    },
                    OpIR {
                        out: Some("value_name".to_string()),
                        s_value: Some("value".to_string()),
                        ..OpIR::new(molt_ir::OpKind::ConstStr)
                    },
                    OpIR {
                        out: Some("has_value".to_string()),
                        args: Some(vec!["obj".to_string(), "value_name".to_string()]),
                        ..OpIR::new(molt_ir::OpKind::HasAttrName)
                    },
                    OpIR {
                        out: Some("new_value".to_string()),
                        value: Some(7),
                        ..OpIR::new(molt_ir::OpKind::Const)
                    },
                    OpIR {
                        args: Some(vec!["obj".to_string(), "new_value".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
                    },
                    OpIR {
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::new(molt_ir::OpKind::DelAttrGenericObj)
                    },
                    OpIR {
                        out: Some("method_result".to_string()),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("method".to_string()),
                        ..OpIR::new(molt_ir::OpKind::CallMethod)
                    },
                    OpIR {
                        out: Some("getattr_builtin".to_string()),
                        s_value: Some("molt_getattr_builtin".to_string()),
                        ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
                    },
                    OpIR {
                        out: Some("setattr_builtin".to_string()),
                        s_value: Some("molt_set_attr_name".to_string()),
                        ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
                    },
                    OpIR {
                        out: Some("delattr_builtin".to_string()),
                        s_value: Some("molt_del_attr_name".to_string()),
                        ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
                    },
                    OpIR {
                        out: Some("hasattr_builtin".to_string()),
                        s_value: Some("molt_has_attr_name".to_string()),
                        ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
                    },
                ],
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_method".to_string(),
                params: vec!["self".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_class".to_string(),
                params: vec!["cls".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_static".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_get".to_string(),
                params: vec!["self".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_set".to_string(),
                params: vec!["self".to_string(), "value".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "descriptor_del".to_string(),
                params: vec!["self".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            },
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(
        source.contains("function molt_get_attr")
            && source.contains("function molt_has_attr")
            && source.contains("function molt_set_attr")
            && source.contains("function molt_del_attr"),
        "descriptor-aware attribute helpers should be emitted, got:\n{source}"
    );
    assert!(
            source.contains(
                "local cm_desc = {__molt_descriptor_kind=\"classmethod\", __func=class_func}"
            ) && source.contains(
                "local sm_desc = {__molt_descriptor_kind=\"staticmethod\", __func=static_func}"
            ) && source.contains(
                "local prop_desc = {__molt_descriptor_kind=\"property\", __get=get_func, __set=set_func, __del=del_func}"
            ),
            "descriptor constructors should use one table shape, got:\n{source}"
        );
    assert!(
        source.contains("then molt_function_attr_set(cls, \"cm\", cm_desc) else molt_set_attr(cls, \"cm\", cm_desc) end")
            && source.contains("then molt_func_attr_get(obj, \"cm\") else molt_get_attr(obj, \"cm\")")
            && source.contains("then molt_func_attr_get(cls, \"sm\") else molt_get_attr(cls, \"sm\")")
            && source.contains("then molt_func_attr_get(obj, \"value\") else molt_get_attr(obj, \"value\")")
            && source.contains("then molt_func_attr_get(obj, value_name) ~= nil else molt_has_attr(obj, value_name)")
            && source.contains("then molt_function_attr_set(obj, \"value\", new_value) else molt_set_attr(obj, \"value\", new_value) end")
            && source.contains("then molt_function_attr_del(obj, \"value\") else molt_del_attr(obj, \"value\") end")
            && source.contains(
                "local method_result; do local __method = molt_get_attr_checked(obj, \"method\");"
            )
            && source.contains("method_result = molt_call_checked(__method) end"),
        "attribute get/set/delete and method call should route through descriptor authority, got:\n{source}"
    );
    assert!(
        source.contains("local getattr_builtin = function(a, ...)")
            && source
                .contains("then molt_func_attr_get(a[1], a[2]) else molt_get_attr(a[1], a[2])")
            && source.contains("then return molt_function_attr_set(a[1], a[2], a[3]) end")
            && source.contains("then return molt_function_attr_del(a[1], a[2]) end")
            && source.contains("then return molt_func_attr_get(a[1], a[2]) ~= nil end")
            && !source.contains("molt_getattr(table.unpack(a))"),
        "attribute builtins should route through descriptor helpers, got:\n{source}"
    );
    assert!(
        !source.contains("[classmethod_new]")
            && !source.contains("[staticmethod_new]")
            && !source.contains("[property_new]")
            && !source.contains("[unsupported op: classmethod_new]")
            && !source.contains("[unsupported op: staticmethod_new]")
            && !source.contains("[unsupported op: property_new]"),
        "descriptor ops must not leave checked-output markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_rejects_unavailable_lexical_cell_transport() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "internal_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                out: Some("v0".to_string()),
                ..OpIR::new(molt_ir::OpKind::FunctionClosureBits)
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("compile_checked must reject unsupported lexical-cell transport");
    assert!(
        err.contains("rejected before source generation")
            && err.contains("`function_closure_bits`")
            && err.contains("shared mutable lexical cells")
            && err.contains("exact closure transport"),
        "error should come from generated pre-source admission, got: {err}"
    );
}

#[test]
fn test_compile_checked_rejects_retired_bridge_marker() {
    let error = molt_ir::SimpleIR::from_json_str(r#"{"functions":[{"name":"retired","return_abi":"void","params":[],"ops":[{"kind":"bridge_unavailable"}]}]}"#)
        .expect_err("a retired spelling must fail during wire decode");
    assert!(error.contains("bridge_unavailable"), "{error}");
}

#[test]
fn test_compile_checked_lowers_invoke_ffi_to_luau_capability_error() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "invoke_ffi_capability_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                out: Some("v0".to_string()),
                ..OpIR::new(molt_ir::OpKind::InvokeFfi)
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(
            source.contains(
                "local v0: any = error({__type=\"RuntimeError\", __msg=\"Luau target does not support FFI\"})"
            ),
            "invoke_ffi should be an explicit target capability error, got:\n{source}"
        );
    assert!(
        !source.contains("[invoke_ffi]") && !source.contains("[unsupported op: invoke_ffi]"),
        "invoke_ffi must not leave semantic stub markers, got:\n{source}"
    );
}

#[test]
fn class_statement_identity_and_guarded_fields_lower_by_name() {
    let statement = "class:m:P";
    let named = |kind: molt_ir::OpKind, args: &[&str], out: Option<&str>| OpIR {
        args: Some(args.iter().map(|arg| (*arg).to_string()).collect()),
        out: out.map(str::to_string),
        class_definition: Some(statement.to_string()),
        ..OpIR::new(kind)
    };
    let field = |mut op: OpIR| {
        op.s_value = Some("x".to_string());
        op.value = Some(0);
        op
    };
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "class_identity".to_string(),
            params: vec![
                "cls".to_string(),
                "obj".to_string(),
                "version".to_string(),
                "val".to_string(),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                named(molt_ir::OpKind::ClassSetDefinition, &["cls"], None),
                field(named(
                    molt_ir::OpKind::GuardedFieldSet,
                    &["obj", "version", "val"],
                    None,
                )),
                field(named(
                    molt_ir::OpKind::GuardedFieldGet,
                    &["obj", "version"],
                    Some("read"),
                )),
            ],
        }],
        profile: None,
    };
    let bindings = molt_ir::code_slots::CodeSlotBindings::try_from(
        std::collections::BTreeMap::from([(statement.to_string(), 0)]),
    )
    .unwrap();
    let source = LuauBackend::new().with_code_slots(bindings).compile(&ir);
    assert!(
        source.contains("cls.__molt_class_definition = 0"),
        "the stamp records the statement's slot: {source}"
    );
    assert!(source.contains("obj.x = val"), "{source}");
    assert!(!source.contains("obj.x = version"), "{source}");
    assert!(source.contains("local read = obj.x"), "{source}");
}

#[test]
fn luau_compile_rejects_unknown_ops_instead_of_failing_open() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "dispatch_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                out: Some("v0".to_string()),
                args: Some(vec!["v1".to_string(), "v2".to_string()]),
                ..OpIR::new(molt_ir::OpKind::GpuBarrier)
            }],
        }],
        profile: None,
    };
    let error = LuauBackend::new()
        .compile_checked(&ir)
        .expect_err("compile_checked must reject unknown ops");
    assert!(error.contains("gpu_barrier"), "{error}");
}

#[test]
fn test_luau_repr_authority_typed_list_call_method_dispatch() {
    // Structured TIR facts, not legacy transport hints, authorize direct
    // list-method lowering for Luau tables.
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "append_to".to_string(),
            params: vec!["xs".to_string(), "v".to_string()],
            param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    s_value: Some("append".to_string()),
                    args: Some(vec!["xs".to_string(), "v".to_string()]),
                    ..OpIR::new(molt_ir::OpKind::CallMethod)
                },
                OpIR::new(molt_ir::OpKind::RetVoid),
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    // Must use the shared direct list append lowering, not xs:append(v).
    assert!(
        output.contains("rawset(xs, #xs + 1, v)"),
        "Expected raw list append for list param, got:\n{output}"
    );
    assert!(
        !output.contains("xs:append"),
        "Must NOT emit method call for list.append(), got:\n{output}"
    );
}
