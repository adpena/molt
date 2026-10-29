use super::*;

#[test]
fn call_argument_form_survives_tir_roundtrip() {
    for (wire, expected) in [
        (None, molt_ir::CallArgumentForm::Stack),
        (Some("expanded"), molt_ir::CallArgumentForm::Expanded),
    ] {
        let simple: FunctionIR = serde_json::from_value(serde_json::json!({
            "name": "call_form", "params": ["callee"], "return_abi": "value",
            "ops": [
                {"kind": "callargs_new", "out": "builder", "s_value": wire},
                {"kind": "call_bind", "out": "result", "args": ["callee", "builder"]},
                {"kind": "ret", "args": ["result"]}
            ]
        }))
        .unwrap();
        let tir = lower_to_tir(&simple);
        let builder = tir
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .find(|op| op.origin == Some(molt_ir::OpKind::CallargsNew))
            .expect("builder survives lifting");
        assert_eq!(builder.call_argument_form().unwrap(), expected);
        let lowered = lower_to_simple_ir(&tir);
        let builder = lowered
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::CallargsNew)
            .unwrap();
        assert_eq!(builder.call_argument_form().unwrap(), expected);
        assert_eq!(builder.s_value.as_deref(), wire);
    }
}

#[test]
fn builtin_roundtrip_uses_the_shared_named_and_dynamic_argument_contract() {
    for (origin, attrs, count, expected_kind, expected_name) in [
        (
            None,
            AttrDict::from([("name".into(), AttrValue::Str("len".into()))]),
            1,
            molt_ir::OpKind::CallBuiltin,
            Some("len"),
        ),
        (None, AttrDict::new(), 2, molt_ir::OpKind::CallBuiltin, None),
        (
            Some(molt_ir::OpKind::RangeNew),
            AttrDict::from([("name".into(), AttrValue::Str("range".into()))]),
            3,
            molt_ir::OpKind::RangeNew,
            Some("range"),
        ),
        (
            Some(molt_ir::OpKind::BuiltinPrint),
            AttrDict::new(),
            1,
            molt_ir::OpKind::BuiltinPrint,
            Some("print"),
        ),
    ] {
        let op = TirOp {
            origin,
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: (0..count).map(ValueId).collect(),
            results: vec![ValueId(4)],
            attrs,
            source_span: None,
        };
        let simple = super::super::op_lowering::lower_op_many(&op);
        assert_eq!(simple.len(), 1);
        assert_eq!(simple[0].kind, expected_kind);
        assert_eq!(simple[0].s_value.as_deref(), expected_name);
        assert_eq!(simple[0].args.as_ref().unwrap().len(), count as usize);
        assert_eq!(simple[0].args.as_ref().unwrap()[0], value_var(ValueId(0)));
    }
}

#[test]
#[should_panic(expected = "malformed CallBuiltin identity or argument contract")]
fn conflicting_builtin_identity_never_silently_disappears_in_roundtrip() {
    let op = TirOp {
        origin: Some(molt_ir::OpKind::Print),
        dialect: Dialect::Molt,
        opcode: OpCode::CallBuiltin,
        operands: vec![ValueId(0)],
        results: vec![ValueId(1)],
        attrs: AttrDict::from([("name".into(), AttrValue::Str("len".into()))]),
        source_span: None,
    };
    super::super::op_lowering::lower_op_many(&op);
}

#[test]
fn tir_round_trip_preserves_object_argument_call_sequence() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::passes::run_pipeline;
    use crate::tir::type_refine::refine_types;

    let callee_ir = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "func_objarg__g".into(),
        params: vec!["x".into()],
        ops: vec![
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some("x".into()),
                args: Some(vec!["x".into()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
            OpIR {
                kind: molt_ir::OpKind::Line,
                value: Some(5),
                col_offset: Some(4),
                end_col_offset: Some(18),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::TypeOf,
                args: Some(vec!["x".into()]),
                out: Some("v99".into()),
                ..OpIR::new(molt_ir::OpKind::TypeOf)
            },
            OpIR {
                kind: molt_ir::OpKind::Print,
                args: Some(vec!["v99".into()]),
                ..OpIR::new(molt_ir::OpKind::Print)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstNone,
                out: Some("v100".into()),
                ..OpIR::new(molt_ir::OpKind::ConstNone)
            },
            OpIR {
                kind: molt_ir::OpKind::Ret,
                args: Some(vec!["v100".into()]),
                ..OpIR::new(molt_ir::OpKind::Ret)
            },
        ],
        param_types: Some(vec!["i64".into()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let caller_ir = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "func_objarg__molt_module_chunk_1".into(),
        params: vec!["__molt_module_obj__".into()],
        ops: vec![
            OpIR {
                kind: molt_ir::OpKind::Line,
                value: Some(1),
                col_offset: Some(0),
                end_col_offset: Some(8),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::Const,
                value: Some(100),
                out: Some("v63".into()),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            OpIR {
                kind: molt_ir::OpKind::BuiltinType,
                args: Some(vec!["v63".into()]),
                out: Some("v64".into()),
                ..OpIR::new(molt_ir::OpKind::BuiltinType)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("C".into()),
                out: Some("v65".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("C".into()),
                out: Some("v66".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__main__".into()),
                out: Some("v67".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::Const,
                value: Some(1),
                out: Some("v68".into()),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__name__".into()),
                out: Some("v69".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__qualname__".into()),
                out: Some("v70".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__module__".into()),
                out: Some("v71".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__firstlineno__".into()),
                out: Some("v72".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ClassDef,
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
                ..OpIR::new(molt_ir::OpKind::ClassDef)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("C".into()),
                out: Some("v74".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleSetAttr,
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "v74".into(),
                    "v73".into(),
                ]),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleSetAttr)
            },
            OpIR {
                kind: molt_ir::OpKind::Line,
                value: Some(4),
                col_offset: Some(0),
                end_col_offset: Some(18),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::FuncNew,
                s_value: Some("func_objarg__g".into()),
                value: Some(1),
                out: Some("v75".into()),
                ..OpIR::new(molt_ir::OpKind::FuncNew)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("g".into()),
                out: Some("v76".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v76".into()]),
                s_value: Some("__name__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("g".into()),
                out: Some("v77".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v77".into()]),
                s_value: Some("__qualname__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("func_objarg".into()),
                out: Some("v78".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v78".into()]),
                s_value: Some("__module__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("x".into()),
                out: Some("v79".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::TupleNew,
                args: Some(vec!["v79".into()]),
                out: Some("v80".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::new(molt_ir::OpKind::TupleNew)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v80".into()]),
                s_value: Some("__molt_arg_names__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::Const,
                value: Some(0),
                out: Some("v81".into()),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v81".into()]),
                s_value: Some("__molt_posonly__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::TupleNew,
                args: Some(vec![]),
                out: Some("v82".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::new(molt_ir::OpKind::TupleNew)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v82".into()]),
                s_value: Some("__molt_kwonly_names__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstNone,
                out: Some("v83".into()),
                ..OpIR::new(molt_ir::OpKind::ConstNone)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__molt_vararg__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__molt_varkw__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__defaults__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__kwdefaults__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v83".into()]),
                s_value: Some("__doc__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("/tmp/func_objarg.py".into()),
                out: Some("v88".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::Const,
                value: Some(4),
                out: Some("v89".into()),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("g".into()),
                out: Some("v90".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("x".into()),
                out: Some("v92".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::TupleNew,
                args: Some(vec!["v92".into()]),
                out: Some("v93".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::new(molt_ir::OpKind::TupleNew)
            },
            OpIR {
                kind: molt_ir::OpKind::TupleNew,
                args: Some(vec![]),
                out: Some("v94".into()),
                type_hint: Some("tuple".into()),
                ..OpIR::new(molt_ir::OpKind::TupleNew)
            },
            OpIR {
                kind: molt_ir::OpKind::CodeNew,
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
                ..OpIR::new(molt_ir::OpKind::CodeNew)
            },
            OpIR {
                kind: molt_ir::OpKind::SetAttrGenericObj,
                args: Some(vec!["v75".into(), "v97".into()]),
                s_value: Some("__code__".into()),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::SetAttrGenericObj)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__dict__".into()),
                out: Some("code_globals_name".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleGetAttr,
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "code_globals_name".into(),
                ]),
                out: Some("code_globals".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
            },
            OpIR {
                kind: molt_ir::OpKind::CodeSlotSet,
                s_value: Some("func_objarg__g".into()),
                args: Some(vec!["v97".into(), "code_globals".into()]),
                ..OpIR::new(molt_ir::OpKind::CodeSlotSet)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("g".into()),
                out: Some("v98".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleSetAttr,
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "v98".into(),
                    "v75".into(),
                ]),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleSetAttr)
            },
            OpIR {
                kind: molt_ir::OpKind::Line,
                value: Some(7),
                col_offset: Some(0),
                end_col_offset: Some(7),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("C".into()),
                out: Some("v101".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleGetAttr,
                args: Some(vec!["__molt_module_obj__".into(), "v101".into()]),
                out: Some("v102".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::CallargsNew,
                out: Some("v103".into()),
                ..OpIR::new(molt_ir::OpKind::CallargsNew)
            },
            OpIR {
                kind: molt_ir::OpKind::CallBind,
                args: Some(vec!["v102".into(), "v103".into()]),
                out: Some("v104".into()),
                ..OpIR::new(molt_ir::OpKind::CallBind)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("c".into()),
                out: Some("v105".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleSetAttr,
                args: Some(vec![
                    "__molt_module_obj__".into(),
                    "v105".into(),
                    "v104".into(),
                ]),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleSetAttr)
            },
            OpIR {
                kind: molt_ir::OpKind::Line,
                value: Some(8),
                col_offset: Some(0),
                end_col_offset: Some(4),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::Call,
                s_value: Some("func_objarg__g".into()),
                args: Some(vec!["v104".into()]),
                out: Some("v106".into()),
                ..OpIR::new(molt_ir::OpKind::Call)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::Label)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionLast,
                out: Some("v107".into()),
                ..OpIR::new(molt_ir::OpKind::ExceptionLast)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstNone,
                out: Some("v108".into()),
                ..OpIR::new(molt_ir::OpKind::ConstNone)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstNone,
                out: Some("v108".into()),
                ..OpIR::new(molt_ir::OpKind::ConstNone)
            },
            OpIR {
                kind: molt_ir::OpKind::Is,
                args: Some(vec!["v107".into(), "v108".into()]),
                out: Some("v109".into()),
                ..OpIR::new(molt_ir::OpKind::Is)
            },
            OpIR {
                kind: molt_ir::OpKind::Not,
                args: Some(vec!["v109".into()]),
                out: Some("v110".into()),
                ..OpIR::new(molt_ir::OpKind::Not)
            },
            OpIR {
                kind: molt_ir::OpKind::If,
                args: Some(vec!["v110".into()]),
                ..OpIR::new(molt_ir::OpKind::If)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("func_objarg".into()),
                out: Some("v111".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleCacheDel,
                args: Some(vec!["v111".into()]),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleCacheDel)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("__main__".into()),
                out: Some("v112".into()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleCacheDel,
                args: Some(vec!["v112".into()]),
                out: Some("none".into()),
                ..OpIR::new(molt_ir::OpKind::ModuleCacheDel)
            },
            OpIR {
                kind: molt_ir::OpKind::EndIf,
                ..OpIR::new(molt_ir::OpKind::EndIf)
            },
            OpIR {
                kind: molt_ir::OpKind::RetVoid,
                ..OpIR::new(molt_ir::OpKind::RetVoid)
            },
        ],
        param_types: Some(vec!["i64".into()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    for func_ir in [callee_ir, caller_ir] {
        let mut tir_func = crate::tir::lower_from_simple::lower_to_tir_for_target(
            &func_ir,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        refine_types(&mut tir_func);
        run_pipeline(
            &mut tir_func,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        refine_types(&mut tir_func);
        let round_tripped = lower_to_simple_ir(&tir_func);

        if func_ir.name == "func_objarg__g" {
            let type_of = round_tripped
                .iter()
                .find(|op| op.kind == molt_ir::OpKind::TypeOf)
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
            assert_eq!(arg_op.kind, molt_ir::OpKind::CopyVar);
            assert_eq!(arg_op.var.as_deref(), Some("x"));
        } else {
            let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
                .iter()
                .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
                .collect();
            let call = round_tripped
                .iter()
                .find(|op| {
                    op.kind == molt_ir::OpKind::Call
                        && op.s_value.as_deref() == Some("func_objarg__g")
                })
                .expect("caller must preserve direct call to func_objarg__g");
            let call_args = call
                .args
                .as_ref()
                .expect("direct call must keep its argument");
            assert_eq!(call_args.len(), 1);
            let arg_op = producer_by_out
                .get(&call_args[0])
                .expect("direct call argument must come from an op");
            assert_eq!(arg_op.kind, molt_ir::OpKind::CallBind);
            assert_eq!(arg_op.s_value, None);
        }
    }
}
