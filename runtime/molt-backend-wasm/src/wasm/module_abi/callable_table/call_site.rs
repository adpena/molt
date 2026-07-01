use crate::passes::ReturnAliasSummary;
use std::collections::{BTreeMap, BTreeSet};

use super::WasmCallableTablePlan;

pub(in crate::wasm) struct WasmCallableCallSiteAbi<'a> {
    func_table_slots: &'a BTreeMap<String, u32>,
    func_indices: &'a BTreeMap<String, u32>,
    trampoline_slots: &'a BTreeMap<String, u32>,
    table_base: u32,
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
            table_base: plan.table_base,
            closure_functions: &plan.closure_functions,
            escaped_callable_targets,
            call_func_spill_offset,
            return_alias_summaries,
        }
    }

    pub(in crate::wasm) fn table_index(&self, target_name: &str, call_kind: &str) -> u32 {
        let slot = *self
            .func_table_slots
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} table target not found: {target_name}"));
        self.table_base + slot
    }

    pub(in crate::wasm) fn function_index(&self, target_name: &str, call_kind: &str) -> u32 {
        *self
            .func_indices
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} function target not found: {target_name}"))
    }

    pub(in crate::wasm) fn trampoline_table_index(
        &self,
        target_name: &str,
        call_kind: &str,
    ) -> u32 {
        let slot = *self
            .trampoline_slots
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} trampoline target not found: {target_name}"));
        self.table_base + slot
    }

    pub(in crate::wasm) fn callable_table_pair(
        &self,
        target_name: &str,
        call_kind: &str,
    ) -> WasmCallableTablePair {
        WasmCallableTablePair {
            function_table_index: self.table_index(target_name, call_kind),
            trampoline_table_index: self.trampoline_table_index(target_name, call_kind),
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
    pub(in crate::wasm) function_table_index: u32,
    pub(in crate::wasm) trampoline_table_index: u32,
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
            table_indices: Vec::new(),
            sentinel_func_idx: u32::MAX,
            split_runtime_owned_slot_start: 0,
            split_runtime_shared_abi_slot_end: 0,
            func_to_table_idx: BTreeMap::from([("callee".to_string(), 7)]),
            func_to_index: BTreeMap::from([("callee".to_string(), 42)]),
            func_to_trampoline_idx: BTreeMap::from([("callee".to_string(), 9)]),
            closure_functions: BTreeSet::from(["callee".to_string()]),
            trampoline_entries: Vec::new(),
        };
        let escaped_targets = BTreeSet::from(["callee".to_string()]);
        let return_alias_summaries =
            BTreeMap::from([("callee".to_string(), ReturnAliasSummary::Param(1))]);
        let abi = plan.call_site_abi(&escaped_targets, 4096, &return_alias_summaries);

        let table_pair = abi.callable_table_pair("callee", "test_call");
        assert_eq!(table_pair.function_table_index, 107);
        assert_eq!(table_pair.trampoline_table_index, 109);
        assert_eq!(abi.function_index("callee", "test_call"), 42);
        assert!(abi.is_closure_function("callee"));
        assert!(abi.is_escaped_callable("callee"));
        assert_eq!(abi.call_func_spill_offset(), 4096);
        assert!(abi.returns_alias_param("callee", &["x".to_string(), "y".to_string()]));
        assert!(!abi.returns_alias_param("callee", &["x".to_string()]));
    }
}
