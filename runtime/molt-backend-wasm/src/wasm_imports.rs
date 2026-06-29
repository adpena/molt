//! Static import registry and op->import dependency table for WASM backend.
//!
//! Generated from `wasm_abi_manifest.toml` so import names, type indices,
//! callable metadata, pure-profile skip prefixes, runtime-surface import
//! matchers, and op dependency planning all share one manifest authority. Edit
//! the manifest, then run
//! `python tools/gen_wasm_abi.py`.

pub(crate) use crate::wasm_abi_generated::{
    IMPORT_REGISTRY, OP_IMPORT_DEPS, runtime_surface_requires_direct_import,
};
#[cfg(test)]
mod tests {
    use super::{IMPORT_REGISTRY, OP_IMPORT_DEPS, runtime_surface_requires_direct_import};
    use crate::wasm_abi_generated::{
        LirRuntimeCall, WasmObjectNewBoundPayload, wasm_object_new_bound_selection,
    };

    #[test]
    fn module_cache_del_is_registered_as_on_demand_wasm_import() {
        let import_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "module_cache_del").then_some(type_idx));
        assert_eq!(
            import_type,
            Some(2),
            "module_cache_del must use the unary i64 -> i64 host import ABI"
        );

        let structural = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "__structural__").then_some(deps))
            .expect("structural WASM import deps must exist");
        assert!(
            !structural.contains(&"module_cache_del"),
            "module_cache_del is cleanup-only and must not inflate every Auto-profile WASM binary"
        );

        let op_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "module_cache_del").then_some(deps))
            .expect("module_cache_del op must declare its WASM import dependency");
        assert_eq!(
            op_deps,
            ["module_cache_del"],
            "module_cache_del codegen must request its runtime import explicitly"
        );
    }

    #[test]
    fn object_new_bound_declares_wasm_imports() {
        let bound_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "object_new_bound").then_some(type_idx));
        assert_eq!(
            bound_type,
            Some(2),
            "object_new_bound must use the unary i64 -> i64 host import ABI"
        );

        let sized_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "object_new_bound_sized").then_some(type_idx));
        assert_eq!(
            sized_type,
            Some(3),
            "object_new_bound_sized must use the binary i64,i64 -> i64 host import ABI"
        );

        assert!(
            OP_IMPORT_DEPS
                .iter()
                .all(|&(kind, _deps)| kind != "object_new_bound"),
            "object_new_bound import demand is selected from payload-size metadata, not OP_IMPORT_DEPS"
        );
        let unsized_selection = wasm_object_new_bound_selection(WasmObjectNewBoundPayload::Unsized);
        assert_eq!(unsized_selection.import_name, "object_new_bound");
        assert_eq!(
            unsized_selection.lir_runtime_call,
            LirRuntimeCall::ObjectNewBound
        );
        let sized = wasm_object_new_bound_selection(WasmObjectNewBoundPayload::Sized);
        assert_eq!(sized.import_name, "object_new_bound_sized");
        assert_eq!(sized.lir_runtime_call, LirRuntimeCall::ObjectNewBoundSized);

        let stack_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "object_new_bound_stack").then_some(deps))
            .expect("object_new_bound_stack op must declare its WASM import dependencies");
        assert_eq!(
            stack_deps,
            ["object_new_bound_sized"],
            "WASM has no native stack object representation; stack-eligible class allocation lowers to the sized heap constructor"
        );
    }

    #[test]
    fn runtime_surface_direct_import_matchers_are_generated() {
        assert!(runtime_surface_requires_direct_import("path_exists"));
        assert!(runtime_surface_requires_direct_import("socket_bind"));
        assert!(runtime_surface_requires_direct_import("socketpair"));
        assert!(runtime_surface_requires_direct_import("os_name"));
        assert!(runtime_surface_requires_direct_import("errno_constants"));
        assert!(!runtime_surface_requires_direct_import("call_func"));
    }
}
