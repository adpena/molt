use super::*;

#[test]
fn lir_runtime_calls_are_manifest_registered_imports() {
    let manifest_imports: std::collections::BTreeSet<_> = crate::wasm_abi::IMPORT_REGISTRY
        .iter()
        .map(|spec| spec.import)
        .collect();

    for call in LirRuntimeCall::ALL {
        let import = call.import();
        assert!(
            manifest_imports.contains(&import),
            "LIR fast runtime call {call:?} must register {} in wasm_abi_manifest.toml",
            import.name()
        );
    }
}
