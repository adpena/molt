//! Static import registry for the WASM backend.
//!
//! Generated from `wasm_abi_manifest.toml` so import names, type indices,
//! callable metadata, and pure-profile skip prefixes share one manifest
//! authority. Edit the manifest, then run
//! `python tools/gen_wasm_abi.py`.

pub(crate) use crate::wasm_abi_generated::IMPORT_REGISTRY;
#[cfg(test)]
mod tests {
    use super::IMPORT_REGISTRY;
    use crate::wasm_abi_generated::{
        LirRuntimeCall, WasmObjectNewBoundPayload, WasmRuntimeImport, op_loop_runtime_call,
        wasm_object_new_bound_selection,
    };

    #[test]
    fn module_cache_del_is_registered_as_on_demand_wasm_import() {
        let import_type = IMPORT_REGISTRY.iter().find_map(|spec| {
            (spec.import == WasmRuntimeImport::ModuleCacheDel).then_some(spec.type_idx)
        });
        assert_eq!(
            import_type,
            Some(2),
            "module_cache_del must use the unary i64 -> i64 host import ABI"
        );

        let op_call =
            op_loop_runtime_call("module_cache_del").expect("module_cache_del op-loop call");
        assert_eq!(op_call.import, WasmRuntimeImport::ModuleCacheDel);
        assert_eq!(
            op_call.required_imports,
            [WasmRuntimeImport::ModuleCacheDel],
            "module_cache_del codegen must request its runtime import explicitly"
        );
    }

    #[test]
    fn object_new_bound_declares_wasm_imports() {
        let bound_type = IMPORT_REGISTRY.iter().find_map(|spec| {
            (spec.import == WasmRuntimeImport::ObjectNewBound).then_some(spec.type_idx)
        });
        assert_eq!(
            bound_type,
            Some(2),
            "object_new_bound must use the unary i64 -> i64 host import ABI"
        );

        let sized_type = IMPORT_REGISTRY.iter().find_map(|spec| {
            (spec.import == WasmRuntimeImport::ObjectNewBoundSized).then_some(spec.type_idx)
        });
        assert_eq!(
            sized_type,
            Some(3),
            "object_new_bound_sized must use the binary i64,i64 -> i64 host import ABI"
        );

        let unsized_selection = wasm_object_new_bound_selection(WasmObjectNewBoundPayload::Unsized);
        assert_eq!(unsized_selection.import, WasmRuntimeImport::ObjectNewBound);
        assert_eq!(
            unsized_selection.lir_runtime_call,
            LirRuntimeCall::ObjectNewBound
        );
        let sized = wasm_object_new_bound_selection(WasmObjectNewBoundPayload::Sized);
        assert_eq!(sized.import, WasmRuntimeImport::ObjectNewBoundSized);
        assert_eq!(sized.lir_runtime_call, LirRuntimeCall::ObjectNewBoundSized);
    }
}
