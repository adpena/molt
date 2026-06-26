use super::*;

#[test]
fn test_compile_checked_lowers_type_check_helpers() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "type_check_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("int_tag".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "builtin_type".to_string(),
                    out: Some("int_cls".to_string()),
                    args: Some(vec!["int_tag".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("bool_tag".to_string()),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "builtin_type".to_string(),
                    out: Some("bool_cls".to_string()),
                    args: Some(vec!["bool_tag".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".to_string(),
                    out: Some("flag".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "isinstance".to_string(),
                    out: Some("is_int".to_string()),
                    args: Some(vec!["flag".to_string(), "int_cls".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "issubclass".to_string(),
                    out: Some("bool_is_int".to_string()),
                    args: Some(vec!["bool_cls".to_string(), "int_cls".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_new".to_string(),
                    out: Some("base_cls".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_new".to_string(),
                    out: Some("derived_cls".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_set_base".to_string(),
                    args: Some(vec!["derived_cls".to_string(), "base_cls".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new".to_string(),
                    out: Some("obj".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_set_class".to_string(),
                    args: Some(vec!["obj".to_string(), "derived_cls".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "isinstance".to_string(),
                    out: Some("obj_is_base".to_string()),
                    args: Some(vec!["obj".to_string(), "base_cls".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("type-check ops should lower through Luau helper authority");
    assert!(
        source.contains("local function molt_builtin_type")
            && source.contains("local function molt_issubclass")
            && source.contains("local function molt_isinstance"),
        "type-check helper authority should be emitted, got:\n{source}"
    );
    assert!(
        source.contains("local int_cls = molt_builtin_type(int_tag)")
            && source.contains("local is_int = molt_isinstance(flag, int_cls)")
            && source.contains("local bool_is_int = molt_issubclass(bool_cls, int_cls)")
            && source.contains("local base_cls = {__molt_is_type = true}")
            && source.contains("local obj_is_base = molt_isinstance(obj, base_cls)"),
        "type-check ops should use named builtin/class metadata, got:\n{source}"
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
fn test_compile_checked_lowers_descriptor_attribute_authority() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "descriptor_attribute_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("method_func".to_string()),
                        s_value: Some("descriptor_method".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("class_func".to_string()),
                        s_value: Some("descriptor_class".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("static_func".to_string()),
                        s_value: Some("descriptor_static".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("get_func".to_string()),
                        s_value: Some("descriptor_get".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("set_func".to_string()),
                        s_value: Some("descriptor_set".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("del_func".to_string()),
                        s_value: Some("descriptor_del".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_new".to_string(),
                        out: Some("obj".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_set_class".to_string(),
                        args: Some(vec!["obj".to_string(), "cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "classmethod_new".to_string(),
                        out: Some("cm_desc".to_string()),
                        args: Some(vec!["class_func".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "staticmethod_new".to_string(),
                        out: Some("sm_desc".to_string()),
                        args: Some(vec!["static_func".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "property_new".to_string(),
                        out: Some("prop_desc".to_string()),
                        args: Some(vec![
                            "get_func".to_string(),
                            "set_func".to_string(),
                            "del_func".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec!["cls".to_string(), "method_func".to_string()]),
                        s_value: Some("method".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec!["cls".to_string(), "cm_desc".to_string()]),
                        s_value: Some("cm".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec!["cls".to_string(), "sm_desc".to_string()]),
                        s_value: Some("sm".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec!["cls".to_string(), "prop_desc".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        out: Some("cm_bound".to_string()),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("cm".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        out: Some("sm_func".to_string()),
                        args: Some(vec!["cls".to_string()]),
                        s_value: Some("sm".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        out: Some("prop_value".to_string()),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("value_name".to_string()),
                        s_value: Some("value".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "has_attr_name".to_string(),
                        out: Some("has_value".to_string()),
                        args: Some(vec!["obj".to_string(), "value_name".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("new_value".to_string()),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec!["obj".to_string(), "new_value".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "del_attr_generic_obj".to_string(),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("value".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_method".to_string(),
                        out: Some("method_result".to_string()),
                        args: Some(vec!["obj".to_string()]),
                        s_value: Some("method".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "builtin_func".to_string(),
                        out: Some("getattr_builtin".to_string()),
                        s_value: Some("molt_getattr_builtin".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "builtin_func".to_string(),
                        out: Some("setattr_builtin".to_string()),
                        s_value: Some("molt_set_attr_name".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "builtin_func".to_string(),
                        out: Some("delattr_builtin".to_string()),
                        s_value: Some("molt_del_attr_name".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "builtin_func".to_string(),
                        out: Some("hasattr_builtin".to_string()),
                        s_value: Some("molt_has_attr_name".to_string()),
                        ..OpIR::default()
                    },
                ],
            },
            FunctionIR {
                name: "descriptor_method".to_string(),
                params: vec!["self".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
            FunctionIR {
                name: "descriptor_class".to_string(),
                params: vec!["cls".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
            FunctionIR {
                name: "descriptor_static".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
            FunctionIR {
                name: "descriptor_get".to_string(),
                params: vec!["self".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
            FunctionIR {
                name: "descriptor_set".to_string(),
                params: vec!["self".to_string(), "value".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
            FunctionIR {
                name: "descriptor_del".to_string(),
                params: vec!["self".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("descriptor ops should lower through Luau attribute authority");
    assert!(
        source.contains("local function molt_get_attr")
            && source.contains("local function molt_has_attr")
            && source.contains("local function molt_set_attr")
            && source.contains("local function molt_del_attr"),
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
        source.contains("molt_set_attr(cls, \"cm\", cm_desc)")
            && source.contains("local cm_bound = molt_get_attr(obj, \"cm\")")
            && source.contains("local sm_func = molt_get_attr(cls, \"sm\")")
            && source.contains("local prop_value = molt_get_attr(obj, \"value\")")
            && source.contains("local has_value = molt_has_attr(obj, value_name)")
            && source.contains("molt_set_attr(obj, \"value\", new_value)")
            && source.contains("molt_del_attr(obj, \"value\")")
            && source.contains(
                "local method_result; do local __method = molt_get_attr(obj, \"method\");"
            ),
        "attribute get/set/delete and method call should route through descriptor authority, got:\n{source}"
    );
    assert!(
            source.contains("local getattr_builtin = function(a, ...)")
                && source.contains("local value = molt_get_attr(a[1], a[2])")
                && source.contains("local setattr_builtin = function(a, ...) return molt_set_attr(a[1], a[2], a[3]) end")
                && source.contains("local delattr_builtin = function(a, ...) return molt_del_attr(a[1], a[2]) end")
                && source.contains("local hasattr_builtin = function(a, ...) return molt_has_attr(a[1], a[2]) end")
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
fn test_compile_checked_lowers_class_apply_set_name_authority() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "class_apply_set_name_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "func_new".to_string(),
                        out: Some("set_name_func".to_string()),
                        s_value: Some("descriptor_set_name".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("descriptor_cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec![
                            "descriptor_cls".to_string(),
                            "set_name_func".to_string(),
                        ]),
                        s_value: Some("__set_name__".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_new".to_string(),
                        out: Some("descriptor".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_set_class".to_string(),
                        args: Some(vec!["descriptor".to_string(), "descriptor_cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("owner_cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        args: Some(vec!["owner_cls".to_string(), "descriptor".to_string()]),
                        s_value: Some("field".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_apply_set_name".to_string(),
                        args: Some(vec!["owner_cls".to_string()]),
                        out: Some("none".to_string()),
                        ..OpIR::default()
                    },
                ],
            },
            FunctionIR {
                name: "descriptor_set_name".to_string(),
                params: vec!["self".to_string(), "owner".to_string(), "name".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("class_apply_set_name should lower through descriptor authority");
    assert!(
        source.contains("local function molt_class_apply_set_name")
            && source.contains("local entries = {}")
            && source.contains("local hook = molt_get_attr(value, \"__set_name__\")")
            && source.contains("if hook ~= nil then hook(cls, name) end"),
        "class_apply_set_name helper should snapshot and dispatch hooks, got:\n{source}"
    );
    assert!(
        source.contains("molt_set_attr(descriptor_cls, \"__set_name__\", set_name_func)")
            && source.contains("molt_set_attr(owner_cls, \"field\", descriptor)")
            && source.contains("molt_class_apply_set_name(owner_cls)"),
        "dunder class attrs and apply op should share attribute authority, got:\n{source}"
    );
    assert!(
        !source.contains("[class op: class_apply_set_name]")
            && !source.contains("[unsupported op: class_apply_set_name]")
            && !source.contains("All other dunders"),
        "class_apply_set_name must not leave stale stub/no-op markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_rejects_internal_marker() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "internal_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![OpIR {
                kind: "function_closure_bits".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("compile_checked must reject internal stub markers");
    assert!(
        err.contains("semantic stub marker"),
        "error should mention semantic stub marker, got: {err}"
    );
}

#[test]
fn test_compile_checked_lowers_bridge_unavailable_to_runtime_error() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "bridge_unavailable_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("message".to_string()),
                    s_value: Some("dynamic bridge disabled".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "bridge_unavailable".to_string(),
                    out: Some("v0".to_string()),
                    args: Some(vec!["message".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("bridge_unavailable must lower to a checked runtime error");
    assert!(
        source.contains("local v0: any = error({__type=\"RuntimeError\""),
        "bridge_unavailable should be a terminal RuntimeError expression, got:\n{source}"
    );
    assert!(
        source.contains("Molt bridge unavailable: "),
        "diagnostic should match runtime bridge-unavailable prefix, got:\n{source}"
    );
    assert!(
        !source.contains("[bridge_unavailable]"),
        "bridge_unavailable must not leave a semantic stub marker, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_invoke_ffi_to_luau_capability_error() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "invoke_ffi_capability_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![OpIR {
                kind: "invoke_ffi".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("invoke_ffi should lower to a Luau target capability error");
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
fn test_compile_checked_lowers_object_set_class_metatable() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "object_set_class_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "object_new".to_string(),
                    out: Some("obj".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_new".to_string(),
                    out: Some("cls".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_set_class".to_string(),
                    args: Some(vec!["obj".to_string(), "cls".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("object_set_class must lower to Luau metatable assignment");
    assert!(
        source.contains("setmetatable(obj, cls)"),
        "object_set_class should bind the object to its class metatable, got:\n{source}"
    );
    assert!(
        !source.contains("[class op: object_set_class]"),
        "object_set_class must not be reported as a class-op marker, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_class_layout_metadata() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "class_layout_metadata_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "class_new".to_string(),
                    out: Some("cls".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_layout_version".to_string(),
                    out: Some("version_before".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("field_name".to_string()),
                    s_value: Some("field".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("field_offset".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("offsets".to_string()),
                    args: Some(vec!["field_name".to_string(), "field_offset".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("layout_size".to_string()),
                    value: Some(24),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_merge_layout".to_string(),
                    args: Some(vec![
                        "cls".to_string(),
                        "offsets".to_string(),
                        "layout_size".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("layout_version".to_string()),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_set_layout_version".to_string(),
                    args: Some(vec!["cls".to_string(), "layout_version".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_layout_version".to_string(),
                    out: Some("version_after".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("class layout metadata ops must lower to Luau class-table metadata");
    assert!(
        source.contains("local version_before = if type(cls) == \"table\""),
        "class_layout_version should read class-table layout metadata, got:\n{source}"
    );
    assert!(
        source.contains("__cls.__molt_field_offsets__ = __merged")
            && source.contains("__cls.__molt_layout_size__ = __layout_size"),
        "class_merge_layout should maintain field offsets and layout size, got:\n{source}"
    );
    assert!(
        source.contains("__cls.__molt_layout_version = __version"),
        "class_set_layout_version should write layout version metadata, got:\n{source}"
    );
    assert!(
        !source.contains("[class op: class_layout_version]")
            && !source.contains("[class op: class_set_layout_version]")
            && !source.contains("[class op: class_merge_layout]"),
        "layout metadata ops must not leave class-op markers, got:\n{source}"
    );
}

#[test]
fn test_default_luau_dispatch_uses_checked_path() {
    // Verify that both compile_via_ir and compile_checked reject the same unknown ops.
    // This ensures no fail-open path exists.
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dispatch_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![OpIR {
                kind: "unknown_luau_op".to_string(),
                out: Some("v0".to_string()),
                args: Some(vec!["v1".to_string(), "v2".to_string()]),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend_ir = LuauBackend::new();
    let mut backend_checked = LuauBackend::new();
    let err_ir = backend_ir
        .compile_via_ir(&ir)
        .expect_err("compile_via_ir must reject unknown ops");
    let err_checked = backend_checked
        .compile_checked(&ir)
        .expect_err("compile_checked must reject unknown ops");
    assert_eq!(
        err_ir, err_checked,
        "compile_via_ir and compile_checked must produce identical errors"
    );
}

#[test]
fn test_luau_repr_authority_typed_list_call_method_dispatch() {
    // Structured TIR facts, not legacy transport hints, authorize direct
    // list-method lowering for Luau tables.
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "append_to".to_string(),
            params: vec!["xs".to_string(), "v".to_string()],
            param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("append".to_string()),
                    args: Some(vec!["xs".to_string(), "v".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    // Must use direct table insertion, not xs:append(v).
    assert!(
        output.contains("xs[#xs + 1] = v"),
        "Expected table insert for list param, got:\n{output}"
    );
    assert!(
        !output.contains("xs:append"),
        "Must NOT emit method call for list.append(), got:\n{output}"
    );
}
