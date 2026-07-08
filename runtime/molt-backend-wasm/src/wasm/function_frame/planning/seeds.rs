use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm_values::box_none;
use std::collections::BTreeSet;
use wasm_encoder::ValType;

#[derive(Default)]
pub(super) struct FrameConstSeedPlan {
    seen: BTreeSet<String>,
    inline_locals: Vec<(u32, i64)>,
    runtime_ops: Vec<(usize, OpIR)>,
}

impl FrameConstSeedPlan {
    pub(super) fn observe_const_output(
        &mut self,
        op_idx: usize,
        op: &OpIR,
        out: &str,
        out_local_idx: u32,
        is_dead: bool,
        locals: &mut WasmFrameLocals,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) {
        if let Some(const_policy) = WasmConstOpPolicy::for_op(op) {
            locals.ensure_literal_scratch_for_policy(out, const_policy, local_types, local_count);
            if !self.seen.contains(out) {
                if let Some(bits) = const_policy.inline_seed_bits(op) {
                    if !is_dead {
                        self.seen.insert(out.to_string());
                        self.inline_locals.push((out_local_idx, bits));
                    }
                } else if const_policy.needs_dispatch_runtime_seed() && !is_dead {
                    self.seen.insert(out.to_string());
                    self.runtime_ops.push((op_idx, op.clone()));
                }
            }
        }
    }

    pub(super) fn seed_undefined_locals(
        &mut self,
        used_vars: &BTreeSet<String>,
        defined_vars: &BTreeSet<String>,
        param_set: &BTreeSet<String>,
        locals: &WasmFrameLocals,
        dead_sink_idx: u32,
    ) {
        for undef in used_vars.difference(defined_vars) {
            if let Some(&local_idx) = locals.get(undef.as_str())
                && local_idx != dead_sink_idx
                && !param_set.contains(undef.as_str())
                && !self.seen.contains(undef)
            {
                self.seen.insert(undef.clone());
                self.inline_locals.push((local_idx, box_none()));
            }
        }
    }

    pub(super) fn into_dispatch_seeds(
        self,
        needs_dispatch: bool,
    ) -> (Vec<(u32, i64)>, Vec<(usize, OpIR)>, BTreeSet<usize>) {
        if !needs_dispatch {
            return (Vec::new(), Vec::new(), BTreeSet::new());
        }
        let runtime_op_indices = self.runtime_ops.iter().map(|(idx, _)| *idx).collect();
        (self.inline_locals, self.runtime_ops, runtime_op_indices)
    }
}
