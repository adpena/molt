use super::*;
use crate::backend_process::input::ContentIdentity;
use molt_backend::FunctionIR;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

fn plan(cache: &NativeObjectCache) -> PlannedStdlibArchive {
    let functions = ["first", "second"]
        .into_iter()
        .map(|name| FunctionIR {
            name: name.into(),
            source_file: Some(format!("{name}.py")),
            ..Default::default()
        })
        .collect();
    plan_batched_stdlib_cache_archive(
        StdlibBatchPlan::from_functions(functions, NativeBackendModuleContext::default()),
        None,
        None,
        Some(cache),
    )
    .unwrap()
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

fn emit(job: &[u8], name: &str, output: &std::path::Path) -> io::Result<NativeWorkerOrigin> {
    let bytes = object_bytes(name);
    std::fs::write(output, &bytes)?;
    Ok(NativeWorkerOrigin {
        schema: "molt.native-worker-origin.v2".into(),
        process_id: std::process::id(),
        scope: Some("a".repeat(64)),
        input: ContentIdentity::of_bytes(job),
        output: ContentIdentity::of_bytes(&bytes),
    })
}

#[test]
fn failed_archive_retains_verified_constituents_and_retry_compiles_only_the_missing_job() {
    let _scope = molt_ir::backend_environment::enter(std::sync::Arc::default());
    let directory = std::env::temp_dir().join(format!(
        "molt-incremental-stdlib-publication-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let archive = directory.join("stdlib.a");
    let manifest = serde_json::json!({
        "schema": "stdlib-manifest-v2-archive",
        "artifact_kind": "archive",
        "cache_key": "program",
        "target_triple": null,
        "cache_variant": "test-profile",
        "compiler_fingerprint": "test-compiler",
    })
    .to_string();
    let cache = NativeObjectCache::for_archive(&archive, Some("program"), Some(&manifest), None)
        .unwrap()
        .unwrap();
    let planned = plan(&cache);
    assert_eq!(planned.batches.len(), 2);
    let keys: Vec<_> = planned
        .batches
        .iter()
        .map(|batch| batch.key.unwrap())
        .collect();
    let published = AtomicBool::new(false);
    let error = planned
        .assemble_with_worker(
            "test",
            Some(&cache),
            |_, _| {
                published.store(true, Ordering::SeqCst);
                Ok(())
            },
            |job, name, output| {
                if name == "batch_1.json" {
                    return Err(io::Error::other("second worker failed"));
                }
                emit(job, name, output)
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("second worker failed"));
    assert!(!published.load(Ordering::SeqCst));
    assert!(!archive.exists(), "incomplete archives are never published");
    let (retained, original_origin) = cache.admit(&keys[0]).unwrap().unwrap();
    assert_eq!(retained.as_ref(), object_bytes("batch_0.json"));
    assert!(cache.admit(&keys[1]).unwrap().is_none());
    drop(retained);
    let emitted = AtomicU64::new(0);
    plan(&cache)
        .assemble_with_worker(
            "retry",
            Some(&cache),
            |objects, provenance| {
                assert_eq!(objects.len(), 2);
                assert_eq!(objects[0].as_ref(), object_bytes("batch_0.json"));
                assert_eq!(objects[1].as_ref(), object_bytes("batch_1.json"));
                assert_eq!(provenance.workers[0], original_origin);
                assert_eq!(provenance.workers.len(), 2);
                published.store(true, Ordering::SeqCst);
                Ok(())
            },
            |job, name, output| {
                assert_eq!(name, "batch_1.json", "completed job must be reused");
                emitted.fetch_add(1, Ordering::SeqCst);
                emit(job, name, output)
            },
        )
        .unwrap();
    assert_eq!(emitted.load(Ordering::SeqCst), 1);
    assert!(published.load(Ordering::SeqCst));
    std::fs::remove_dir_all(directory).unwrap();
}
