//! WASM code splitting — separates runtime core from stdlib and user code.

use super::function::TirModule;

/// Split plan for a WASM module.
pub struct WasmSplitPlan {
    /// Functions that go in the core module (always loaded).
    pub core_functions: Vec<String>,
    /// Functions that go in stdlib stubs (loaded on demand).
    pub stdlib_functions: Vec<String>,
    /// Functions that go in user code module.
    pub user_functions: Vec<String>,
}

/// Analyze a TIR module and produce a split plan.
pub fn plan_split(module: &TirModule) -> WasmSplitPlan {
    let mut core = Vec::new();
    let mut stdlib = Vec::new();
    let mut user = Vec::new();

    for func in &module.functions {
        if func.name.starts_with("__builtins__") || func.name == "__main__" || func.name == "main" {
            core.push(func.name.clone());
        } else if func.name.starts_with("stdlib_") || func.name.contains("__stdlib__") {
            stdlib.push(func.name.clone());
        } else {
            user.push(func.name.clone());
        }
    }

    WasmSplitPlan {
        core_functions: core,
        stdlib_functions: stdlib,
        user_functions: user,
    }
}

/// Estimate binary size for each split component.
pub fn estimate_sizes(plan: &WasmSplitPlan, module: &TirModule) -> SplitSizes {
    let ops_per_func: std::collections::HashMap<&str, usize> = module
        .functions
        .iter()
        .map(|f| {
            (
                f.name.as_str(),
                f.blocks.values().map(|b| b.ops.len()).sum(),
            )
        })
        .collect();

    let estimate = |names: &[String]| -> usize {
        names
            .iter()
            .filter_map(|n| ops_per_func.get(n.as_str()))
            .map(|&ops| ops * 8) // ~8 bytes per WASM op (rough estimate)
            .sum()
    };

    SplitSizes {
        core_bytes: estimate(&plan.core_functions),
        stdlib_bytes: estimate(&plan.stdlib_functions),
        user_bytes: estimate(&plan.user_functions),
    }
}

pub struct SplitSizes {
    pub core_bytes: usize,
    pub stdlib_bytes: usize,
    pub user_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::types::TirType;

    fn make_module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "test".to_string(),
            functions: funcs,
        }
    }

    #[test]
    fn plan_split_categorises_builtins_as_core() {
        let module = make_module(vec![
            TirFunction::new("__builtins__len".to_string(), vec![], TirType::I64),
            TirFunction::new("main".to_string(), vec![], TirType::None),
            TirFunction::new("__main__".to_string(), vec![], TirType::None),
        ]);
        let plan = plan_split(&module);
        assert_eq!(plan.core_functions.len(), 3);
        assert!(plan.stdlib_functions.is_empty());
        assert!(plan.user_functions.is_empty());
    }

    #[test]
    fn plan_split_categorises_stdlib_functions() {
        let module = make_module(vec![
            TirFunction::new("stdlib_print".to_string(), vec![], TirType::None),
            TirFunction::new("foo__stdlib__bar".to_string(), vec![], TirType::None),
        ]);
        let plan = plan_split(&module);
        assert_eq!(plan.stdlib_functions.len(), 2);
        assert!(plan.core_functions.is_empty());
        assert!(plan.user_functions.is_empty());
    }

    #[test]
    fn plan_split_puts_unknown_functions_in_user() {
        let module = make_module(vec![TirFunction::new(
            "my_function".to_string(),
            vec![],
            TirType::I64,
        )]);
        let plan = plan_split(&module);
        assert_eq!(plan.user_functions, vec!["my_function".to_string()]);
        assert!(plan.core_functions.is_empty());
        assert!(plan.stdlib_functions.is_empty());
    }

    #[test]
    fn estimate_sizes_returns_zero_for_empty_module() {
        let module = make_module(vec![]);
        let plan = plan_split(&module);
        let sizes = estimate_sizes(&plan, &module);
        assert_eq!(sizes.core_bytes, 0);
        assert_eq!(sizes.stdlib_bytes, 0);
        assert_eq!(sizes.user_bytes, 0);
    }

    #[test]
    fn estimate_sizes_scales_with_op_count() {
        // A function with no ops should contribute 0 bytes.
        let module = make_module(vec![TirFunction::new(
            "main".to_string(),
            vec![],
            TirType::None,
        )]);
        let plan = plan_split(&module);
        let sizes = estimate_sizes(&plan, &module);
        // Entry block has 0 ops (empty function), so 0 * 8 = 0 bytes.
        assert_eq!(sizes.core_bytes, 0);
    }
}
