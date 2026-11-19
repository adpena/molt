use super::*;

#[test]
fn native_backend_preserves_split_stub_calls_to_void_and_value_chunks() {
    let chunk0 = "__molt_chunk_demo__molt_module_chunk_1_0".to_string();
    let chunk1 = "__molt_chunk_demo__molt_module_chunk_1_1".to_string();
    let stub = "demo__molt_module_chunk_1".to_string();
    let compiled = compile_function_to_clif_with_imports(
        vec![
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: chunk0,
                params: vec![],
                ops: vec![OpIR {
                    ..OpIR::new(OpKind::RetVoid)
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: chunk1,
                params: vec![],
                ops: vec![
                    OpIR {
                        out: Some("chunk_ret".to_string()),
                        value: Some(7),
                        ..OpIR::new(OpKind::Const)
                    },
                    OpIR {
                        args: Some(vec!["chunk_ret".to_string()]),
                        ..OpIR::new(OpKind::Ret)
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: stub.clone(),
                params: vec![],
                ops: vec![
                    OpIR {
                        s_value: Some("__molt_chunk_demo__molt_module_chunk_1_0".to_string()),
                        out: Some("__chunk_discard_0".to_string()),
                        ..OpIR::new(OpKind::CallInternal)
                    },
                    OpIR {
                        s_value: Some("__molt_chunk_demo__molt_module_chunk_1_1".to_string()),
                        out: Some("__chunk_ret".to_string()),
                        ..OpIR::new(OpKind::CallInternal)
                    },
                    OpIR {
                        args: Some(vec!["__chunk_ret".to_string()]),
                        ..OpIR::new(OpKind::Ret)
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        &stub,
    );
    // A local callee is any callee that is not a runtime import: every call
    // from object code is colocated now, so colocation no longer tells them
    // apart (code_owner.rs `declare_func_in_func`).
    let imports: std::collections::BTreeSet<u32> =
        compiled.import_ids.values().map(|id| id.as_u32()).collect();
    let func = &compiled.function;
    let clif = func.display().to_string();
    let local_callees: Vec<String> = func
        .dfg
        .ext_funcs
        .iter()
        .filter_map(|(func_ref, data)| match &data.name {
            cranelift_codegen::ir::ExternalName::User(name) => {
                let index = func.params.user_named_funcs()[*name].index;
                (!imports.contains(&index)).then(|| func_ref.to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        local_callees.len(),
        2,
        "stub CLIF should reference exactly two local chunk callees:\n{clif}",
    );
    assert!(
        local_callees
            .iter()
            .any(|callee| clif.contains(&format!("call {callee}("))),
        "split stub must retain the direct call to the first void-returning chunk:\n{clif}",
    );
    assert!(
        local_callees
            .iter()
            .any(|callee| clif.contains(&format!("= call {callee}("))),
        "split stub must retain the direct call to the final value-returning chunk:\n{clif}",
    );
}

#[test]
fn native_backend_compiles_split_local_frame_with_inherited_chunks() {
    let mut ops = vec![
        OpIR {
            s_value: Some("native_framed_large".to_string()),
            ..OpIR::new(OpKind::TraceEnterSlot)
        },
        OpIR {
            value: Some(1),
            ..OpIR::new(OpKind::CheckException)
        },
    ];
    for line in 1..=6 {
        ops.push(OpIR {
            value: Some(line),
            ..OpIR::new(OpKind::Line)
        });
        ops.push(OpIR {
            out: Some(format!("v{line}")),
            ..OpIR::new(OpKind::ConstNone)
        });
    }
    ops.extend([
        OpIR {
            ..OpIR::new(OpKind::TraceExit)
        },
        OpIR {
            ..OpIR::new(OpKind::RetVoid)
        },
        OpIR {
            value: Some(1),
            ..OpIR::new(OpKind::Label)
        },
        OpIR {
            ..OpIR::new(OpKind::TraceExit)
        },
        OpIR {
            ..OpIR::new(OpKind::RetVoid)
        },
    ]);
    let original = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "native_framed_large".to_string(),
        ops,
        execution_context: crate::ir::ExecutionContextPolicy::Local,
        ..FunctionIR::default()
    };
    let mut occupied = BTreeSet::from([original.name.clone()]);
    let (stub, chunks) = crate::passes::split_large_function(original, 3, &mut occupied).unwrap();
    let stub_name = stub.name.clone();
    let mut functions = std::iter::once(stub).chain(chunks).collect::<Vec<_>>();
    molt_ir::function_table::resolve_function_references(&mut functions).unwrap();
    crate::validate_simple_ir(&SimpleIR {
        functions: functions.clone(),
        profile: None,
    })
    .unwrap();
    let object = {
        let _guard = acquire_backend_env_lock();
        let _trace_env = ScopedEnvVar::set("MOLT_BACKEND_EMIT_TRACES", Some("0"));
        SimpleBackend::new()
            .compile(SimpleIR {
                functions: functions.clone(),
                profile: None,
            })
            .bytes
    };
    for symbol in [
        b"molt_frame_record_enter".as_slice(),
        b"molt_frame_record_exit".as_slice(),
    ] {
        assert!(
            object.windows(symbol.len()).any(|window| window == symbol),
            "trace-enabled native object is missing {}",
            String::from_utf8_lossy(symbol)
        );
    }

    let clif = compile_function_to_clif_text(functions, &stub_name);
    assert!(clif.matches("call fn").count() >= 2, "{clif}");
}
