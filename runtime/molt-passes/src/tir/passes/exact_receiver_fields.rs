//! Field access on proven exact receivers (design 78, 3.8 and 5.8).
//!
//! The frontend lowers an attribute access by name (`get_attr_generic_*`,
//! `set_attr_generic_*`) or behind a layout guard (`guarded_field_get/set`)
//! whenever it cannot prove the receiver's class at lowering time. The TIR
//! exact solver proves more: an allocation's result, and a parameter the type
//! flow cloned for an exact class (`ArgKind::Class`), hold an instance of
//! exactly one class statement. Each class statement's stamp
//! (`class_set_definition`) carries the attributes an exact instance reads and
//! writes at fixed offsets (`exact_field_slots`, the frontend's exact-receiver
//! rule: layout stable, no data descriptor, no attribute hook), so such an
//! access is the field load or store at that offset (`Load`/`Store`), the form
//! an exact `self` already gets. One table per program, one rewrite.

use molt_ir::class_identity::{EXACT_FIELD_SLOTS_ATTR, decode_field_slots};
use molt_ir::deterministic_hash::HashMap;
use molt_ir::{ClassId, OpKind};

use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::types::TirType;

/// Each class statement's exact field slots, from its definition stamps.
pub type ClassFieldSlots = HashMap<ClassId, HashMap<String, i64>>;

/// The program's table: every `class_set_definition` that carries slots.
pub fn class_field_slots(module: &TirModule) -> ClassFieldSlots {
    let mut table = ClassFieldSlots::default();
    for func in &module.functions {
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.origin != Some(OpKind::ClassSetDefinition) {
                    continue;
                }
                let (Some(AttrValue::Str(definition)), Some(AttrValue::Bytes(bytes))) = (
                    op.attrs.get("class_definition"),
                    op.attrs.get(EXACT_FIELD_SLOTS_ATTR),
                ) else {
                    continue;
                };
                let slots = decode_field_slots(bytes)
                    .expect("exact field slots decode from their own encoding");
                table.insert(
                    ClassId::of_definition(definition),
                    slots.into_iter().collect(),
                );
            }
        }
    }
    table
}

/// The fixed-offset operation and whether it discards the layout operand.
fn field_rewrite(op: &TirOp) -> Option<(OpKind, bool)> {
    match (op.origin?, op.operands.len()) {
        (OpKind::GetAttrGenericObj | OpKind::GetAttrGenericPtr, 1) => Some((OpKind::Load, false)),
        (OpKind::GuardedFieldGet, 2) => Some((OpKind::Load, true)),
        (OpKind::SetAttrGenericObj | OpKind::SetAttrGenericPtr, 2) => Some((OpKind::Store, false)),
        (OpKind::GuardedFieldSet, 3) => Some((OpKind::Store, true)),
        _ => None,
    }
}

/// Rewrite `func`'s attribute accesses on proven exact receivers; return how
/// many it rewrote.
pub fn run(func: &mut TirFunction, table: &ClassFieldSlots) -> usize {
    if table.is_empty()
        || !func
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|op| field_rewrite(op).is_some())
    {
        return 0;
    }
    let exact = crate::tir::type_refine::extract_exact_class_map(func);
    if exact.is_empty() {
        return 0;
    }
    let mut rewritten = 0;
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            let Some((field_op, discard_layout)) = field_rewrite(op) else {
                continue;
            };
            let Some(TirType::Class(class)) = exact.get(&op.operands[0]) else {
                continue;
            };
            let Some(AttrValue::Str(name)) = op.attrs.get("name") else {
                continue;
            };
            let Some(&offset) = table.get(class).and_then(|slots| slots.get(name)) else {
                continue;
            };
            if discard_layout {
                op.operands.remove(1);
            }
            op.origin = Some(field_op);
            op.attrs.insert("value".into(), AttrValue::Int(offset));
            op.attrs.remove("class_definition");
            rewritten += 1;
        }
    }
    rewritten
}
