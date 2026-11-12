//! Compiled module global slot sites (call_anatomy.md 4.5, invariant J3).
//!
//! Each read or store that carries its `global_slot` (stamped at integration
//! from the module registry layout) becomes a slot site:
//!
//! ```text
//!   %v = ModuleGetGlobal %module, %name          {slot M:k}
//! becomes
//!   %m = ModuleSlot[M] ; %b = GlobalSlotBase %m
//!   %v = GlobalSlotGet %b, %module, %name        {slot M:k}
//!
//!   ModuleSetAttr %module, %name, %value         {slot M:k}
//!   Copy[dict_set] %globals, %name, %value       {slot M:k}   ; STORE_GLOBAL in a function
//! become
//!   %m = ModuleSlot[M] ; %k = ConstInt k
//!   Call[molt_global_slot_set] %m, %k, %module|None, %name, %value
//! ```
//!
//! `GlobalSlotBase` is pure within an activation, so the cleanup round after
//! this pass merges every base of one module into one per function and
//! hoists it out of loops. A read's meaning (slot load, unbound test, retain,
//! by-name fallback) is the shared generator in `molt_codegen_abi::global_slot`;
//! a store's is `molt_global_slot_set`, which replaces the dictionary row by
//! index and falls back to the site's by-name store.

use std::collections::HashSet;

use super::PassStats;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Rewrite every stamped site of `func`. Requires: the module phase is done
/// (promotion has already read these ops by their slot) and drops are not
/// inserted.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "global_slot_sites",
        ..Default::default()
    };
    let used: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| {
            block
                .ops
                .iter()
                .flat_map(|op| op.operands.iter().copied())
                .chain(terminator_values(&block.terminator))
        })
        .collect();
    // `blocks` is a HashMap: visit in id order so fresh value ids, and the
    // emitted code, are the same in every process.
    let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    for block_id in block_ids {
        let ops = std::mem::take(&mut func.blocks.get_mut(&block_id).unwrap().ops);
        let mut rewritten = Vec::with_capacity(ops.len());
        for op in ops {
            let Some(slot) = op.global_slot() else {
                rewritten.push(op);
                continue;
            };
            let site = if op.opcode == OpCode::ModuleGetGlobal {
                Site::Read
            } else if is_store(&op) && op.results.iter().all(|result| !used.contains(result)) {
                Site::Store
            } else {
                // A slot on a store whose result is still read keeps the
                // by-name op: the runtime entry returns None.
                Site::ByName
            };
            if site == Site::ByName {
                let mut unchanged = op;
                unchanged.set_global_slot(None);
                rewritten.push(unchanged);
                continue;
            }
            let span = op.source_span;
            let module_id = fresh(func, TirType::I64);
            let mut module_slot = new_op(OpCode::ModuleSlot, vec![], vec![module_id], span);
            module_slot
                .attrs
                .insert("s_value".into(), AttrValue::Str(slot.module.clone()));
            rewritten.push(module_slot);
            if site == Site::Read {
                let base = fresh(func, TirType::I64);
                rewritten.push(new_op(
                    OpCode::GlobalSlotBase,
                    vec![module_id],
                    vec![base],
                    span,
                ));
                let mut get = op;
                get.opcode = OpCode::GlobalSlotGet;
                get.origin = None;
                get.operands.insert(0, base);
                rewritten.push(get);
            } else {
                let index = fresh(func, TirType::I64);
                let mut index_op = new_op(OpCode::ConstInt, vec![], vec![index], span);
                index_op
                    .attrs
                    .insert("value".into(), AttrValue::Int(i64::from(slot.index)));
                rewritten.push(index_op);
                // [module, name, value] or, for STORE_GLOBAL in a function,
                // [globals(), name, value]: its by-name fallback stores into
                // the frame's globals (None).
                let module = if op.opcode == OpCode::ModuleSetAttr {
                    op.operands[0]
                } else {
                    let none = fresh(func, TirType::None);
                    rewritten.push(new_op(OpCode::ConstNone, vec![], vec![none], span));
                    none
                };
                let mut store = op;
                store.set_global_slot(None);
                store.opcode = OpCode::Call;
                store.origin = None;
                store.runtime = Some(molt_ir::runtime_fn_generated::RuntimeFn::GlobalSlotSet);
                store.attrs.insert(
                    "s_value".into(),
                    AttrValue::Str(
                        molt_ir::runtime_fn_generated::RuntimeFn::GlobalSlotSet
                            .symbol()
                            .to_string(),
                    ),
                );
                store.operands = vec![
                    module_id,
                    index,
                    module,
                    store.operands[1],
                    store.operands[2],
                ];
                if store.results.is_empty() {
                    store.results.push(fresh(func, TirType::DynBox));
                }
                rewritten.push(store);
            }
            stats.ops_added += 2;
            stats.values_changed += 1;
        }
        func.blocks.get_mut(&block_id).unwrap().ops = rewritten;
    }
    stats
}

/// What one stamped op becomes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Site {
    Read,
    Store,
    ByName,
}

fn terminator_values(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Branch { args, .. } => args.clone(),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => std::iter::once(*cond)
            .chain(then_args.iter().copied())
            .chain(else_args.iter().copied())
            .collect(),
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => std::iter::once(*value)
            .chain(cases.iter().flat_map(|(_, _, args)| args.iter().copied()))
            .chain(default_args.iter().copied())
            .collect(),
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => cases
            .iter()
            .flat_map(|(_, _, args)| args.iter().copied())
            .chain(default_args.iter().copied())
            .collect(),
        Terminator::Return { values } => values.clone(),
        Terminator::Unreachable => Vec::new(),
    }
}

fn is_store(op: &TirOp) -> bool {
    let module_store = op.opcode == OpCode::ModuleSetAttr;
    let frame_store = op.opcode == OpCode::Copy && op.origin == Some(molt_ir::OpKind::DictSet);
    (module_store || frame_store) && op.operands.len() == 3
}

fn new_op(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    span: Option<(u32, u32)>,
) -> TirOp {
    TirOp {
        source_span: span,
        ..TirOp::new(opcode, operands, results)
    }
}

fn fresh(func: &mut TirFunction, ty: TirType) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, ty);
    value
}

#[cfg(test)]
#[path = "global_slot_sites_tests.rs"]
mod tests;
