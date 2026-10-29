use super::tests::BlobBuilder;
use super::*;

#[test]
fn namespace_catalog_preserves_portion_order_without_body_or_file_provenance() {
    let mut builder = BlobBuilder::new();
    builder
        .row("alias", 0, None, Some(1), MODULE_KIND_ALIAS, 0)
        .row(
            "pkg",
            0,
            None,
            None,
            MODULE_KIND_NAMESPACE_PARENT,
            MODULE_FLAG_PACKAGE,
        );
    let mut blob = builder.build();
    // Independent wire fixture: two length-prefixed directory strings.
    let portions = b"\x06\x00\x00\x00/a/pkg\x06\x00\x00\x00/b/pkg";
    blob[48..56].copy_from_slice(&(portions.len() as u64).to_le_bytes());
    let row = MODULE_REGISTRY_HEADER_BYTES + MODULE_REGISTRY_ROW_BYTES;
    blob[row + 40..row + 44].copy_from_slice(&(portions.len() as u32).to_le_bytes());
    blob.extend_from_slice(portions);
    let registry = ModuleRegistry::parse(blob.as_ptr()).unwrap();
    for name in ["pkg", "alias"] {
        let metadata = registry.compiled_spec(name).unwrap();
        assert_eq!(metadata.provider_name, "pkg");
        assert!(!metadata.has_body);
        assert!(!metadata.has_location);
        assert!(metadata.is_package);
        assert_eq!(metadata.origin, "");
        assert_eq!(
            metadata
                .namespace_portions
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            ["/a/pkg", "/b/pkg"]
        );
    }
    // Artifact corruption must fail at installation, before a finder reads it.
    for (offset, bytes) in [
        (row + 24, vec![MODULE_KIND_SOURCE]),
        (
            row + 25,
            vec![MODULE_FLAG_PACKAGE | MODULE_FLAG_HAS_LOCATION],
        ),
        (row + 25, vec![MODULE_FLAG_PACKAGE | MODULE_FLAG_HAS_BODY]),
        (row + 36, 1u32.to_le_bytes().to_vec()),
        (row + 40, 0u32.to_le_bytes().to_vec()),
        (blob.len() - portions.len(), 0u32.to_le_bytes().to_vec()),
        (blob.len() - portions.len(), 7u32.to_le_bytes().to_vec()),
        (blob.len() - 1, vec![0xff]),
    ] {
        let mut corrupt = blob.clone();
        corrupt[offset..offset + bytes.len()].copy_from_slice(&bytes);
        assert!(
            ModuleRegistry::parse(corrupt.as_ptr()).is_err(),
            "offset {offset}"
        );
    }
}

#[test]
fn compiled_spec_projects_alias_provider_without_running_initializers() {
    static INITIALIZER_CALLS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn forbidden_initializer() -> u64 {
        INITIALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
        0
    }
    assert_eq!(INITIALIZER_CALLS.load(Ordering::SeqCst), 0);
    let init = forbidden_initializer as *const () as usize as u64;
    let mut builder = BlobBuilder::new();
    builder
        .row("alias", 0, None, Some(5), MODULE_KIND_ALIAS, 0)
        .row("alias_again", 0, None, Some(0), MODULE_KIND_ALIAS, 0)
        .row("builtin", init, None, None, MODULE_KIND_RUNTIME_BUILTIN, 0)
        .row(
            "namespace_reserved",
            0,
            None,
            None,
            MODULE_KIND_SOURCE,
            MODULE_FLAG_PACKAGE,
        )
        .row(
            "no_body",
            0,
            None,
            None,
            MODULE_KIND_SOURCE,
            MODULE_FLAG_PACKAGE,
        )
        .row(
            "package",
            init,
            None,
            None,
            MODULE_KIND_SOURCE,
            MODULE_FLAG_PACKAGE | MODULE_FLAG_HAS_LOCATION | MODULE_FLAG_STDLIB_PROVIDER,
        )
        .row("zz_static", init, None, None, MODULE_KIND_SOURCE, 0);
    let mut blob = builder.build();
    // Literal independent image fields: only the terminal package row has
    // this origin. Alias row flags/origins intentionally differ.
    let origin = b"/catalog/package/__init__.py";
    blob[40..48].copy_from_slice(&(origin.len() as u64).to_le_bytes());
    let row_start = MODULE_REGISTRY_HEADER_BYTES + 5 * MODULE_REGISTRY_ROW_BYTES;
    blob[row_start + 32..row_start + 36].copy_from_slice(&(origin.len() as u32).to_le_bytes());
    blob.extend_from_slice(origin);
    let registry = ModuleRegistry::parse(blob.as_ptr()).expect("valid independent catalog");
    for name in ["package", "alias", "alias_again"] {
        assert_eq!(
            registry.compiled_spec(name),
            Some(CompiledModuleSpec {
                provider_name: "package",
                is_stdlib_provider: true,
                has_body: true,
                origin: "/catalog/package/__init__.py",
                is_package: true,
                has_location: true,
                namespace_portions: None,
            })
        );
    }
    assert_eq!(
        registry.compiled_spec("builtin"),
        Some(CompiledModuleSpec {
            provider_name: "builtin",
            is_stdlib_provider: false,
            has_body: true,
            origin: "",
            is_package: false,
            has_location: false,
            namespace_portions: None,
        })
    );
    assert_eq!(
        registry.compiled_spec("zz_static"),
        Some(CompiledModuleSpec {
            provider_name: "zz_static",
            is_stdlib_provider: false,
            has_body: true,
            origin: "",
            is_package: false,
            has_location: false,
            namespace_portions: None,
        }),
        "source kind alone does not admit a file location"
    );
    for name in ["namespace_reserved", "no_body", "absent"] {
        assert_eq!(registry.compiled_spec(name), None);
    }
    assert_eq!(INITIALIZER_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn selected_body_availability_uses_source_provider_role_including_alias_metadata() {
    static BODY_CALLS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn body() -> u64 {
        BODY_CALLS.fetch_add(1, Ordering::SeqCst);
        0
    }
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        transaction.with_target_python_minor(py, 13, || {
            for name in ["msilib", "multiprocessing.popen_spawn_win32"] {
                if name.starts_with("multiprocessing") && cfg!(target_os = "windows") {
                    continue;
                }
                for is_stdlib in [false, true] {
                    let mut builder = BlobBuilder::new();
                    builder
                        .row("alias", 0, None, Some(1), MODULE_KIND_ALIAS, 0)
                        .row(
                            name,
                            body as *const () as usize as u64,
                            None,
                            None,
                            MODULE_KIND_SOURCE,
                            if is_stdlib {
                                MODULE_FLAG_STDLIB_PROVIDER
                            } else {
                                0
                            },
                        );
                    let blob = builder.build();
                    let registry = ModuleRegistry::parse(blob.as_ptr()).unwrap();
                    let spec = registry.compiled_spec("alias").unwrap();
                    assert_eq!(spec.provider_name, name);
                    assert_eq!(spec.is_stdlib_provider, is_stdlib);
                    let before = BODY_CALLS.load(Ordering::SeqCst);
                    dispatch_module_body(py, &registry, 1, body as *const () as usize as u64);
                    if is_stdlib {
                        assert_eq!(BODY_CALLS.load(Ordering::SeqCst), before);
                        let error = crate::builtins::exceptions::ExceptionValue::adopt(
                            py,
                            crate::molt_exception_last_pending(),
                        );
                        assert!(crate::builtins::exceptions::exception_matches_kind(
                            py,
                            error.bits(),
                            crate::BuiltinExc::ModuleNotFoundError,
                        ));
                        crate::clear_exception(py);
                        let key = crate::attr_name_bits_from_bytes(py, b"name").unwrap();
                        let missing =
                            crate::builtins::attributes::get_attr_name(py, error.bits(), key)
                                .unwrap();
                        let expected = if name == "msilib" { "msilib" } else { "msvcrt" };
                        assert_eq!(
                            crate::string_obj_to_owned(obj_from_bits(missing)).as_deref(),
                            Some(expected)
                        );
                        dec_ref_bits(py, key);
                        dec_ref_bits(py, missing);
                        crate::clear_exception(py);
                    } else {
                        assert!(!exception_pending(py));
                        assert_eq!(
                            BODY_CALLS.load(Ordering::SeqCst),
                            before + 1,
                            "same-name user source is not a stdlib provider"
                        );
                    }
                }
            }
        });
    });
}
