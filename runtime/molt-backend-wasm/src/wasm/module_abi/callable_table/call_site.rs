use crate::passes::ReturnAliasSummary;
use std::collections::{BTreeMap, BTreeSet};

use super::WasmCallableTablePlan;
use crate::wasm_table::{WasmCallableTableRole, WasmCallableTableTarget};

pub(in crate::wasm) struct WasmCallableCallSiteAbi<'a> {
    func_table_slots: &'a BTreeMap<String, u32>,
    func_indices: &'a BTreeMap<String, u32>,
    trampoline_slots: &'a BTreeMap<String, u32>,
    plan: &'a WasmCallableTablePlan,
    closure_functions: &'a BTreeSet<String>,
    escaped_callable_targets: &'a BTreeSet<String>,
    call_func_spill_offset: u32,
    return_alias_summaries: &'a BTreeMap<String, ReturnAliasSummary>,
}

impl<'a> WasmCallableCallSiteAbi<'a> {
    pub(super) fn from_table_plan(
        plan: &'a WasmCallableTablePlan,
        escaped_callable_targets: &'a BTreeSet<String>,
        call_func_spill_offset: u32,
        return_alias_summaries: &'a BTreeMap<String, ReturnAliasSummary>,
    ) -> Self {
        Self {
            func_table_slots: &plan.func_to_table_idx,
            func_indices: &plan.func_to_index,
            trampoline_slots: &plan.func_to_trampoline_idx,
            plan,
            closure_functions: &plan.closure_functions,
            escaped_callable_targets,
            call_func_spill_offset,
            return_alias_summaries,
        }
    }

    pub(in crate::wasm) fn table_target(
        &self,
        target_name: &str,
        call_kind: &str,
    ) -> WasmCallableTableTarget {
        let slot = *self
            .func_table_slots
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} table target not found: {target_name}"));
        self.plan
            .target_for_slot(slot, WasmCallableTableRole::DirectCallable)
    }

    pub(in crate::wasm) fn function_index(&self, target_name: &str, call_kind: &str) -> u32 {
        *self
            .func_indices
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} function target not found: {target_name}"))
    }

    pub(in crate::wasm) fn function_abi_returns_value(&self, target_name: &str) -> bool {
        *self
            .plan
            .function_abi_returns_value
            .get(target_name)
            .unwrap_or_else(|| {
                panic!("WASM call target has no canonical ABI result fact: {target_name}")
            })
    }

    pub(in crate::wasm) fn trampoline_target(
        &self,
        target_name: &str,
        call_kind: &str,
    ) -> WasmCallableTableTarget {
        let slot = *self
            .trampoline_slots
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} trampoline target not found: {target_name}"));
        self.plan
            .target_for_slot(slot, WasmCallableTableRole::Trampoline)
    }

    pub(in crate::wasm) fn callable_table_pair(
        &self,
        target_name: &str,
        call_kind: &str,
    ) -> WasmCallableTablePair {
        WasmCallableTablePair {
            function: self.table_target(target_name, call_kind),
            trampoline: self.trampoline_target(target_name, call_kind),
        }
    }

    pub(in crate::wasm) fn is_closure_function(&self, target_name: &str) -> bool {
        self.closure_functions.contains(target_name)
    }

    pub(in crate::wasm) fn is_escaped_callable(&self, target_name: &str) -> bool {
        self.escaped_callable_targets.contains(target_name)
    }

    pub(in crate::wasm) fn call_func_spill_offset(&self) -> u32 {
        self.call_func_spill_offset
    }

    pub(in crate::wasm) fn returns_alias_param(
        &self,
        target_name: &str,
        args_names: &[String],
    ) -> bool {
        self.return_alias_summaries
            .get(target_name)
            .and_then(|summary| match summary {
                ReturnAliasSummary::Param(param_idx) if *param_idx < args_names.len() => {
                    Some(*param_idx)
                }
                _ => None,
            })
            .is_some()
    }
}

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmCallableTablePair {
    pub(in crate::wasm) function: WasmCallableTableTarget,
    pub(in crate::wasm) trampoline: WasmCallableTableTarget,
}

#[cfg(test)]
mod tests {
    use super::super::WasmCallableTablePlan;
    use crate::passes::ReturnAliasSummary;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn callable_table_plan_canonicalizes_call_site_indices_and_lifecycle_facts() {
        let plan = WasmCallableTablePlan {
            table_base: 100,
            fixed_shared_runtime_abi_base: None,
            table_entries: (0..10)
                .map(|defined_func_index| super::super::WasmCallableTableEntry {
                    func_index: 42 + defined_func_index,
                    symbol: crate::wasm_table::WasmFunctionSymbol::Defined { defined_func_index },
                })
                .collect(),
            split_runtime_shared_abi_slot_end: 0,
            func_to_table_idx: BTreeMap::from([("callee".to_string(), 7)]),
            func_to_index: BTreeMap::from([("callee".to_string(), 42)]),
            func_to_trampoline_idx: BTreeMap::from([("callee".to_string(), 9)]),
            app_callable_resolver: None,
            closure_functions: BTreeSet::from(["callee".to_string()]),
            function_abi_returns_value: BTreeMap::from([("callee".to_string(), true)]),
            trampoline_entries: Vec::new(),
        };
        let escaped_targets = BTreeSet::from(["callee".to_string()]);
        let return_alias_summaries =
            BTreeMap::from([("callee".to_string(), ReturnAliasSummary::Param(1))]);
        let abi = plan.call_site_abi(&escaped_targets, 4096, &return_alias_summaries);

        let table_pair = abi.callable_table_pair("callee", "test_call");
        assert_eq!(table_pair.function.current_table_index, 107);
        assert_eq!(table_pair.trampoline.current_table_index, 109);
        assert_eq!(abi.function_index("callee", "test_call"), 42);
        assert!(abi.function_abi_returns_value("callee"));
        assert!(abi.is_closure_function("callee"));
        assert!(abi.is_escaped_callable("callee"));
        assert_eq!(abi.call_func_spill_offset(), 4096);
        assert!(abi.returns_alias_param("callee", &["x".to_string(), "y".to_string()]));
        assert!(!abi.returns_alias_param("callee", &["x".to_string()]));
    }

    #[test]
    fn shared_runtime_prefix_is_the_only_fixed_table_address_class() {
        let plan = WasmCallableTablePlan {
            table_base: 100,
            fixed_shared_runtime_abi_base: Some(40),
            table_entries: (0..10)
                .map(|defined_func_index| super::super::WasmCallableTableEntry {
                    func_index: 42 + defined_func_index,
                    symbol: crate::wasm_table::WasmFunctionSymbol::Defined { defined_func_index },
                })
                .collect(),
            split_runtime_shared_abi_slot_end: 8,
            func_to_table_idx: BTreeMap::from([("callee".to_string(), 7)]),
            func_to_index: BTreeMap::from([("callee".to_string(), 42)]),
            func_to_trampoline_idx: BTreeMap::from([("callee".to_string(), 9)]),
            app_callable_resolver: None,
            closure_functions: BTreeSet::new(),
            function_abi_returns_value: BTreeMap::from([("callee".to_string(), true)]),
            trampoline_entries: Vec::new(),
        };
        let escaped = BTreeSet::new();
        let returns = BTreeMap::new();
        let abi = plan.call_site_abi(&escaped, 0, &returns);
        let pair = abi.callable_table_pair("callee", "fixed-mask-test");

        assert!(matches!(
            pair.function.address,
            crate::wasm_table::WasmCallableTableAddress::FixedSharedRuntimeAbi { .. }
        ));
        assert_eq!(pair.function.current_table_index, 47);
        assert_eq!(pair.trampoline.current_table_index, 101);
        assert!(matches!(
            pair.trampoline.address,
            crate::wasm_table::WasmCallableTableAddress::Relocatable(_)
        ));
    }
}
