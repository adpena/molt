use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{EntityType, ImportSection, ValType};

use super::runtime_surface::WasmRuntimeSurfacePlan;
use crate::wasm::WasmBackend;
use crate::wasm_abi::{STATIC_TYPE_COUNT, TypeSectionExt};
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
            auto_required,
        };

        for &(name, type_idx) in crate::wasm_imports::IMPORT_REGISTRY {
            registrar.add(name, type_idx);
        }

        let mut simple_i64_import_type_map: BTreeMap<usize, u32> = BTreeMap::from([
            (0, 0),
            (1, 2),
            (2, 3),
            (3, 5),
            (4, 7),
            (5, 12),
            (6, 9),
            (7, 10),
            (8, 28),
            (9, 35),
            (10, 36),
            (11, 37),
            (12, 38),
        ]);
        let mut next_type_idx = STATIC_TYPE_COUNT;
        let auto_import_names = runtime_surface.auto_import_names(registrar.import_ids);
        for &arity in auto_import_names.iter().map(|(_, arity)| arity) {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                simple_i64_import_type_map.entry(arity)
            {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }
        for (import_name, arity) in auto_import_names {
            let type_idx = *simple_i64_import_type_map
                .get(&arity)
                .unwrap_or_else(|| panic!("missing simple i64 import type for arity {arity}"));
            registrar.add(import_name.as_str(), type_idx);
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
    auto_required: Option<BTreeSet<String>>,
}

impl RuntimeImportRegistrar<'_> {
    fn add(&mut self, name: &str, ty: u32) {
        if matches!(
            std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
            Some("1")
        ) && name == "task_new"
        {
            eprintln!(
                "WASM_IMPORTS add_import name=task_new skipped_prefix={} auto_required_contains={}",
                self.is_skipped_import(name),
                self.auto_required
                    .as_ref()
                    .is_none_or(|required| required.contains(name))
            );
        }
        if self.is_skipped_import(name) {
            self.import_ids.insert(name.to_string(), u32::MAX);
            return;
        }
        if let Some(ref required) = self.auto_required
            && !required.contains(name)
        {
            self.import_ids.insert(name.to_string(), u32::MAX);
            return;
        }
        self.imports
            .import("molt_runtime", name, EntityType::Function(ty));
        self.import_ids.insert(name.to_string(), self.import_idx);
        self.import_idx += 1;
    }

    fn is_skipped_import(&self, name: &str) -> bool {
        self.is_pure && crate::wasm_abi_generated::pure_profile_skips_import(name)
    }
}
