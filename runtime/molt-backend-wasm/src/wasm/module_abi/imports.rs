use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{EntityType, ImportSection};

use super::runtime_surface::WasmRuntimeSurfacePlan;
use crate::wasm::WasmBackend;
use crate::wasm_abi_generated::{RuntimeImportSpec, WasmRuntimeImport};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmProfile;
use crate::{SimpleIR, TrampolineKind};

pub(super) struct WasmRuntimeImportEmission {
    pub(super) runtime_surface: WasmRuntimeSurfacePlan,
    pub(super) next_type_idx: u32,
}

impl WasmBackend {
    pub(super) fn emit_runtime_import_surface(
        &mut self,
        ir: &SimpleIR,
        lir_lowering_plans: &crate::wasm::lir_fast::WasmFunctionLoweringPlans,
        task_kinds: &BTreeMap<String, TrampolineKind>,
    ) -> WasmRuntimeImportEmission {
        let runtime_surface =
            WasmRuntimeSurfacePlan::build(ir, lir_lowering_plans, task_kinds, &self.options);
        let auto_required = runtime_surface
            .import_demand
            .auto_required_imports()
            .cloned();
        let mut registrar = RuntimeImportRegistrar {
            imports: &mut self.imports,
            import_ids: &mut self.import_ids,
            import_idx: 0,
            is_pure: self.options.wasm_profile == WasmProfile::Pure,
            planned_required: auto_required,
        };

        for spec in crate::wasm_imports::IMPORT_REGISTRY {
            registrar.add_spec(*spec);
        }

        let next_type_idx = crate::wasm_abi::STATIC_TYPE_COUNT;
        for import in runtime_surface.auto_imports(registrar.import_ids) {
            registrar.add_import(import);
        }

        self.func_count = registrar.import_idx;
        WasmRuntimeImportEmission {
            runtime_surface,
            next_type_idx,
        }
    }
}

struct RuntimeImportRegistrar<'a> {
    imports: &'a mut ImportSection,
    import_ids: &'a mut TrackedImportIds,
    import_idx: u32,
    is_pure: bool,
    planned_required: Option<BTreeSet<WasmRuntimeImport>>,
}

impl RuntimeImportRegistrar<'_> {
    fn add_spec(&mut self, spec: RuntimeImportSpec) {
        self.add_import(spec.import);
    }

    fn add_import(&mut self, import: WasmRuntimeImport) {
        let name = import.name();
        if matches!(
            std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
            Some("1")
        ) && name == "task_new"
        {
            eprintln!(
                "WASM_IMPORTS add_import name=task_new skipped_prefix={} planned_required_contains={}",
                self.is_skipped_import(import),
                self.planned_required
                    .as_ref()
                    .is_none_or(|required| required.contains(&import))
            );
        }
        if self.is_skipped_import(import) {
            self.import_ids.insert(import, u32::MAX);
            return;
        }
        if let Some(ref required) = self.planned_required
            && !required.contains(&import)
        {
            self.import_ids.insert(import, u32::MAX);
            return;
        }
        self.imports.import(
            "molt_runtime",
            name,
            EntityType::Function(import.type_idx()),
        );
        self.import_ids.insert(import, self.import_idx);
        self.import_idx += 1;
    }

    fn is_skipped_import(&self, import: WasmRuntimeImport) -> bool {
        let name = import.name();
        self.is_pure && crate::wasm_abi_generated::pure_profile_skips_import(name)
    }
}
