use std::collections::BTreeSet;

use crate::ir::OpIR;

use super::cfg::CFG;
use super::simple_def_use::{simple_ir_defined_names, simple_ir_read_names};

/// Exact name liveness over the canonical SimpleIR CFG.
///
/// The graph includes structured and unstructured successors plus implicit
/// exception and state-resume edges. `live_after_op` is therefore suitable for
/// backend block transport; a linear last-use index is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCfgLiveness {
    pub live_in_by_block: Vec<BTreeSet<String>>,
    pub live_out_by_block: Vec<BTreeSet<String>>,
    pub live_after_op: Vec<BTreeSet<String>>,
    pub op_to_block: Vec<usize>,
}

impl SimpleCfgLiveness {
    pub fn live_after(&self, op_idx: usize) -> &BTreeSet<String> {
        self.live_after_op
            .get(op_idx)
            .unwrap_or_else(|| panic!("SimpleIR op index {op_idx} is outside liveness plan"))
    }

    pub fn block_for_op(&self, op_idx: usize) -> usize {
        *self
            .op_to_block
            .get(op_idx)
            .unwrap_or_else(|| panic!("SimpleIR op index {op_idx} is outside CFG"))
    }
}

pub fn analyze_simple_cfg_liveness(ops: &[OpIR]) -> SimpleCfgLiveness {
    if ops.is_empty() {
        return SimpleCfgLiveness {
            live_in_by_block: Vec::new(),
            live_out_by_block: Vec::new(),
            live_after_op: Vec::new(),
            op_to_block: Vec::new(),
        };
    }

    let cfg = CFG::build(ops);
    let block_count = cfg.blocks.len();
    let mut successors = cfg.successors.clone();
    for &(from, to) in &cfg.exception_edges {
        if !successors[from].contains(&to) {
            successors[from].push(to);
        }
    }
    for &(from, to, _) in &cfg.state_resume_edges {
        if !successors[from].contains(&to) {
            successors[from].push(to);
        }
    }
    for edges in &mut successors {
        edges.sort_unstable();
        edges.dedup();
    }

    let mut block_uses = vec![BTreeSet::new(); block_count];
    let mut block_defs = vec![BTreeSet::new(); block_count];
    let mut op_to_block = vec![0; ops.len()];
    for block in &cfg.blocks {
        let uses = &mut block_uses[block.id];
        let defs = &mut block_defs[block.id];
        for op_idx in block.start_op..block.end_op {
            op_to_block[op_idx] = block.id;
            for name in simple_ir_read_names(&ops[op_idx]) {
                if !defs.contains(&name) {
                    uses.insert(name);
                }
            }
            defs.extend(simple_ir_defined_names(&ops[op_idx]));
        }
    }

    let mut live_in_by_block = vec![BTreeSet::new(); block_count];
    let mut live_out_by_block = vec![BTreeSet::new(); block_count];
    loop {
        let mut changed = false;
        for block_id in (0..block_count).rev() {
            let mut live_out = BTreeSet::new();
            for &successor in &successors[block_id] {
                live_out.extend(live_in_by_block[successor].iter().cloned());
            }
            let mut live_in = block_uses[block_id].clone();
            live_in.extend(
                live_out
                    .iter()
                    .filter(|name| !block_defs[block_id].contains(*name))
                    .cloned(),
            );
            if live_out != live_out_by_block[block_id] {
                live_out_by_block[block_id] = live_out;
                changed = true;
            }
            if live_in != live_in_by_block[block_id] {
                live_in_by_block[block_id] = live_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut live_after_op = vec![BTreeSet::new(); ops.len()];
    for block in &cfg.blocks {
        let mut live = live_out_by_block[block.id].clone();
        for op_idx in (block.start_op..block.end_op).rev() {
            live_after_op[op_idx] = live.clone();
            for name in simple_ir_defined_names(&ops[op_idx]) {
                live.remove(&name);
            }
            live.extend(simple_ir_read_names(&ops[op_idx]));
        }
    }

    SimpleCfgLiveness {
        live_in_by_block,
        live_out_by_block,
        live_after_op,
        op_to_block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    #[test]
    fn branch_target_use_is_live_across_conditional_edge() {
        let mut branch = op("br_if");
        branch.args = Some(vec!["cond".into()]);
        branch.value = Some(7);
        let mut fallthrough_return = op("ret");
        fallthrough_return.var = Some("fallback".into());
        let mut label = op("label");
        label.value = Some(7);
        let mut target_return = op("ret");
        target_return.var = Some("carried".into());
        let ops = vec![branch, fallthrough_return, label, target_return];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(plan.live_after(0).contains("carried"));
        assert!(plan.live_after(0).contains("fallback"));
    }

    #[test]
    fn exception_target_use_is_live_across_implicit_edge() {
        let mut check = op("check_exception");
        check.value = Some(9);
        let mut normal_return = op("ret");
        normal_return.var = Some("normal".into());
        let mut handler = op("label");
        handler.value = Some(9);
        let mut handler_return = op("ret");
        handler_return.var = Some("exception_context".into());
        let ops = vec![check, normal_return, handler, handler_return];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(plan.live_after(0).contains("exception_context"));
        assert!(plan.live_after(0).contains("normal"));
    }

    #[test]
    fn exception_fallthrough_carries_values_defined_inside_the_current_block() {
        let mut define = op("const_str");
        define.out = Some("carried".into());
        let mut check = op("check_exception");
        check.value = Some(9);
        let mut consume = op("print");
        consume.args = Some(vec!["carried".into()]);
        let normal_return = op("ret_void");
        let mut handler = op("label");
        handler.value = Some(9);
        let handler_return = op("ret_void");
        let ops = vec![
            define,
            check,
            consume,
            normal_return,
            handler,
            handler_return,
        ];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(plan.live_after(1).contains("carried"));
        assert!(
            !plan.live_in_by_block[plan.block_for_op(1)].contains("carried"),
            "block-entry liveness cannot represent a value defined before an intra-block exception split",
        );
    }

    #[test]
    fn state_dispatch_use_is_live_across_resume_edge() {
        let switch = op("state_switch");
        let mut suspend = op("state_yield");
        suspend.value = Some(3);
        let mut resume = op("state_label");
        resume.value = Some(3);
        let mut resumed_return = op("ret");
        resumed_return.var = Some("frame_value".into());
        let ops = vec![switch, suspend, resume, resumed_return];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(plan.live_in_by_block[plan.block_for_op(0)].contains("frame_value"));
    }

    #[test]
    fn loop_backedge_reaches_fixed_point() {
        let mut label = op("label");
        label.value = Some(1);
        let mut update = op("copy");
        update.args = Some(vec!["carried".into()]);
        update.out = Some("next".into());
        let mut jump = op("jump");
        jump.value = Some(1);
        let ops = vec![label, update, jump];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(plan.live_after(2).contains("carried"));
    }

    #[test]
    fn definition_kills_prior_value_within_block() {
        let mut define = op("const_int");
        define.out = Some("value".into());
        let mut ret = op("ret");
        ret.var = Some("value".into());
        let plan = analyze_simple_cfg_liveness(&[define, ret]);

        assert!(plan.live_after(0).contains("value"));
        assert!(!plan.live_in_by_block[0].contains("value"));
    }
}
