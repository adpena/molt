use crate::wasm::frame_locals::WasmFrameLocals;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::ValType;

#[derive(Clone, Copy)]
pub(super) struct FrameLocalAllocationPolicy<'a> {
    pub(super) read_vars: &'a BTreeSet<String>,
    pub(super) param_set: &'a BTreeSet<String>,
    pub(super) coalesced_map: &'a BTreeMap<String, String>,
    pub(super) dead_sink_idx: u32,
}

pub(super) fn ensure_frame_local(
    locals: &mut WasmFrameLocals,
    local_types: &mut Vec<ValType>,
    local_count: &mut u32,
    policy: FrameLocalAllocationPolicy<'_>,
    name: &str,
    as_dead_out: bool,
) -> u32 {
    if let Some(&idx) = locals.get(name) {
        return idx;
    }
    if as_dead_out && !policy.read_vars.contains(name) && !policy.param_set.contains(name) {
        locals.insert(name.to_string(), policy.dead_sink_idx);
        return policy.dead_sink_idx;
    }
    if let Some(repr) = policy.coalesced_map.get(name)
        && repr != name
        && let Some(&repr_idx) = locals.get(repr)
    {
        locals.insert(name.to_string(), repr_idx);
        return repr_idx;
    }
    let idx = *local_count;
    locals.insert(name.to_string(), idx);
    local_types.push(ValType::I64);
    *local_count += 1;
    idx
}
