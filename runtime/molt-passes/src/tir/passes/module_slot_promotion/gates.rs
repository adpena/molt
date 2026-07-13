//! Soundness / eligibility predicates for module-slot promotion.
//!
//! Each predicate is a conservative gate: a `true` from a hazard check (or a
//! `None`/refusal from a discovery helper) leaves the loop with its
//! per-iteration dict traffic intact. See the module doc in [`super`] for the
//! full soundness contract these gates enforce.

use std::collections::HashMap;

use crate::tir::function::{TirFunction, TirModule};
use crate::tir::op_kinds_generated::{
    ModuleConcurrencyMarkerSourceRole, ModuleSlotAccessRole, opcode_is_state_machine_table,
    opcode_module_concurrency_marker_source_facts_table, opcode_module_slot_access_role_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::super::alias_analysis::AliasAnalysisResult;

pub(super) fn module_has_concurrency_markers(module: &TirModule) -> bool {
    for func in &module.functions {
        for block in func.blocks.values() {
            for op in &block.ops {
                let facts = opcode_module_concurrency_marker_source_facts_table(op.opcode);
                match facts.role {
                    ModuleConcurrencyMarkerSourceRole::ModuleName => {
                        for &key in facts.attrs {
                            if let Some(AttrValue::Str(s)) = op.attrs.get(key)
                                && (s == "threading" || s == "_thread")
                            {
                                return true;
                            }
                        }
                    }
                    // A direct intrinsic CALL to the thread machinery (no
                    // module import needed) — the callee symbol, not an
                    // argument string.
                    ModuleConcurrencyMarkerSourceRole::ThreadIntrinsicCallee => {
                        for &key in facts.attrs {
                            if let Some(AttrValue::Str(s)) = op.attrs.get(key)
                                && s.starts_with("molt_thread")
                            {
                                return true;
                            }
                        }
                    }
                    ModuleConcurrencyMarkerSourceRole::None => {}
                }
            }
        }
    }
    false
}

/// A `Copy` passthrough of a structural / debug-marker SimpleIR kind (`line`
/// numbers, `nop`, labels…) — position metadata with no memory semantics.
/// `is_plain_value_copy` deliberately rejects passthroughs (they are not value
/// copies), but they are not barriers either.
pub(super) fn is_marker_passthrough(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(k)) => k == "line" || crate::tir::is_structural(k),
        _ => false,
    }
}

/// Resolve `func`'s single module-object root: every `ModuleGetAttr` /
/// `ModuleSetAttr` first operand must canonicalize (through transparent
/// copies, via the alias oracle) to ONE root value. Returns `None` (skip
/// function) when there are no module ops, multiple roots, or a root that is
/// not function-entry-stable (we require it to be an entry-block argument so
/// the object identity provably never changes mid-function).
pub(super) fn single_module_root(
    func: &TirFunction,
    alias: &AliasAnalysisResult,
) -> Option<ValueId> {
    let mut root: Option<ValueId> = None;
    for block in func.blocks.values() {
        for op in &block.ops {
            if opcode_module_slot_access_role_table(op.opcode) == ModuleSlotAccessRole::KeyedAttr {
                let m = alias.root(*op.operands.first()?);
                match root {
                    None => root = Some(m),
                    Some(r) if r == m => {}
                    Some(_) => return None,
                }
            }
        }
    }
    let root = root?;
    let entry = &func.blocks[&func.entry_block];
    entry.args.iter().any(|a| a.id == root).then_some(root)
}

/// Map every `ConstStr` result in `func` to its string value (module-attr
/// names are `ConstStr` operands).
pub(super) fn const_str_defs(func: &TirFunction) -> HashMap<ValueId, String> {
    let mut map = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstStr
                && let (Some(&r), Some(AttrValue::Str(s))) =
                    (op.results.first(), op.attrs.get("s_value"))
            {
                map.insert(r, s.clone());
            }
        }
    }
    map
}

/// The dynamic / wildcard module ops that can touch ANY module-ATTR slot. A
/// function containing one is skipped wholesale (conservative).
///
/// Deliberately NOT wildcards: `ModuleCacheGet`/`ModuleCacheSet`/
/// `ModuleCacheDel` operate on the global module CACHE (`sys.modules`
/// registration — every module chunk registers/unregisters itself), and
/// `ModuleGetName` reads the module's name field — neither touches the
/// module's ATTR dict, so they cannot alias promoted slots. (Inside a loop
/// they would still refuse promotion through the coarse `ModuleDict` region
/// barrier — conservative — but their routine presence at chunk entry/exit
/// must not disqualify the whole function: that inertness is exactly what the
/// refusal-reason instrument caught on `bench_sum__molt_module_chunk_1`.)
pub(super) fn is_wildcard_module_op(op: &TirOp, names: &HashMap<ValueId, String>) -> bool {
    match opcode_module_slot_access_role_table(op.opcode) {
        ModuleSlotAccessRole::KeyedAttr => {
            // Const-named accesses are precise; a non-const name is wildcard.
            op.operands.get(1).is_none_or(|n| !names.contains_key(n))
        }
        ModuleSlotAccessRole::WildcardModuleDict => true,
        ModuleSlotAccessRole::None => false,
    }
}

/// Generator/async state-machine opcodes — functions containing one are
/// skipped (mirrors the inliner's exclusion).
pub(super) fn is_state_machine_op(opcode: OpCode) -> bool {
    opcode_is_state_machine_table(opcode)
}
