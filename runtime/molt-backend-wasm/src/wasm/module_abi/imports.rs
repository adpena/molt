use wasm_encoder::{EntityType, ImportSection};

use super::runtime_surface::WasmRuntimeSurfacePlan;
use crate::SimpleIR;
use crate::wasm::WasmBackend;
use crate::wasm_abi::{
    IMPORT_REGISTRY, RUNTIME_IMPORT_MODULE, RuntimeImportSpec, WasmRuntimeImport,
};
use crate::wasm_function_types::WasmFunctionTypes;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmProfile;

impl WasmBackend {
    pub(super) fn emit_runtime_import_surface(&mut self, ir: &SimpleIR) -> WasmRuntimeSurfacePlan {
        let runtime_surface = WasmRuntimeSurfacePlan::build(ir, self.options.wasm_profile);
        let mut registrar = RuntimeImportRegistrar {
            imports: &mut self.imports,
            types: &mut self.types,
            import_ids: &mut self.import_ids,
            import_idx: 0,
            profile: self.options.wasm_profile,
        };

        // Register the full profile-allowed surface upfront. Import-surface
        // minimization is owned by finalization: TrackedImportIds records
        // which imports emitted code actually addresses, and unobserved
        // imports are stripped and remapped before relocation/link sections
        // are emitted (docs/architecture/wasm-import-stripping.md). A
        // pre-registration filter would have to mirror every emitter's
        // import usage (layout roots, poll table, op-loop metadata, ...) —
        // any miss is a codegen panic, and three landed within one day.
        for spec in IMPORT_REGISTRY {
            registrar.add_spec(*spec);
        }

        for import in runtime_surface.auto_imports(registrar.import_ids) {
            registrar.add_import(import);
        }

        self.func_count = registrar.import_idx;
        runtime_surface
    }
}

struct RuntimeImportRegistrar<'a> {
    imports: &'a mut ImportSection,
    types: &'a mut WasmFunctionTypes,
    import_ids: &'a mut TrackedImportIds,
    import_idx: u32,
    profile: WasmProfile,
}

impl RuntimeImportRegistrar<'_> {
    fn add_spec(&mut self, spec: RuntimeImportSpec) {
        self.add_import(spec.import);
    }

    fn add_import(&mut self, import: WasmRuntimeImport) {
        let name = import.name();
        if !self.profile.allows_runtime_import(import) {
            self.import_ids.insert(import, u32::MAX);
            return;
        }
        let (params, results) = import.machine_signature();
        let type_index = self.types.function(params, results);
        self.imports.import(
            RUNTIME_IMPORT_MODULE,
            name,
            EntityType::Function(type_index),
        );
        self.import_ids.insert(import, self.import_idx);
        self.import_idx += 1;
    }
}
