use crate::wasm::frame_locals::WasmFrameLocals;
use crate::wasm::local_analysis::LocalStoragePlan;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::ValType;

pub(super) struct FrameLocalAllocator<'a> {
    read_vars: &'a BTreeSet<String>,
    param_set: &'a BTreeSet<String>,
    storage: &'a LocalStoragePlan,
    dead_sink_idx: u32,
    /// Physical local created for each shared storage slot on first use.
    slot_locals: BTreeMap<u32, u32>,
}

impl<'a> FrameLocalAllocator<'a> {
    pub(super) fn new(
        read_vars: &'a BTreeSet<String>,
        param_set: &'a BTreeSet<String>,
        storage: &'a LocalStoragePlan,
        dead_sink_idx: u32,
    ) -> Self {
        Self {
            read_vars,
            param_set,
            storage,
            dead_sink_idx,
            slot_locals: BTreeMap::new(),
        }
    }

    pub(super) fn ensure(
        &mut self,
        locals: &mut WasmFrameLocals,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
        name: &str,
        as_dead_out: bool,
    ) -> u32 {
        if let Some(&idx) = locals.get(name) {
            return idx;
        }
        if as_dead_out && !self.read_vars.contains(name) && !self.param_set.contains(name) {
            // Preserve the physical slot's synthetic kind on every dead-output
            // alias. Treating these names as ordinary Value locals lets call-site
            // retention inspect and INCREF stale bytes left in the shared sink.
            locals.insert_dead_sink_alias(name.to_string(), self.dead_sink_idx);
            return self.dead_sink_idx;
        }
        let slot = self.storage.shared_slot(name);
        if let Some(&idx) = slot.and_then(|slot| self.slot_locals.get(&slot)) {
            locals.insert(name.to_string(), idx);
            return idx;
        }
        let idx = *local_count;
        locals.insert(name.to_string(), idx);
        local_types.push(ValType::I64);
        *local_count += 1;
        if let Some(slot) = slot {
            self.slot_locals.insert(slot, idx);
        }
        idx
    }
}
