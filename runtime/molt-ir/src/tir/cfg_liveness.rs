use std::collections::{BTreeSet, HashMap};

use crate::ir::OpIR;

use super::cfg::CFG;
use super::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};

/// Exact liveness over the canonical SimpleIR CFG, using its one name table.
///
/// Block boundaries retain compact bitsets. Operation boundaries retain sparse
/// IDs, so long functions with few live values do not allocate a full name-width
/// bitset per operation. Names are materialized only by diagnostic consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCfgLiveness {
    pub names: SimpleNameTable,
    pub live_in_by_block: Vec<SimpleNameSet>,
    pub live_out_by_block: Vec<SimpleNameSet>,
    pub live_after_op: Vec<Vec<u32>>,
    pub op_to_block: Vec<usize>,
}

impl SimpleCfgLiveness {
    pub fn live_after(&self, op_idx: usize) -> &[u32] {
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
    analyze_simple_cfg_liveness_with_cfg(ops, &CFG::build(ops))
}

pub fn analyze_simple_cfg_liveness_with_cfg(ops: &[OpIR], cfg: &CFG) -> SimpleCfgLiveness {
    let facts = analyze_simple_cfg_liveness_facts_with_cfg(ops, cfg);
    let mut live_after_op = vec![Vec::new(); ops.len()];
    for block in 0..facts.block_count() {
        facts.visit_block_backward(ops, block, |op_idx, live| {
            live_after_op[op_idx] = live.iter().collect();
        });
    }
    SimpleCfgLiveness {
        names: facts.names,
        live_in_by_block: facts.live_in_by_block,
        live_out_by_block: facts.live_out_by_block,
        live_after_op,
        op_to_block: facts.op_to_block,
    }
}

/// Dense identities for every name a SimpleIR function reads or defines,
/// assigned in first-appearance order (reads before definitions within an
/// operation) so every projection is deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleNameTable {
    names: Vec<String>,
    ids: HashMap<String, u32>,
}

impl SimpleNameTable {
    fn for_ops(ops: &[OpIR]) -> Self {
        let mut table = Self::default();
        for op in ops {
            visit_simple_ir_reads(op, |read| table.intern(read.name));
            visit_simple_ir_defined_names(op, |name| table.intern(name));
        }
        table
    }

    fn intern(&mut self, name: &str) {
        if self.ids.contains_key(name) {
            return;
        }
        let id = u32::try_from(self.names.len()).expect("SimpleIR function exceeds u32 names");
        self.ids.insert(name.to_string(), id);
        self.names.push(name.to_string());
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn id(&self, name: &str) -> Option<u32> {
        self.ids.get(name).copied()
    }

    pub fn name(&self, id: u32) -> &str {
        &self.names[id as usize]
    }

    pub fn materialize(&self, set: &SimpleNameSet) -> BTreeSet<String> {
        set.iter().map(|id| self.name(id).to_string()).collect()
    }
}

/// A set of [`SimpleNameTable`] identities, one bit per name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleNameSet {
    words: Vec<u64>,
}

impl SimpleNameSet {
    fn empty(names: usize) -> Self {
        Self {
            words: vec![0; names.div_ceil(64)],
        }
    }

    pub fn contains(&self, id: u32) -> bool {
        let (word, bit) = Self::bit(id);
        self.words.get(word).is_some_and(|bits| (bits & bit) != 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> {
        self.words.iter().enumerate().flat_map(|(word, &bits)| {
            let mut remaining = bits;
            std::iter::from_fn(move || {
                if remaining == 0 {
                    return None;
                }
                let offset = remaining.trailing_zeros();
                remaining &= remaining - 1;
                Some(word as u32 * 64 + offset)
            })
        })
    }

    fn insert(&mut self, id: u32) {
        let (word, bit) = Self::bit(id);
        self.words[word] |= bit;
    }

    fn remove(&mut self, id: u32) {
        let (word, bit) = Self::bit(id);
        self.words[word] &= !bit;
    }

    fn union_with(&mut self, other: &Self) {
        for (bits, other) in self.words.iter_mut().zip(&other.words) {
            *bits |= other;
        }
    }

    fn bit(id: u32) -> (usize, u64) {
        ((id / 64) as usize, 1 << (id % 64))
    }
}

/// Exact block-level liveness over the canonical SimpleIR CFG in compact
/// form. This is the single dataflow solution: name-keyed and per-operation
/// views are projections of it. Each boundary set array takes
/// `blocks * names / 8` bytes; no per-operation set is materialized here.
#[derive(Debug, Clone)]
pub struct SimpleCfgLivenessFacts {
    pub names: SimpleNameTable,
    /// Half-open operation range of each CFG block, in operation order.
    pub block_ops: Vec<(usize, usize)>,
    pub op_to_block: Vec<usize>,
    pub live_in_by_block: Vec<SimpleNameSet>,
    pub live_out_by_block: Vec<SimpleNameSet>,
}

impl SimpleCfgLivenessFacts {
    pub fn block_count(&self) -> usize {
        self.block_ops.len()
    }

    /// Replay `block` backwards from its exact live-out set. `visit` observes
    /// each operation, last first, with the set live immediately after it;
    /// the operation's transfer (definitions kill, reads generate) is applied
    /// before the preceding operation is visited.
    pub fn visit_block_backward(
        &self,
        ops: &[OpIR],
        block: usize,
        mut visit: impl FnMut(usize, &SimpleNameSet),
    ) {
        let (start, end) = self.block_ops[block];
        let mut live = self.live_out_by_block[block].clone();
        for op_idx in (start..end).rev() {
            visit(op_idx, &live);
            visit_simple_ir_defined_names(&ops[op_idx], |name| {
                if let Some(id) = self.names.id(name) {
                    live.remove(id);
                }
            });
            visit_simple_ir_reads(&ops[op_idx], |read| {
                if let Some(id) = self.names.id(read.name) {
                    live.insert(id);
                }
            });
        }
    }
}

/// The edge relation liveness is solved over: structured and unstructured
/// successors plus implicit exception edges (including a transfer back to its
/// own block's leader) and state-resume edges.
pub fn liveness_block_successors(cfg: &CFG) -> Vec<Vec<usize>> {
    let mut successors = cfg.successors.clone();
    for &(from, to) in &cfg.exception_edges {
        successors[from].push(to);
    }
    for &(from, to, _) in &cfg.state_resume_edges {
        successors[from].push(to);
    }
    for edges in &mut successors {
        edges.sort_unstable();
        edges.dedup();
    }
    successors
}

pub fn analyze_simple_cfg_liveness_facts(ops: &[OpIR]) -> SimpleCfgLivenessFacts {
    analyze_simple_cfg_liveness_facts_with_cfg(ops, &CFG::build(ops))
}

pub fn analyze_simple_cfg_liveness_facts_with_cfg(
    ops: &[OpIR],
    cfg: &CFG,
) -> SimpleCfgLivenessFacts {
    let names = SimpleNameTable::for_ops(ops);
    if ops.is_empty() {
        return SimpleCfgLivenessFacts {
            names,
            block_ops: Vec::new(),
            op_to_block: Vec::new(),
            live_in_by_block: Vec::new(),
            live_out_by_block: Vec::new(),
        };
    }

    let successors = liveness_block_successors(cfg);
    let block_count = cfg.blocks.len();
    let empty = SimpleNameSet::empty(names.len());
    // Upward-exposed reads and definitions are sparse per block; only the
    // boundary sets that the fixed point propagates are dense.
    let mut block_uses: Vec<Vec<u32>> = Vec::with_capacity(block_count);
    let mut block_defs: Vec<Vec<u32>> = Vec::with_capacity(block_count);
    let mut defined_in_block = empty.clone();
    let mut block_ops = Vec::with_capacity(block_count);
    let mut op_to_block = vec![0; ops.len()];
    for block in &cfg.blocks {
        block_ops.push((block.start_op, block.end_op));
        let mut uses = Vec::new();
        let mut defs = Vec::new();
        for op_idx in block.start_op..block.end_op {
            op_to_block[op_idx] = block.id;
            visit_simple_ir_reads(&ops[op_idx], |read| {
                let id = names.id(read.name).expect("interned SimpleIR name");
                if !defined_in_block.contains(id) {
                    uses.push(id);
                }
            });
            visit_simple_ir_defined_names(&ops[op_idx], |name| {
                let id = names.id(name).expect("interned SimpleIR name");
                if !defined_in_block.contains(id) {
                    defined_in_block.insert(id);
                    defs.push(id);
                }
            });
        }
        for &id in &defs {
            defined_in_block.remove(id);
        }
        block_uses.push(uses);
        block_defs.push(defs);
    }

    let mut live_in_by_block = vec![empty.clone(); block_count];
    let mut live_out_by_block = vec![empty.clone(); block_count];
    let mut changed = true;
    while changed {
        changed = false;
        for block_id in (0..block_count).rev() {
            let mut live_out = empty.clone();
            for &successor in &successors[block_id] {
                live_out.union_with(&live_in_by_block[successor]);
            }
            let mut live_in = live_out.clone();
            for &id in &block_defs[block_id] {
                live_in.remove(id);
            }
            for &id in &block_uses[block_id] {
                live_in.insert(id);
            }
            if live_out != live_out_by_block[block_id] {
                live_out_by_block[block_id] = live_out;
                changed = true;
            }
            if live_in != live_in_by_block[block_id] {
                live_in_by_block[block_id] = live_in;
                changed = true;
            }
        }
    }

    SimpleCfgLivenessFacts {
        names,
        block_ops,
        op_to_block,
        live_in_by_block,
        live_out_by_block,
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
        fallthrough_return.args = Some(vec!["fallback".into()]);
        let mut label = op("label");
        label.value = Some(7);
        let mut target_return = op("ret");
        target_return.args = Some(vec!["carried".into()]);
        let ops = vec![branch, fallthrough_return, label, target_return];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(
            plan.live_after(0)
                .contains(&plan.names.id("carried").unwrap())
        );
        assert!(
            plan.live_after(0)
                .contains(&plan.names.id("fallback").unwrap())
        );
    }

    #[test]
    fn exception_target_use_is_live_across_implicit_edge() {
        let mut check = op("check_exception");
        check.value = Some(9);
        let mut normal_return = op("ret");
        normal_return.args = Some(vec!["normal".into()]);
        let mut handler = op("label");
        handler.value = Some(9);
        let mut handler_return = op("ret");
        handler_return.args = Some(vec!["exception_context".into()]);
        let ops = vec![check, normal_return, handler, handler_return];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(
            plan.live_after(0)
                .contains(&plan.names.id("exception_context").unwrap())
        );
        assert!(
            plan.live_after(0)
                .contains(&plan.names.id("normal").unwrap())
        );
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

        assert!(
            plan.live_after(1)
                .contains(&plan.names.id("carried").unwrap())
        );
        assert!(
            !plan.live_in_by_block[plan.block_for_op(1)]
                .contains(plan.names.id("carried").unwrap()),
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
        resumed_return.args = Some(vec!["frame_value".into()]);
        let ops = vec![switch, suspend, resume, resumed_return];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert!(
            plan.live_in_by_block[plan.block_for_op(0)]
                .contains(plan.names.id("frame_value").unwrap())
        );
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

        assert!(
            plan.live_after(2)
                .contains(&plan.names.id("carried").unwrap())
        );
    }

    #[test]
    fn definition_kills_prior_value_within_block() {
        let mut define = op("const_int");
        define.out = Some("value".into());
        let mut ret = op("ret");
        ret.args = Some(vec!["value".into()]);
        let plan = analyze_simple_cfg_liveness(&[define, ret]);

        assert!(
            plan.live_after(0)
                .contains(&plan.names.id("value").unwrap())
        );
        assert!(!plan.live_in_by_block[0].contains(plan.names.id("value").unwrap()));
    }

    #[test]
    fn exception_transfer_back_to_its_own_block_keeps_reentry_reads_live() {
        let mut handler = op("label");
        handler.value = Some(5);
        let mut read = op("print");
        read.args = Some(vec!["carried".into()]);
        let mut define = op("const_int");
        define.out = Some("other".into());
        let mut check = op("check_exception");
        check.value = Some(5);
        let ops = vec![handler, read, define, check, op("ret_void")];

        let plan = analyze_simple_cfg_liveness(&ops);

        assert_eq!(plan.block_for_op(0), plan.block_for_op(3));
        assert!(
            plan.live_after(3)
                .contains(&plan.names.id("carried").unwrap()),
            "the transfer re-enters the block that reads the value"
        );
        assert!(
            plan.live_after(2)
                .contains(&plan.names.id("carried").unwrap())
        );
    }

    #[test]
    fn name_sets_address_every_word_boundary() {
        let mut set = SimpleNameSet::empty(130);
        for id in [0, 63, 64, 127, 129] {
            set.insert(id);
        }
        set.remove(63);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0, 64, 127, 129]);
        assert!(set.contains(129) && !set.contains(63) && !set.contains(128));
        assert!(
            !set.contains(4096),
            "identities outside the table are absent"
        );
    }

    #[test]
    fn compact_facts_keep_first_appearance_identities_and_block_boundaries() {
        let mut define = op("add");
        define.args = Some(vec!["b".into(), "a".into()]);
        define.out = Some("c".into());
        let mut check = op("check_exception");
        check.value = Some(4);
        let mut consume = op("print");
        consume.args = Some(vec!["c".into()]);
        let mut handler = op("label");
        handler.value = Some(4);
        let mut handler_read = op("print");
        handler_read.args = Some(vec!["a".into()]);
        let ops = vec![
            define,
            check,
            consume,
            op("ret_void"),
            handler,
            handler_read,
        ];

        let facts = analyze_simple_cfg_liveness_facts(&ops);

        assert_eq!(
            (0..facts.names.len() as u32)
                .map(|id| facts.names.name(id))
                .collect::<Vec<_>>(),
            vec!["b", "a", "c"]
        );
        assert_eq!(facts.block_ops, vec![(0, 2), (2, 4), (4, 6)]);
        let live_out: Vec<_> = facts.live_out_by_block[0]
            .iter()
            .map(|id| facts.names.name(id))
            .collect();
        assert_eq!(live_out, vec!["a", "c"], "fallthrough and handler reads");
        assert!(analyze_simple_cfg_liveness_facts(&[]).names.is_empty());
    }

    #[test]
    fn projection_matches_exhaustive_path_liveness_on_generated_control_flow() {
        for seed in 0..256 {
            let ops = generated_ops(seed);
            let plan = analyze_simple_cfg_liveness(&ops);
            let oracle = exhaustive_live_after(&ops);
            let observed: Vec<BTreeSet<String>> = plan
                .live_after_op
                .iter()
                .map(|ids| {
                    ids.iter()
                        .map(|&id| plan.names.name(id).to_owned())
                        .collect()
                })
                .collect();
            assert_eq!(observed, oracle, "seed {seed}: {ops:?}");
            for (block, live_in) in plan.live_in_by_block.iter().enumerate() {
                let first = plan
                    .op_to_block
                    .iter()
                    .position(|&owner| owner == block)
                    .unwrap_or_else(|| {
                        assert_eq!(block, 0, "only invocation can lack source operations");
                        assert_ne!(plan.op_to_block[0], 0);
                        // Empty invocation has the same live-in environment
                        // as the first source operation it enters.
                        0
                    });
                assert_eq!(
                    plan.names.materialize(live_in),
                    live_before(&ops[first], &oracle[first]),
                    "seed {seed}, block {block}: {ops:?}"
                );
            }
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }

    const GENERATED_NAMES: [&str; 5] = ["a", "b", "c", "d", "e"];

    fn pick(rng: &mut Lcg) -> String {
        GENERATED_NAMES[rng.below(GENERATED_NAMES.len() as u64) as usize].to_string()
    }

    fn generated_op(rng: &mut Lcg) -> OpIR {
        let label = 1 + rng.below(3) as i64;
        let mut generated = OpIR::default();
        match rng.below(9) {
            0 | 1 => {
                generated.kind = "add".into();
                generated.args = Some(vec![pick(rng), pick(rng)]);
                generated.out = Some(pick(rng));
            }
            2 => {
                generated.kind = "store_var".into();
                generated.args = Some(vec![pick(rng)]);
                generated.var = Some(pick(rng));
            }
            3 => {
                generated.kind = "print".into();
                generated.args = Some(vec![pick(rng)]);
            }
            4 => {
                generated.kind = "jump".into();
                generated.value = Some(label);
            }
            5 => {
                generated.kind = "br_if".into();
                generated.args = Some(vec![pick(rng)]);
                generated.value = Some(label);
            }
            6 => {
                generated.kind = "check_exception".into();
                generated.value = Some(label);
            }
            7 => {
                generated.kind = "ret".into();
                generated.args = Some(vec![pick(rng)]);
            }
            _ => {
                generated.kind = "const_int".into();
                generated.out = Some(pick(rng));
            }
        }
        generated
    }

    /// Unstructured bodies with forward, backward and exception transfers to
    /// three labels, dead code after terminators, and reads that precede
    /// every definition.
    fn generated_ops(seed: u64) -> Vec<OpIR> {
        let mut rng = Lcg(seed);
        let count = 6 + rng.below(20);
        let mut ops: Vec<OpIR> = (0..count).map(|_| generated_op(&mut rng)).collect();
        for label in 1..=3 {
            let at = rng.below(ops.len() as u64 + 1) as usize;
            let mut target = op("label");
            target.value = Some(label);
            ops.insert(at, target);
        }
        ops.push(op("ret_void"));
        ops
    }

    /// Path semantics over the operation-level successor relation: a name is
    /// live after an operation when some successor path reads it before any
    /// redefinition. Independent of the block dataflow solver.
    fn exhaustive_live_after(ops: &[OpIR]) -> Vec<BTreeSet<String>> {
        let cfg = CFG::build(ops);
        let block_successors = liveness_block_successors(&cfg);
        let mut successors = vec![Vec::new(); ops.len()];
        for block in &cfg.blocks {
            if block.start_op == block.end_op {
                continue; // Invocation has no source operation.
            }
            let tail = block.end_op - 1;
            for op_idx in block.start_op..tail {
                successors[op_idx].push(op_idx + 1);
            }
            for &successor in &block_successors[block.id] {
                successors[tail].push(cfg.blocks[successor].start_op);
            }
        }
        let mut names = BTreeSet::new();
        for op in ops {
            visit_simple_ir_reads(op, |read| {
                names.insert(read.name.to_string());
            });
            visit_simple_ir_defined_names(op, |name| {
                names.insert(name.to_string());
            });
        }
        (0..ops.len())
            .map(|op_idx| {
                names
                    .iter()
                    .filter(|name| reads_before_redefinition(ops, &successors, op_idx, name))
                    .cloned()
                    .collect()
            })
            .collect()
    }

    fn reads_before_redefinition(
        ops: &[OpIR],
        successors: &[Vec<usize>],
        from: usize,
        name: &str,
    ) -> bool {
        let mut pending = successors[from].clone();
        let mut visited = BTreeSet::new();
        while let Some(op_idx) = pending.pop() {
            if !visited.insert(op_idx) {
                continue;
            }
            let mut reads = false;
            visit_simple_ir_reads(&ops[op_idx], |read| reads |= read.name == name);
            if reads {
                return true;
            }
            let mut defines = false;
            visit_simple_ir_defined_names(&ops[op_idx], |defined| defines |= defined == name);
            if !defines {
                pending.extend(&successors[op_idx]);
            }
        }
        false
    }

    fn live_before(op: &OpIR, live_after: &BTreeSet<String>) -> BTreeSet<String> {
        let mut live = live_after.clone();
        visit_simple_ir_defined_names(op, |name| {
            live.remove(name);
        });
        visit_simple_ir_reads(op, |read| {
            live.insert(read.name.to_string());
        });
        live
    }
}
