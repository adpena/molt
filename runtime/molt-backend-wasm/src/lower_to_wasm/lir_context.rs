use super::*;
use molt_tir::tir::lir::LirValue;

pub(super) fn lir_repr_to_val(repr: LirRepr) -> ValType {
    match repr {
        LirRepr::I64 => ValType::I64,
        LirRepr::F64 => ValType::F64,
        LirRepr::Bool1 => ValType::I32,
        LirRepr::DynBox | LirRepr::Ref64 => ValType::I64,
    }
}

pub(super) struct LirLowerCtx<'a> {
    pub(super) func: &'a LirFunction,
    pub(super) value_locals: HashMap<ValueId, u32>,
    pub(super) value_reprs: HashMap<ValueId, LirRepr>,
    /// Reverse map: local index -> ValType. Built during allocation so the
    /// locals vector can be constructed in O(N) instead of O(N^2).
    pub(super) local_types: HashMap<u32, ValType>,
    pub(super) next_local: u32,
    pub(super) instructions: Vec<Instruction<'static>>,
    /// Named runtime calls in emission order (see
    /// [`WasmFunctionOutput::runtime_calls`]).
    pub(super) runtime_calls: Vec<&'static str>,
    pub(super) rpo: Vec<BlockId>,
    pub(super) block_index: HashMap<BlockId, usize>,
}

impl<'a> LirLowerCtx<'a> {
    pub(super) fn new(func: &'a LirFunction) -> Self {
        Self::new_with_local_base(func, 0)
    }

    pub(super) fn new_with_local_base(func: &'a LirFunction, local_base: u32) -> Self {
        let rpo = compute_lir_rpo(func);
        let block_index = rpo.iter().enumerate().map(|(i, &bid)| (bid, i)).collect();
        Self {
            func,
            value_locals: HashMap::new(),
            value_reprs: HashMap::new(),
            local_types: HashMap::new(),
            next_local: local_base,
            instructions: Vec::new(),
            runtime_calls: Vec::new(),
            rpo,
            block_index,
        }
    }

    /// Emit a NAMED runtime-import call: a placeholder `Call` paired
    /// positionally with `name` in `runtime_calls`, resolved to the real
    /// import index by the module assembler. This is how the LIR fast lane
    /// reaches runtime helpers (e.g. `int_from_i64` for the overflow-safe
    /// box) without bailing the whole function the way `Call(0)` does.
    pub(super) fn emit_runtime_call(&mut self, name: &'static str) {
        self.instructions
            .push(Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER));
        self.runtime_calls.push(name);
    }

    pub(super) fn local_for(&mut self, value: &LirValue) -> u32 {
        if let Some(&idx) = self.value_locals.get(&value.id) {
            return idx;
        }
        let idx = self.next_local;
        self.next_local += 1;
        self.value_locals.insert(value.id, idx);
        self.value_reprs.insert(value.id, value.repr);
        self.local_types.insert(idx, lir_repr_to_val(value.repr));
        idx
    }

    pub(super) fn get_local(&self, vid: ValueId) -> u32 {
        self.value_locals[&vid]
    }

    pub(super) fn emit_get(&mut self, vid: ValueId) {
        self.instructions
            .push(Instruction::LocalGet(self.get_local(vid)));
    }

    pub(super) fn emit_set(&mut self, vid: ValueId) {
        self.instructions
            .push(Instruction::LocalSet(self.get_local(vid)));
    }

    pub(super) fn repr_of(&self, vid: ValueId) -> LirRepr {
        self.value_reprs
            .get(&vid)
            .copied()
            .unwrap_or(LirRepr::DynBox)
    }
}

fn compute_lir_rpo(func: &LirFunction) -> Vec<BlockId> {
    let mut visited = HashMap::new();
    let mut order = Vec::new();
    rpo_visit_lir(func, func.entry_block, &mut visited, &mut order);
    order.reverse();
    order
}

fn rpo_visit_lir(
    func: &LirFunction,
    block_id: BlockId,
    visited: &mut HashMap<BlockId, bool>,
    order: &mut Vec<BlockId>,
) {
    if visited.contains_key(&block_id) {
        return;
    }
    visited.insert(block_id, true);
    if let Some(block) = func.blocks.get(&block_id) {
        for succ in lir_terminator_successors(&block.terminator) {
            rpo_visit_lir(func, succ, visited, order);
        }
    }
    order.push(block_id);
}

pub(super) fn lir_terminator_successors(term: &LirTerminator) -> Vec<BlockId> {
    match term {
        LirTerminator::Branch { target, .. } => vec![*target],
        LirTerminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        LirTerminator::Switch { cases, default, .. }
        | LirTerminator::StateDispatch { cases, default, .. } => {
            let mut succs: Vec<BlockId> = cases.iter().map(|(_, bid, _)| *bid).collect();
            succs.push(*default);
            succs
        }
        LirTerminator::Return { .. } | LirTerminator::Unreachable => vec![],
    }
}
