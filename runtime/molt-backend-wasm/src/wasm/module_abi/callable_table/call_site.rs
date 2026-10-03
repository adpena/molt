use std::collections::{BTreeMap, BTreeSet};

use super::WasmCallableTablePlan;
use crate::wasm_table::{WasmCallableTableRole, WasmCallableTableTarget};

pub(in crate::wasm) struct WasmCallableCallSiteAbi<'a> {
    func_table_slots: &'a BTreeMap<String, u32>,
    func_indices: &'a BTreeMap<String, u32>,
    trampoline_slots: &'a BTreeMap<String, u32>,
    plan: &'a WasmCallableTablePlan,
    escaped_callable_targets: &'a BTreeSet<String>,
    call_func_spill_offset: u32,
    entry_custody: &'a BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration>,
}

impl<'a> WasmCallableCallSiteAbi<'a> {
    pub(super) fn from_table_plan(
        plan: &'a WasmCallableTablePlan,
        escaped_callable_targets: &'a BTreeSet<String>,
        call_func_spill_offset: u32,
        entry_custody: &'a BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration>,
    ) -> Self {
        Self {
            func_table_slots: &plan.func_to_table_idx,
            func_indices: &plan.func_to_index,
            trampoline_slots: &plan.func_to_trampoline_idx,
            plan,
            escaped_callable_targets,
            call_func_spill_offset,
            entry_custody,
        }
    }

    /// The runtime entry custody of the function object a `func_new`
    /// creates for `target_name`, from that function's own parameter
    /// declaration. A target without one, or whose declaration has no
    /// one-bit runtime encoding, is a compiler error: guessing "borrowed"
    /// would let a transferring body release its caller's references.
    pub(in crate::wasm) fn entry_custody_word(
        &self,
        target_name: &str,
        has_closure: bool,
        arity: i64,
    ) -> u64 {
        let declaration = self.entry_custody.get(target_name).unwrap_or_else(|| {
            panic!("func_new target `{target_name}` has no parameter declaration")
        });
        usize::try_from(arity)
            .map_err(|_| molt_codegen_abi::EntryCustodyError::Signature)
            .and_then(|arity| declaration.encode(has_closure, arity))
            .unwrap_or_else(|error| {
                panic!("func_new target `{target_name}` has no runtime entry custody: {error:?}")
            })
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

    /// Table address of a native initializer handed to the runtime
    /// extension-init transaction instead of being called directly.
    pub(in crate::wasm) fn native_initializer_target(
        &self,
        symbol: &str,
    ) -> WasmCallableTableTarget {
        let slot = *self
            .plan
            .native_initializer_to_table_idx
            .get(symbol)
            .unwrap_or_else(|| panic!("native initializer `{symbol}` has no callable-table slot"));
        self.plan
            .target_for_slot(slot, WasmCallableTableRole::DirectCallable)
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
        self.plan
            .positional_call_shapes
            .get(target_name)
            .is_some_and(|shape| shape.1)
    }

    pub(in crate::wasm) fn positional_arity(&self, target_name: &str) -> Option<usize> {
        self.plan
            .positional_call_shapes
            .get(target_name)
            .map(|shape| shape.0)
    }

    pub(in crate::wasm) fn is_escaped_callable(&self, target_name: &str) -> bool {
        self.escaped_callable_targets.contains(target_name)
    }

    pub(in crate::wasm) fn call_func_spill_offset(&self) -> u32 {
        self.call_func_spill_offset
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
            native_initializer_to_table_idx: BTreeMap::new(),
            app_callable_resolver: None,
            positional_call_shapes: BTreeMap::from([("callee".to_string(), (2, true))]),
            function_abi_returns_value: BTreeMap::from([("callee".to_string(), true)]),
            trampoline_entries: Vec::new(),
        };
        let escaped_targets = BTreeSet::from(["callee".to_string()]);
        let entry_custody = BTreeMap::from([(
            "callee".to_string(),
            molt_codegen_abi::EntryCustodyDeclaration::declare(true, 3, &[false, true, true]),
        )]);
        let abi = plan.call_site_abi(&escaped_targets, 4096, &entry_custody);
        assert_eq!(
            abi.entry_custody_word("callee", true, 2),
            molt_codegen_abi::ENTRY_CUSTODY_ADOPTS
        );

        let table_pair = abi.callable_table_pair("callee", "test_call");
        assert_eq!(table_pair.function.current_table_index, 107);
        assert_eq!(table_pair.trampoline.current_table_index, 109);
        assert_eq!(abi.function_index("callee", "test_call"), 42);
        assert!(abi.function_abi_returns_value("callee"));
        assert!(abi.is_closure_function("callee"));
        assert_eq!(abi.positional_arity("callee"), Some(2));
        assert_eq!(abi.positional_arity("unknown"), None);
        assert!(abi.is_escaped_callable("callee"));
        assert_eq!(abi.call_func_spill_offset(), 4096);
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
            native_initializer_to_table_idx: BTreeMap::new(),
            app_callable_resolver: None,
            positional_call_shapes: BTreeMap::new(),
            function_abi_returns_value: BTreeMap::from([("callee".to_string(), true)]),
            trampoline_entries: Vec::new(),
        };
        let escaped = BTreeSet::new();
        let entry_custody = BTreeMap::new();
        let abi = plan.call_site_abi(&escaped, 0, &entry_custody);
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
