use super::*;
use molt_backend::{FunctionIR, NativeBackendModuleContext, OpIR, SimpleIR};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

fn cache() -> StdlibObjectCache {
    StdlibObjectCache {
        root: std::env::temp_dir().join(format!(
            "molt-stdlib-objects-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        )),
        authority: serde_json::json!({"compiler_fingerprint": "compiler-a", "cache_variant": "profile=dev;codegen_env=a", "executing_backend_sha256": "binary-a", "effective_codegen": "isa-a"}),
    }
}

fn job() -> NativeBatchObjectJob {
    NativeBatchObjectJob {
        ir: SimpleIR {
            functions: vec![FunctionIR {
                name: "reader".into(),
                ops: vec![OpIR {
                    kind: "func_new".into(),
                    s_value: Some("target".into()),
                    value: Some(0),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            }],
            profile: None,
        },
        module_context: NativeBackendModuleContext::default(),
        codegen_environment: molt_ir::backend_environment::NativeCodegenEnvironment::capture()
            .unwrap(),
        target_triple: None,
        emit_app_callable_resolver: false,
        app_callable_manifest: None,
        external_function_names: BTreeSet::from(["target".into()]),
        module_registry: None,
    }
}

fn roundtrip(job: &NativeBatchObjectJob) -> NativeBatchObjectJob {
    serde_json::from_value(serde_json::to_value(job).unwrap()).unwrap()
}

#[test]
fn complete_job_key_tracks_every_context_family_and_compilation_input() {
    let cache = cache();
    let original = job();
    let original_key = cache.input_key(&original).unwrap();
    let base_context = serde_json::to_value(&original.module_context).unwrap();
    let changes = [
        ("partition_sources", serde_json::json!({"reader": "source"})),
        ("function_arities", serde_json::json!({"target": 2})),
        ("function_has_ret", serde_json::json!({"target": false})),
        ("closure_functions", serde_json::json!(["target"])),
        ("task_kinds", serde_json::json!({"target": "Generator"})),
        ("task_closure_sizes", serde_json::json!({"target": 48})),
    ];
    for (field, value) in changes {
        let mut context = base_context.clone();
        context[field] = value;
        let mut changed = roundtrip(&original);
        changed.module_context = serde_json::from_value(context).unwrap();
        let changed = changed.close_dependencies();
        assert_ne!(
            original_key,
            cache.input_key(&changed).unwrap(),
            "missing context key input {field}"
        );
    }
    let provider = FunctionIR {
        name: "target".into(),
        params: vec!["arg".into()],
        ..Default::default()
    };
    let abi = molt_backend_native::NativeFunctionLinkageAbi {
        source_signature: provider.function_signature().unwrap(),
        parameter_custody: vec![molt_backend::ir::ParameterCustody::Transferred],
        param_types: vec![molt_backend::tir::types::TirType::DynBox],
        return_type: Some(molt_backend::tir::types::TirType::DynBox),
    };
    let mut context = base_context.clone();
    context["function_linkage_abis"] = serde_json::json!({"target": abi});
    let mut with_abi = roundtrip(&original);
    with_abi.module_context = serde_json::from_value(context.clone()).unwrap();
    let abi_key = cache.input_key(&with_abi).unwrap();
    assert_ne!(original_key, abi_key);
    context["function_linkage_abis"]["target"]["parameter_custody"] = serde_json::json!([]);
    with_abi.module_context = serde_json::from_value(context).unwrap();
    assert_ne!(
        abi_key,
        cache.input_key(&with_abi).unwrap(),
        "entry custody must invalidate same-body objects"
    );
    let mut changed = roundtrip(&original);
    changed.ir.functions[0].ops[0].value = Some(1);
    assert_ne!(original_key, cache.input_key(&changed).unwrap());
    changed = roundtrip(&original);
    changed.target_triple = Some("aarch64-unknown-linux-gnu".into());
    assert_ne!(original_key, cache.input_key(&changed).unwrap());
    changed = roundtrip(&original);
    changed.ir.profile = Some(molt_backend::PgoProfileIR {
        hot_functions: vec!["reader".into()],
        ..Default::default()
    });
    assert_ne!(original_key, cache.input_key(&changed).unwrap());
    changed = roundtrip(&original);
    changed.external_function_names.clear();
    assert_ne!(original_key, cache.input_key(&changed).unwrap());
    for field in [
        "compiler_fingerprint",
        "cache_variant",
        "executing_backend_sha256",
        "effective_codegen",
    ] {
        let mut changed_authority = cache.authority.clone();
        changed_authority[field] = serde_json::json!("changed");
        let other = StdlibObjectCache {
            root: cache.root.clone(),
            authority: changed_authority,
        };
        assert_ne!(
            original_key,
            other.input_key(&original).unwrap(),
            "missing authority input {field}"
        );
    }
    let mut unrelated = roundtrip(&original);
    let mut context = base_context;
    context["function_arities"] = serde_json::json!({"unreferenced_guest": 19});
    unrelated.module_context = serde_json::from_value(context).unwrap();
    assert_eq!(
        original_key,
        cache.input_key(&unrelated.close_dependencies()).unwrap()
    );
}

fn object_bytes(symbol: &str) -> Vec<u8> {
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };
    let mut object =
        object::write::Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(section, &[0xc3], 1);
    object.add_symbol(object::write::Symbol {
        name: symbol.as_bytes().to_vec(),
        value: 0,
        size: 1,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: object::write::SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    object.write().unwrap()
}

#[test]
fn constituent_publication_roundtrips_rejects_corruption_and_preserves_prior_generation() {
    let cache = cache();
    std::fs::create_dir_all(&cache.root).unwrap();
    let source = cache.root.join("source.o");
    let restored = cache.root.join("restored.o");
    let key = cache.input_key(&job()).unwrap();
    assert_eq!(cache.restore(&key, &restored).unwrap(), None);
    let bytes = object_bytes("original");
    std::fs::write(&source, &bytes).unwrap();
    assert_eq!(cache.publish(&key, &source).unwrap(), bytes.len() as u64);
    assert_eq!(
        cache.restore(&key, &restored).unwrap(),
        Some(bytes.len() as u64)
    );
    assert_eq!(std::fs::read(&restored).unwrap(), bytes);
    let stored = std::fs::read(cache.path(&key)).unwrap();
    std::fs::write(&source, object_bytes("different")).unwrap();
    assert!(
        cache
            .publish(&key, &source)
            .unwrap_err()
            .to_string()
            .contains("different output")
    );
    assert_eq!(std::fs::read(cache.path(&key)).unwrap(), stored);
    std::fs::write(&source, b"invalid object").unwrap();
    assert!(cache.publish(&key, &source).is_err());
    assert_eq!(std::fs::read(cache.path(&key)).unwrap(), stored);
    let mut corrupt = stored;
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(cache.path(&key), corrupt).unwrap();
    assert!(
        cache
            .restore(&key, &restored)
            .unwrap_err()
            .to_string()
            .contains("digest/extent")
    );
    // Failed admission must not overwrite the caller's existing private object.
    assert_eq!(std::fs::read(&restored).unwrap(), bytes);
    std::fs::remove_dir_all(cache.root).unwrap();
}

#[test]
fn constituent_key_preserves_float_bits_and_function_order() {
    let cache = cache();
    let mut first = job();
    first.ir.functions[0].ops.push(OpIR {
        kind: "const_float".into(),
        f_value: Some(0.0),
        ..OpIR::default()
    });
    let mut second = roundtrip(&first);
    second.ir.functions[0].ops.last_mut().unwrap().f_value = Some(-0.0);
    assert_ne!(
        cache.input_key(&first).unwrap(),
        cache.input_key(&second).unwrap()
    );
    first.ir.functions.push(FunctionIR {
        name: "second".into(),
        ..Default::default()
    });
    second = roundtrip(&first);
    second.ir.functions.reverse();
    assert_ne!(
        cache.input_key(&first).unwrap(),
        cache.input_key(&second).unwrap()
    );
}

#[test]
fn constituent_key_binds_actual_emitter_and_presence_gated_pass_inputs() {
    use molt_ir::backend_environment::NativeCodegenEnvironment;
    let cache = cache();
    let mut original = job();
    original.codegen_environment = NativeCodegenEnvironment::from_lookup(|_| Ok(None)).unwrap();
    let key = cache.input_key(&original).unwrap();
    for (name, value) in [
        ("MOLT_BACKEND_INLINE_EXC_DISABLED", "1"),
        ("MOLT_DISABLE_RC_COALESCE", ""),
    ] {
        let mut changed = roundtrip(&original);
        changed.codegen_environment = NativeCodegenEnvironment::from_lookup(|setting| {
            Ok((setting == name).then(|| value.to_string()))
        })
        .unwrap();
        assert_ne!(
            key,
            cache.input_key(&changed).unwrap(),
            "omitted actual codegen control {name}"
        );
    }
}
