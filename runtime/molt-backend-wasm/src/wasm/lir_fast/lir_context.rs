use crate::wasm::body::{WasmBodyOps, WasmLirFallbackReason};
use crate::wasm::const_materialization::WasmConstMaterialization;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::blocks::BlockId;
use molt_tir::tir::lir::{LirFunction, LirRepr, LirTerminator, LirValue};
use molt_tir::tir::ops::{AttrValue, TirOp};
use molt_tir::tir::types::TirType;
use molt_tir::tir::values::ValueId;
use std::collections::{HashMap, HashSet};
use wasm_encoder::{Instruction, ValType};

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
    pub(super) value_types: HashMap<ValueId, TirType>,
    flat_list_int_values: HashSet<ValueId>,
    /// Reverse map: local index -> ValType. Built during allocation so the
    /// locals vector can be constructed in O(N) instead of O(N^2).
    pub(super) local_types: HashMap<u32, ValType>,
    pub(super) next_local: u32,
    pub(super) instructions: WasmBodyOps,
    pub(super) rpo: Vec<BlockId>,
    pub(super) block_index: HashMap<BlockId, usize>,
}

impl<'a> LirLowerCtx<'a> {
    pub(super) fn new_with_local_base(func: &'a LirFunction, local_base: u32) -> Self {
        let rpo = compute_lir_rpo(func);
        let block_index = rpo.iter().enumerate().map(|(i, &bid)| (bid, i)).collect();
        let flat_list_int_values = compute_lir_flat_list_int_values(func);
        Self {
            func,
            value_locals: HashMap::new(),
            value_reprs: HashMap::new(),
            value_types: HashMap::new(),
            flat_list_int_values,
            local_types: HashMap::new(),
            next_local: local_base,
            instructions: WasmBodyOps::default(),
            rpo,
            block_index,
        }
    }

    /// Emit a typed runtime-import call. This is how the LIR fast lane reaches
    /// runtime helpers (e.g. `int_from_i64` for the overflow-safe box) without
    /// bailing the whole function to the generic path.
    pub(super) fn emit_runtime_call(&mut self, call: LirRuntimeCall) {
        self.instructions.push_runtime_import_call(call);
    }

    pub(super) fn emit_bail_to_generic_path(&mut self, reason: WasmLirFallbackReason) {
        self.instructions.push_bail_to_generic_path(reason);
    }

    pub(super) fn emit_const_materialization(&mut self, materialization: WasmConstMaterialization) {
        self.instructions
            .push_const_materialization(materialization);
    }

    pub(super) fn local_for(&mut self, value: &LirValue) -> u32 {
        if let Some(&idx) = self.value_locals.get(&value.id) {
            return idx;
        }
        let idx = self.next_local;
        self.next_local += 1;
        self.value_locals.insert(value.id, idx);
        self.value_reprs.insert(value.id, value.repr);
        self.value_types.insert(value.id, value.ty.clone());
        self.local_types.insert(idx, lir_repr_to_val(value.repr));
        idx
    }

    pub(super) fn allocate_function_locals(&mut self) {
        if let Some(entry) = self.func.blocks.get(&self.func.entry_block) {
            for arg in &entry.args {
                self.local_for(arg);
            }
        }
        for &bid in &self.rpo.clone() {
            if let Some(block) = self.func.blocks.get(&bid) {
                for arg in &block.args {
                    self.local_for(arg);
                }
                for op in &block.ops {
                    for value in &op.result_values {
                        self.local_for(value);
                    }
                }
            }
        }
    }

    pub(super) fn local_declarations_after(&self, first_local: u32) -> Vec<ValType> {
        let mut locals = Vec::with_capacity(self.next_local.saturating_sub(first_local) as usize);
        for idx in first_local..self.next_local {
            locals.push(self.local_types.get(&idx).copied().unwrap_or(ValType::I64));
        }
        locals
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

    pub(super) fn alloc_scratch_local(&mut self, val_type: ValType) -> u32 {
        let idx = self.next_local;
        self.next_local += 1;
        self.local_types.insert(idx, val_type);
        idx
    }

    pub(super) fn repr_of(&self, vid: ValueId) -> LirRepr {
        self.value_reprs
            .get(&vid)
            .copied()
            .unwrap_or(LirRepr::DynBox)
    }

    pub(super) fn type_of(&self, vid: ValueId) -> Option<&TirType> {
        self.value_types.get(&vid)
    }

    pub(super) fn has_flat_list_int_storage(&self, vid: ValueId) -> bool {
        self.flat_list_int_values.contains(&vid)
    }
}

fn compute_lir_flat_list_int_values(func: &LirFunction) -> HashSet<ValueId> {
    let mut facts = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|block_id| block_id.0);
        for block_id in block_ids {
            let Some(block) = func.blocks.get(&block_id) else {
                continue;
            };
            for op in &block.ops {
                let tir_op = &op.tir_op;
                if tir_op_original_kind(tir_op) == Some("list_int_new") {
                    for &result in &tir_op.results {
                        changed |= facts.insert(result);
                    }
                    continue;
                }
                if tir_op.is_plain_value_copy()
                    && let (Some(&source), Some(&result)) =
                        (tir_op.operands.first(), tir_op.results.first())
                    && facts.contains(&source)
                {
                    changed |= facts.insert(result);
                }
            }
        }
    }
    facts
}

fn tir_op_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
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
