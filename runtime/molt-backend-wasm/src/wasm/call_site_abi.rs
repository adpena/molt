use crate::passes::ReturnAliasSummary;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct WasmCallSiteAbi<'a> {
    func_table_slots: &'a BTreeMap<String, u32>,
    func_indices: &'a BTreeMap<String, u32>,
    trampoline_slots: &'a BTreeMap<String, u32>,
    table_base: u32,
    closure_functions: &'a BTreeSet<String>,
    escaped_callable_targets: &'a BTreeSet<String>,
    call_func_spill_offset: u32,
    return_alias_summaries: &'a BTreeMap<String, ReturnAliasSummary>,
}

impl<'a> WasmCallSiteAbi<'a> {
    pub(super) fn new(
        func_table_slots: &'a BTreeMap<String, u32>,
        func_indices: &'a BTreeMap<String, u32>,
        trampoline_slots: &'a BTreeMap<String, u32>,
        table_base: u32,
        closure_functions: &'a BTreeSet<String>,
        escaped_callable_targets: &'a BTreeSet<String>,
        call_func_spill_offset: u32,
        return_alias_summaries: &'a BTreeMap<String, ReturnAliasSummary>,
    ) -> Self {
        Self {
            func_table_slots,
            func_indices,
            trampoline_slots,
            table_base,
            closure_functions,
            escaped_callable_targets,
            call_func_spill_offset,
            return_alias_summaries,
        }
    }

    pub(super) fn table_index(&self, target_name: &str, call_kind: &str) -> u32 {
        let slot = *self
            .func_table_slots
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} table target not found: {target_name}"));
        self.table_base + slot
    }

    pub(super) fn function_index(&self, target_name: &str, call_kind: &str) -> u32 {
        *self
            .func_indices
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} function target not found: {target_name}"))
    }

    pub(super) fn trampoline_table_index(&self, target_name: &str, call_kind: &str) -> u32 {
        let slot = *self
            .trampoline_slots
            .get(target_name)
            .unwrap_or_else(|| panic!("{call_kind} trampoline target not found: {target_name}"));
        self.table_base + slot
    }

    pub(super) fn callable_table_pair(
        &self,
        target_name: &str,
        call_kind: &str,
    ) -> WasmCallableTablePair {
        WasmCallableTablePair {
            function_table_index: self.table_index(target_name, call_kind),
            trampoline_table_index: self.trampoline_table_index(target_name, call_kind),
        }
    }

    pub(super) fn is_closure_function(&self, target_name: &str) -> bool {
        self.closure_functions.contains(target_name)
    }

    pub(super) fn is_escaped_callable(&self, target_name: &str) -> bool {
        self.escaped_callable_targets.contains(target_name)
    }

    pub(super) fn call_func_spill_offset(&self) -> u32 {
        self.call_func_spill_offset
    }

    pub(super) fn returns_alias_param(&self, target_name: &str, args_names: &[String]) -> bool {
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
pub(super) struct WasmCallableTablePair {
    pub(super) function_table_index: u32,
    pub(super) trampoline_table_index: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_site_abi_canonicalizes_callable_indices_and_lifecycle_facts() {
        let table_slots = BTreeMap::from([("callee".to_string(), 7)]);
        let func_indices = BTreeMap::from([("callee".to_string(), 42)]);
        let trampoline_slots = BTreeMap::from([("callee".to_string(), 9)]);
        let closure_functions = BTreeSet::from(["callee".to_string()]);
        let escaped_targets = BTreeSet::from(["callee".to_string()]);
        let return_alias_summaries =
            BTreeMap::from([("callee".to_string(), ReturnAliasSummary::Param(1))]);
        let abi = WasmCallSiteAbi::new(
            &table_slots,
            &func_indices,
            &trampoline_slots,
            100,
            &closure_functions,
            &escaped_targets,
            4096,
            &return_alias_summaries,
        );

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
