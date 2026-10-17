use super::tests::BlobBuilder;
use super::*;

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
            MODULE_KIND_NAMESPACE_PARENT,
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
            MODULE_FLAG_PACKAGE,
        );
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
                origin: "/catalog/package/__init__.py",
                is_package: true,
            })
        );
    }
    assert_eq!(
        registry.compiled_spec("builtin"),
        Some(CompiledModuleSpec {
            origin: "",
            is_package: false,
        })
    );
    for name in ["namespace_reserved", "no_body", "absent"] {
        assert_eq!(registry.compiled_spec(name), None);
    }
    assert_eq!(INITIALIZER_CALLS.load(Ordering::SeqCst), 0);
}
