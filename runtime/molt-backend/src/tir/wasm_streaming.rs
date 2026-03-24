//! WASM streaming compilation support.
//! Generates metadata for browser-side streaming compilation.

use super::wasm_split::{SplitSizes, WasmSplitPlan};

/// Streaming compilation manifest — tells the browser which sections to prioritize.
pub struct StreamingManifest {
    /// Hot functions (entry point + main loop) — compile first.
    pub hot_section_functions: Vec<String>,
    /// Cold functions (stdlib, error handling) — stream in background.
    pub cold_section_functions: Vec<String>,
    /// Estimated hot section size in bytes.
    pub hot_size_estimate: usize,
    /// Estimated cold section size in bytes.
    pub cold_size_estimate: usize,
}

/// Generate a streaming manifest from a split plan.
pub fn generate_manifest(plan: &WasmSplitPlan, sizes: &SplitSizes) -> StreamingManifest {
    StreamingManifest {
        hot_section_functions: plan.core_functions.clone(),
        cold_section_functions: [plan.stdlib_functions.clone(), plan.user_functions.clone()]
            .concat(),
        hot_size_estimate: sizes.core_bytes,
        cold_size_estimate: sizes.stdlib_bytes + sizes.user_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::types::TirType;
    use crate::tir::wasm_split::{estimate_sizes, plan_split};

    fn make_module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "test".to_string(),
            functions: funcs,
            class_hierarchy: None,
        }
    }

    #[test]
    fn manifest_hot_section_is_core_functions() {
        let module = make_module(vec![
            TirFunction::new("main".to_string(), vec![], TirType::None),
            TirFunction::new("stdlib_len".to_string(), vec![], TirType::I64),
            TirFunction::new("user_fn".to_string(), vec![], TirType::None),
        ]);
        let plan = plan_split(&module);
        let sizes = estimate_sizes(&plan, &module);
        let manifest = generate_manifest(&plan, &sizes);

        assert_eq!(manifest.hot_section_functions, vec!["main".to_string()]);
    }

    #[test]
    fn manifest_cold_section_contains_stdlib_and_user() {
        let module = make_module(vec![
            TirFunction::new("stdlib_print".to_string(), vec![], TirType::None),
            TirFunction::new("helper".to_string(), vec![], TirType::None),
        ]);
        let plan = plan_split(&module);
        let sizes = estimate_sizes(&plan, &module);
        let manifest = generate_manifest(&plan, &sizes);

        assert!(
            manifest
                .cold_section_functions
                .contains(&"stdlib_print".to_string()),
            "stdlib not in cold: {:?}",
            manifest.cold_section_functions
        );
        assert!(
            manifest
                .cold_section_functions
                .contains(&"helper".to_string()),
            "user fn not in cold: {:?}",
            manifest.cold_section_functions
        );
    }

    #[test]
    fn manifest_size_estimates_are_consistent() {
        let module = make_module(vec![
            TirFunction::new("__main__".to_string(), vec![], TirType::None),
            TirFunction::new("stdlib_foo".to_string(), vec![], TirType::None),
        ]);
        let plan = plan_split(&module);
        let sizes = estimate_sizes(&plan, &module);
        let manifest = generate_manifest(&plan, &sizes);

        assert_eq!(manifest.hot_size_estimate, sizes.core_bytes);
        assert_eq!(
            manifest.cold_size_estimate,
            sizes.stdlib_bytes + sizes.user_bytes
        );
    }

    #[test]
    fn manifest_empty_module_has_zero_sizes() {
        let module = make_module(vec![]);
        let plan = plan_split(&module);
        let sizes = estimate_sizes(&plan, &module);
        let manifest = generate_manifest(&plan, &sizes);

        assert_eq!(manifest.hot_size_estimate, 0);
        assert_eq!(manifest.cold_size_estimate, 0);
        assert!(manifest.hot_section_functions.is_empty());
        assert!(manifest.cold_section_functions.is_empty());
    }
}
