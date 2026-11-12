use std::collections::BTreeMap;

use super::callable_table::CallableIdentity;
use crate::wasm_abi::{POLL_TABLE_IMPORTS, RUNTIME_POLL_CALLABLE_KEY_BASE};
use crate::wasm_options::WasmRuntimePollSlots;

/// The runtime polls the bound runtime defines, named by their keys.
///
/// A runtime poll is its key on every target (HF-699 K1): the manifest's
/// `poll_table_slot` is the key's offset from `RUNTIME_POLL_CALLABLE_KEY_BASE`,
/// never a table slot, and the app's callable table reserves no slot for it.
/// Only polls the bound runtime defines get an identity, so no compiled task
/// can name a poll the runtime lacks (HF-487).
pub(super) struct WasmRuntimePollKeys {
    runtime_slots: WasmRuntimePollSlots,
}

impl WasmRuntimePollKeys {
    pub(super) fn new(runtime_slots: WasmRuntimePollSlots) -> Self {
        Self { runtime_slots }
    }

    /// Seeds each defined runtime poll's identity with its key constant
    /// (`RUNTIME_POLL_CALLABLE_KEY_BASE` + `poll_table_slot`), not a table
    /// slot (HF-699 K1, K5). The runtime resolves the key through its
    /// generated key table on every target.
    pub(super) fn seed_identities(&self, identities: &mut BTreeMap<String, CallableIdentity>) {
        for spec in POLL_TABLE_IMPORTS
            .iter()
            .filter(|spec| self.runtime_slots.defines_slot(spec.table_slot))
        {
            identities.insert(
                spec.import.runtime_export_name().to_string(),
                CallableIdentity::RuntimeKey(runtime_poll_key(spec.table_slot)),
            );
        }
    }
}

/// The key of the runtime poll at manifest `poll_table_slot` (HF-699 K1).
pub(super) fn runtime_poll_key(table_slot: u32) -> u64 {
    RUNTIME_POLL_CALLABLE_KEY_BASE + u64::from(table_slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_polls_the_runtime_defines_get_a_key() {
        // A runtime built without stdlib_net defines no networking poll
        // (HF-487): no compiled task may name one, so it gets no identity.
        let sleep = POLL_TABLE_IMPORTS
            .iter()
            .find(|spec| spec.import.runtime_export_name() == "molt_async_sleep_poll")
            .expect("async sleep poll");
        let io_wait = POLL_TABLE_IMPORTS
            .iter()
            .find(|spec| spec.import.runtime_export_name() == "molt_io_wait")
            .expect("io wait poll");
        let mask = WasmRuntimePollSlots::all().mask() & !(1u64 << io_wait.table_slot);
        let mut identities = BTreeMap::new();
        WasmRuntimePollKeys::new(WasmRuntimePollSlots::from_mask(mask).expect("subset"))
            .seed_identities(&mut identities);
        assert_eq!(
            identities.get("molt_async_sleep_poll"),
            Some(&CallableIdentity::RuntimeKey(
                RUNTIME_POLL_CALLABLE_KEY_BASE + u64::from(sleep.table_slot)
            ))
        );
        assert!(!identities.contains_key("molt_io_wait"));
        assert!(
            identities
                .values()
                .all(|identity| matches!(identity, CallableIdentity::RuntimeKey(_))),
            "a runtime poll is never a table slot (HF-699 K1)"
        );
    }
}
