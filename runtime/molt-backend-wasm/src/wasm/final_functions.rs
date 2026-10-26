use super::body::WasmLirFallbackReason;
use super::lir_fast::{
    WasmFunctionLoweringPlan, is_production_lir_wasm_fast_path_name, prepare_lir_wasm_fast_plan,
};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::TirFunction;
use crate::{FunctionIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};

/// The emitted body and every scalar/LIR decision have one private owner.
/// Only function retention and escaped-callable admission may change this
/// collection after projection; neither changes executable operations.
pub(super) struct WasmFinalFunctions {
    ir: SimpleIR,
    facts: Vec<Option<WasmFunctionFacts>>,
}

pub(super) struct WasmFunctionFacts {
    pub(super) representation: ScalarRepresentationPlan,
    pub(super) lowering: Option<WasmFunctionLoweringPlan>,
}

impl WasmFinalFunctions {
    pub(super) fn from_final_tir(mut ir: SimpleIR, typed: Vec<(bool, TirFunction)>) -> Self {
        let mut by_name = BTreeMap::new();
        for (external, function) in typed {
            assert!(
                by_name
                    .insert(function.name.clone(), (external, function))
                    .is_none(),
                "WASM final custody produced duplicate function"
            );
        }
        let facts = ir
            .functions
            .iter_mut()
            .map(|function| {
                let (external, typed) = by_name
                    .remove(&function.name)
                    .unwrap_or_else(|| panic!("WASM final custody is missing '{}'", function.name));
                assert_eq!(
                    external, function.is_extern,
                    "WASM final custody extern mismatch"
                );
                if external {
                    return None;
                }
                let (representation, lowering) =
                    if is_production_lir_wasm_fast_path_name(&function.name) {
                        let (representation, lowering) =
                            ScalarRepresentationPlan::project_final_tir_with_value_consumer(
                                function,
                                &typed,
                                prepare_lir_wasm_fast_plan,
                            );
                        (representation, Some(lowering))
                    } else {
                        (
                            ScalarRepresentationPlan::project_final_tir(function, &typed),
                            None,
                        )
                    };
                Some(WasmFunctionFacts {
                    representation,
                    lowering,
                })
            })
            .collect();
        assert!(
            by_name.is_empty(),
            "WASM final custody has unmatched typed bodies"
        );
        Self { ir, facts }
    }

    /// Tests author exact final CFG/lifetime operations. Analyze fresh authored
    /// input without replacing its operations or introducing executable polls.
    /// Production cannot enter through this boundary.
    #[cfg(test)]
    pub(super) fn from_codegen_fixtures(ir: SimpleIR) -> Self {
        for function in &ir.functions {
            molt_ir::ir_schema::validate_normalized_simple_ir_ops(&function.name, &function.ops)
                .unwrap_or_else(|error| panic!("WASM final fixture admission failed: {error}"));
        }
        let target = crate::tir::TargetInfo::wasm_release_fast();
        let facts = ir
            .functions
            .iter()
            .map(|function| {
                if function.is_extern {
                    return None;
                }
                Some(WasmFunctionFacts {
                    representation: ScalarRepresentationPlan::for_function_ir_for_target(
                        function, &target,
                    ),
                    lowering: None,
                })
            })
            .collect();
        Self { ir, facts }
    }

    #[cfg(test)]
    pub(crate) fn lowering_plans_for_fixtures(
        ir: &SimpleIR,
        escaped: &BTreeSet<String>,
    ) -> BTreeMap<String, WasmFunctionLoweringPlan> {
        let typed = ir
            .functions
            .iter()
            .map(|function| {
                let mut typed = crate::tir::lower_from_simple::lower_to_tir(function);
                crate::tir::type_refine::refine_types(&mut typed);
                (function.is_extern, typed)
            })
            .collect();
        let mut final_functions = Self::from_final_tir(ir.clone(), typed);
        final_functions.exclude_escaped_callables(escaped);
        final_functions
            .into_functions()
            .filter_map(|(function, facts)| {
                facts
                    .and_then(|facts| facts.lowering)
                    .map(|plan| (function.name, plan))
            })
            .collect()
    }

    pub(super) fn ir(&self) -> &SimpleIR {
        &self.ir
    }

    pub(super) fn retain_reachable(&mut self, roots: &BTreeSet<String>) {
        let reachable = crate::passes::reachable_function_names(&self.ir.functions, roots);
        let mut functions = Vec::new();
        let mut facts = Vec::new();
        for (function, fact) in std::mem::take(&mut self.ir.functions)
            .into_iter()
            .zip(std::mem::take(&mut self.facts))
        {
            if reachable.contains(&function.name) {
                functions.push(function);
                facts.push(fact);
            }
        }
        self.ir.functions = functions;
        self.facts = facts;
    }

    pub(super) fn exclude_escaped_callables(&mut self, escaped: &BTreeSet<String>) {
        for (function, facts) in self.ir.functions.iter().zip(&mut self.facts) {
            if escaped.contains(&function.name)
                && let Some(facts) = facts
                && facts.lowering.is_some()
            {
                facts.lowering = Some(WasmFunctionLoweringPlan::Generic {
                    reason: WasmLirFallbackReason::EscapedCallableTarget,
                });
            }
        }
    }

    pub(super) fn into_functions(
        self,
    ) -> impl Iterator<Item = (FunctionIR, Option<WasmFunctionFacts>)> {
        self.ir.functions.into_iter().zip(self.facts)
    }
}
