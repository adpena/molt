use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::frame_locals::{WasmFrameAnonymousLocal, WasmLiteralScratchLocals};
use crate::wasm_abi_generated::WasmConstLiteralPayload;
use crate::wasm_values::box_none;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::ValType;

#[derive(Clone)]
pub(in crate::wasm::function_frame) struct FrameConstAnchor {
    pub(in crate::wasm::function_frame) local: u32,
    pub(in crate::wasm::function_frame) op: OpIR,
    pub(in crate::wasm::function_frame) scratch_aliases: Vec<WasmLiteralScratchLocals>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum FrameConstAnchorKey {
    RuntimeSingleton(String),
    Literal { kind: String, bytes: Vec<u8> },
    Integer(Vec<u8>),
}

#[derive(Default)]
pub(super) struct FrameConstSeedPlan {
    seen_inline_outputs: BTreeSet<String>,
    inline_locals: Vec<(u32, i64)>,
    anchors: Vec<FrameConstAnchor>,
    anchor_by_key: BTreeMap<FrameConstAnchorKey, usize>,
    anchor_by_op_index: BTreeMap<usize, u32>,
    anchor_scratch_seen: BTreeSet<(usize, u32, u32)>,
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
            let literal_scratch = locals.ensure_literal_scratch_for_policy(
                out,
                const_policy,
                local_types,
                local_count,
            );
            if let Some(bits) = const_policy.inline_seed_bits(op) {
                if !is_dead && self.seen_inline_outputs.insert(out.to_string()) {
                    self.inline_locals.push((out_local_idx, bits));
                }
                return;
            }
            if !const_policy.needs_runtime_anchor() {
                return;
            }

            let key = anchor_key(const_policy, op);
            let anchor_index = if let Some(&index) = self.anchor_by_key.get(&key) {
                index
            } else {
                let local = locals.allocate_anonymous(
                    WasmFrameAnonymousLocal::ConstLiteralAnchor,
                    local_types,
                    local_count,
                );
                let index = self.anchors.len();
                self.anchor_by_key.insert(key, index);
                self.anchors.push(FrameConstAnchor {
                    local,
                    op: op.clone(),
                    scratch_aliases: Vec::new(),
                });
                index
            };
            let anchor = &mut self.anchors[anchor_index];
            if let Some(scratch) = literal_scratch
                && self.anchor_scratch_seen.insert((
                    anchor_index,
                    scratch.ptr_local(),
                    scratch.len_local(),
                ))
            {
                anchor.scratch_aliases.push(scratch);
            }
            self.anchor_by_op_index.insert(op_idx, anchor.local);
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
                && !self.seen_inline_outputs.contains(undef)
            {
                self.seen_inline_outputs.insert(undef.clone());
                self.inline_locals.push((local_idx, box_none()));
            }
        }
    }

    pub(super) fn into_frame_plans(
        self,
        needs_dispatch: bool,
    ) -> (Vec<(u32, i64)>, Vec<FrameConstAnchor>, BTreeMap<usize, u32>) {
        let inline_locals = if needs_dispatch {
            self.inline_locals
        } else {
            Vec::new()
        };
        (inline_locals, self.anchors, self.anchor_by_op_index)
    }
}

fn anchor_key(policy: WasmConstOpPolicy, op: &OpIR) -> FrameConstAnchorKey {
    match policy.literal_payload() {
        WasmConstLiteralPayload::None if op.kind == "const" => FrameConstAnchorKey::Integer(
            op.value
                .unwrap_or_else(|| panic!("const requires an i64 payload"))
                .to_string()
                .into_bytes(),
        ),
        WasmConstLiteralPayload::BigintDecimal => {
            FrameConstAnchorKey::Integer(policy.required_simple_ir_literal_bytes(op).to_vec())
        }
        WasmConstLiteralPayload::None => FrameConstAnchorKey::RuntimeSingleton(op.kind.clone()),
        _ => FrameConstAnchorKey::Literal {
            kind: op.kind.clone(),
            bytes: policy.required_simple_ir_literal_bytes(op).to_vec(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionIR;
    use crate::wasm::function_frame::WasmFunctionFramePlan;

    fn literal(kind: &str, out: &str, payload: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: Some(out.to_string()),
            s_value: Some(payload.to_string()),
            ..OpIR::default()
        }
    }

    fn function(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: "literal_anchors".to_string(),
            ops,
            ..FunctionIR::default()
        }
    }

    #[test]
    fn equal_literal_payloads_share_anchor_and_preserve_each_scratch_alias() {
        let plan = WasmFunctionFramePlan::for_function(&function(vec![
            literal("const_str", "first", "same"),
            literal("const_str", "second", "same"),
        ]));
        let (_, frame) = plan.into_function_and_frame();

        assert_eq!(frame.const_anchors.len(), 1);
        assert_eq!(frame.const_anchors[0].scratch_aliases.len(), 2);
        assert_eq!(frame.const_anchor_for_op(0), frame.const_anchor_for_op(1));
    }

    #[test]
    fn full_i64_gets_anchor_while_inline_i47_stays_immediate() {
        let mut inline = literal("const", "inline", "");
        inline.s_value = None;
        inline.value = Some(7);
        let mut wide = literal("const", "wide", "");
        wide.s_value = None;
        wide.value = Some(i64::MAX);
        let plan = WasmFunctionFramePlan::for_function(&function(vec![inline, wide]));
        let (_, frame) = plan.into_function_and_frame();

        assert_eq!(frame.const_anchors.len(), 1);
        assert_eq!(frame.const_anchor_for_op(0), None);
        assert!(frame.const_anchor_for_op(1).is_some());
    }

    #[test]
    fn equivalent_full_i64_and_bigint_share_one_owner() {
        let mut wide = literal("const", "wide", "");
        wide.s_value = None;
        wide.value = Some(i64::MAX);
        let bigint = literal("const_bigint", "big", &i64::MAX.to_string());
        for ops in [vec![wide.clone(), bigint.clone()], vec![bigint, wide]] {
            let (_, frame) =
                WasmFunctionFramePlan::for_function(&function(ops)).into_function_and_frame();
            assert_eq!(frame.const_anchors.len(), 1);
            assert_eq!(frame.const_anchor_for_op(0), frame.const_anchor_for_op(1));
        }
    }
}
