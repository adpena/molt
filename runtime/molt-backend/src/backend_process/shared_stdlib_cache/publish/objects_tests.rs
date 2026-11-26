use super::*;
use molt_backend::{FunctionIR, NativeBackendModuleContext, OpIR, SimpleIR};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

fn cache() -> NativeObjectCache {
    NativeObjectCache {
        cache_root: std::env::temp_dir().join(format!(
            "molt-stdlib-objects-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        )),
        authority: serde_json::json!({"compiler_fingerprint": "compiler-a", "cache_variant": "profile=dev;codegen_env=a", "executing_backend_sha256": "binary-a", "effective_codegen": "isa-a"}),
        executable: std::env::current_exe().unwrap(),
        stamp: ExecutableStamp::of(&std::env::current_exe().unwrap()).unwrap(),
    }
}

#[test]
fn a_backend_replaced_after_its_key_was_bound_publishes_nothing() {
    // HF-852: workers run the executable path; a rebuild that replaces it
    // mid-build emits another binary's bytes under the old binary's key.
    let mut cache = cache();
    std::fs::create_dir_all(&cache.cache_root).unwrap();
    let executable = cache.cache_root.join("molt-backend");
    std::fs::write(&executable, b"binary a").unwrap();
    cache.stamp = ExecutableStamp::of(&executable).unwrap();
    cache.executable = executable.clone();
    let key = cache.input_key(&job()).unwrap();
    let source = cache.cache_root.join("source.o");
    let bytes = object_bytes("original");
    std::fs::write(&source, &bytes).unwrap();
    let origin = worker_origin(&bytes, 101);
    // A rebuild writes a new file at the same path.
    std::fs::remove_file(&executable).unwrap();
    std::fs::write(&executable, b"binary b, rebuilt").unwrap();
    assert!(!cache.executable_unchanged().unwrap());
    assert_eq!(cache.publish(&key, &source, &origin).unwrap(), origin);
    assert!(cache.admit(&key).unwrap().is_none(), "nothing was cached");
    std::fs::remove_dir_all(cache.cache_root).unwrap();
}

fn job() -> NativeBatchObjectJob {
    let mut job = NativeBatchObjectJob {
        code_slots: None,
        ir: SimpleIR {
            // A batch document declares the row its body references (HF-847).
            functions: vec![
                FunctionIR {
                    name: "reader".into(),
                    ops: vec![OpIR {
                        s_value: Some("target".into()),
                        value: Some(0),
                        ..OpIR::new(molt_ir::OpKind::FuncNew)
                    }],
                    ..FunctionIR::default()
                },
                FunctionIR {
                    name: "target".into(),
                    is_extern: true,
                    ..FunctionIR::default()
                },
            ],
            profile: None,
        },
        module_context: NativeBackendModuleContext::default(),
        codegen_environment: molt_ir::backend_environment::NativeCodegenEnvironment::from_lookup(
            |_| Ok(None),
        )
        .unwrap(),
        target_triple: None,
        emit_app_callable_resolver: false,
        app_callable_manifest: None,
        external_function_names: BTreeSet::from(["target".into()]),
        module_registry: None,
    };
    // A job carries a decoded document (HF-847 slice 1).
    job.ir.resolve_function_references().unwrap();
    job
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
    let provider = FunctionIR {
        name: "target".into(),
        params: vec!["arg".into()],
        ..Default::default()
    };
    let abi = molt_ir::linkage_abi::NativeFunctionLinkageAbi {
        source_signature: provider.function_signature().unwrap(),
        parameter_custody: vec![molt_backend::ir::ParameterCustody::Transferred],
        param_types: vec![molt_backend::tir::types::TirType::DynBox],
        return_type: Some(molt_backend::tir::types::TirType::DynBox),
        return_availability: Default::default(),
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
    changed = roundtrip(&original);
    changed.emit_app_callable_resolver = true;
    changed.code_slots = Some(Default::default());
    let metadata_key = cache.input_key(&changed).unwrap();
    assert_ne!(original_key, metadata_key);
    changed.code_slots = Some(serde_json::from_value(serde_json::json!({"reader":0})).unwrap());
    assert_ne!(
        metadata_key,
        cache.input_key(&changed).unwrap(),
        "program code slots own their object identity"
    );
    changed.module_registry = Some(molt_backend::ModuleRegistryIR {
        blob: vec![0; 8],
        rows: vec![molt_ir::ir::ModuleRegistryRowIR {
            name: "package".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let registry_key = cache.input_key(&changed).unwrap();
    changed.module_registry.as_mut().unwrap().blob[0] = 1;
    assert_ne!(
        registry_key,
        cache.input_key(&changed).unwrap(),
        "registry blob changes must reemit metadata"
    );
    let registry_key = cache.input_key(&changed).unwrap();
    changed.module_registry.as_mut().unwrap().rows[0].static_cells = Some(2);
    assert_ne!(
        registry_key,
        cache.input_key(&changed).unwrap(),
        "layout changes must reemit metadata"
    );
    for field in [
        "compiler_fingerprint",
        "cache_variant",
        "executing_backend_sha256",
        "effective_codegen",
    ] {
        let mut changed_authority = cache.authority.clone();
        changed_authority[field] = serde_json::json!("changed");
        let other = NativeObjectCache {
            cache_root: cache.cache_root.clone(),
            authority: changed_authority,
            executable: cache.executable.clone(),
            stamp: cache.stamp,
        };
        assert_ne!(
            original_key,
            other.input_key(&original).unwrap(),
            "missing authority input {field}"
        );
    }
    let mut context = base_context;
    context["function_linkage_abis"] = serde_json::json!({"unreferenced_guest": abi});
    let unrelated =
        roundtrip(&original).close_dependencies(&serde_json::from_value(context).unwrap());
    assert_eq!(original_key, cache.input_key(&unrelated).unwrap());
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

fn worker_origin(bytes: &[u8], process_id: u32) -> NativeWorkerOrigin {
    use crate::backend_process::input::ContentIdentity;
    NativeWorkerOrigin {
        schema: "molt.native-worker-origin.v2".into(),
        process_id,
        scope: Some("a".repeat(64)),
        input: ContentIdentity::of_bytes(b"retained worker job"),
        output: ContentIdentity::of_bytes(bytes),
    }
}

#[test]
fn constituent_publication_roundtrips_rejects_corruption_and_preserves_prior_generation() {
    let cache = cache();
    std::fs::create_dir_all(&cache.cache_root).unwrap();
    let source = cache.cache_root.join("source.o");
    let key = cache.input_key(&job()).unwrap();
    assert!(cache.admit(&key).unwrap().is_none());
    let bytes = object_bytes("original");
    std::fs::write(&source, &bytes).unwrap();
    let origin = worker_origin(&bytes, 101);
    assert_eq!(cache.publish(&key, &source, &origin).unwrap(), origin);
    let entry = cache.path(&key);
    let owner = entry.parent().unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86400);
    let mut options = std::fs::File::options();
    #[cfg(not(windows))]
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES,
        };
        options
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let directory = options.open(owner).unwrap();
    directory.set_modified(old).unwrap();
    let aged = owner.metadata().unwrap().modified().unwrap();
    let stored_modified = entry.metadata().unwrap().modified().unwrap();
    let (admitted, retained) = cache.admit(&key).unwrap().expect("published generation");
    assert_eq!(retained, origin);
    assert_eq!(admitted.as_ref(), bytes.as_slice());
    assert_eq!(admitted.len(), bytes.len() as u64);
    let reused = owner.metadata().unwrap().modified().unwrap();
    assert!(
        reused > old,
        "admitted reuse must refresh its eviction owner"
    );
    assert_eq!(
        entry.metadata().unwrap().modified().unwrap(),
        stored_modified
    );
    assert!(cache.admit(&key).unwrap().is_some());
    assert_eq!(owner.metadata().unwrap().modified().unwrap(), reused);
    #[cfg(unix)]
    {
        // The mapping owns the admitted bytes: retention may unlink the entry
        // while assembly still reads them.
        let (held, retained) = cache.admit(&key).unwrap().expect("held generation");
        assert_eq!(retained, origin);
        let stored = std::fs::read(&entry).unwrap();
        std::fs::remove_file(&entry).unwrap();
        assert_eq!(held.as_ref(), bytes.as_slice());
        assert!(cache.admit(&key).unwrap().is_none());
        std::fs::write(&entry, stored).unwrap();
    }
    directory.set_modified(old).unwrap();
    let stored = std::fs::read(cache.path(&key)).unwrap();
    let different = object_bytes("different");
    std::fs::write(&source, &different).unwrap();
    let changed_origin = worker_origin(&different, 505);
    let disagreement = cache
        .publish(&key, &source, &changed_origin)
        .unwrap_err()
        .to_string();
    assert!(disagreement.contains("different output"));
    assert!(disagreement.contains(&bytes_to_lower_hex(&key)));
    assert!(disagreement.contains(&cache.path(&key).display().to_string()));
    assert!(disagreement.contains("first_difference="));
    assert!(disagreement.contains(&changed_origin.input.sha256));
    // The refused emission is kept, byte for byte, outside the entry.
    let digest: [u8; 32] = Sha256::digest(&different).into();
    let refused = cache.refused_object_path(&key, &digest);
    assert!(disagreement.contains(&refused.display().to_string()));
    assert_eq!(std::fs::read(&refused).unwrap(), different);
    assert_eq!(std::fs::read(cache.path(&key)).unwrap(), stored);
    std::fs::write(&source, b"invalid object").unwrap();
    assert!(cache.publish(&key, &source, &origin).is_err());
    assert_eq!(std::fs::read(cache.path(&key)).unwrap(), stored);
    let mut corrupt = stored;
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(cache.path(&key), corrupt).unwrap();
    assert!(
        cache
            .admit(&key)
            .err()
            .expect("corrupt generation is an error, not a miss")
            .to_string()
            .contains("digest/extent")
    );
    // Failed admission is not reuse.
    assert_eq!(owner.metadata().unwrap().modified().unwrap(), aged);
    drop(directory);
    std::fs::remove_dir_all(cache.cache_root).unwrap();
}

#[test]
fn archive_key_binds_every_ordered_constituent_input() {
    let first = [1u8; 32];
    let second = [2u8; 32];
    let key = NativeObjectCache::archive_key(&[first, second]);
    assert_eq!(key, NativeObjectCache::archive_key(&[first, second]));
    assert_ne!(key, NativeObjectCache::archive_key(&[second, first]));
    assert_ne!(key, NativeObjectCache::archive_key(&[first]));
    assert_ne!(
        key,
        NativeObjectCache::archive_key(&[first, second, second])
    );
    let cache = cache();
    let content = cache.content_archive(&key).unwrap();
    assert_eq!(content.key.len(), 64);
    assert!(
        content.path.ends_with(
            std::path::Path::new("native-stdlib-archives-v1")
                .join(&content.key)
                .join("stdlib.a")
        )
    );
    // The manifest is the program-independent constituent authority.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&content.manifest).unwrap(),
        cache.authority
    );
}

#[test]
fn constituent_key_preserves_float_bits_and_function_order() {
    let cache = cache();
    let mut first = job();
    first.ir.functions[0].ops.push(OpIR {
        f_value: Some(0.0),
        ..OpIR::new(molt_ir::OpKind::ConstFloat)
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
    use molt_ir::backend_environment::{BackendEnv, NativeCodegenEnvironment};
    let cache = cache();
    let mut original = job();
    original.codegen_environment = NativeCodegenEnvironment::from_lookup(|_| Ok(None)).unwrap();
    let key = cache.input_key(&original).unwrap();
    for (name, value) in [
        (BackendEnv::MOLT_DISABLE_SPLIT_FIELD_DEFOREST, ""),
        (BackendEnv::MOLT_BACKEND_ENABLE_VERIFIER, "1"),
    ] {
        let mut changed = roundtrip(&original);
        changed.codegen_environment = NativeCodegenEnvironment::from_lookup(|setting| {
            Ok((setting == name).then(|| value.to_string()))
        })
        .unwrap();
        assert_ne!(
            key,
            cache.input_key(&changed).unwrap(),
            "omitted actual codegen control {}",
            name.name()
        );
    }
}

#[test]
fn identical_constituent_publication_retains_original_worker_and_scope() {
    let cache = cache();
    std::fs::create_dir_all(&cache.cache_root).unwrap();
    let source = cache.cache_root.join("source.o");
    let bytes = object_bytes("same");
    std::fs::write(&source, &bytes).unwrap();
    let key = cache.input_key(&job()).unwrap();
    let first = worker_origin(&bytes, 101);
    let mut later = worker_origin(&bytes, 202);
    later.scope = Some("b".repeat(64));
    assert_eq!(cache.publish(&key, &source, &first).unwrap(), first);
    let before = std::fs::read(cache.path(&key)).unwrap();
    assert_eq!(cache.publish(&key, &source, &later).unwrap(), first);
    assert_eq!(std::fs::read(cache.path(&key)).unwrap(), before);
    let (mapped, retained) = cache.admit(&key).unwrap().unwrap();
    assert_eq!(mapped.as_ref(), bytes.as_slice());
    assert_eq!(retained, first);
    drop(mapped);
    std::fs::remove_dir_all(cache.cache_root).unwrap();
}

#[test]
fn unsupported_constituent_generation_is_replaced_without_relabeling_old_bytes() {
    let cache = cache();
    std::fs::create_dir_all(&cache.cache_root).unwrap();
    let source = cache.cache_root.join("source.o");
    let bytes = object_bytes("replacement");
    std::fs::write(&source, &bytes).unwrap();
    let key = cache.input_key(&job()).unwrap();
    let path = cache.path(&key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = b"MOLTNO01retired bytes-only generation";
    std::fs::write(&path, old).unwrap();
    assert!(cache.admit(&key).unwrap().is_none());
    assert_eq!(std::fs::read(&path).unwrap(), old);
    let origin = worker_origin(&bytes, 303);
    assert_eq!(cache.publish(&key, &source, &origin).unwrap(), origin);
    let (mapped, retained) = cache.admit(&key).unwrap().unwrap();
    assert_eq!(mapped.as_ref(), bytes.as_slice());
    assert_eq!(retained, origin);
    drop(mapped);
    std::fs::remove_dir_all(cache.cache_root).unwrap();
}

#[test]
fn constituent_origin_substitution_and_unbounded_extent_are_refused() {
    let cache = cache();
    std::fs::create_dir_all(&cache.cache_root).unwrap();
    let source = cache.cache_root.join("source.o");
    let bytes = object_bytes("original");
    std::fs::write(&source, &bytes).unwrap();
    let key = cache.input_key(&job()).unwrap();
    let origin = worker_origin(&bytes, 404);
    let wrong = worker_origin(&object_bytes("other"), 404);
    assert!(cache.publish(&key, &source, &wrong).is_err());
    assert!(!cache.path(&key).exists());
    cache.publish(&key, &source, &origin).unwrap();
    let original = std::fs::read(cache.path(&key)).unwrap();
    let mut mutated = original.clone();
    mutated[80..88].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(cache.path(&key), mutated).unwrap();
    assert!(cache.admit(&key).is_err());
    let mut mutated = original;
    let length = u64::from_le_bytes(mutated[80..88].try_into().unwrap()) as usize;
    let mut record: serde_json::Value =
        serde_json::from_slice(&mutated[HEADER_SIZE..HEADER_SIZE + length]).unwrap();
    record["output"]["sha256"] = serde_json::json!("f".repeat(64));
    let replacement = serde_json::to_vec(&record).unwrap();
    mutated.splice(
        HEADER_SIZE..HEADER_SIZE + length,
        replacement.iter().copied(),
    );
    mutated[80..88].copy_from_slice(&(replacement.len() as u64).to_le_bytes());
    std::fs::write(cache.path(&key), mutated).unwrap();
    assert!(cache.admit(&key).is_err());
    std::fs::remove_dir_all(cache.cache_root).unwrap();
}
