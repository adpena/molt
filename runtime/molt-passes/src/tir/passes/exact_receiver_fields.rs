//! Member access on proven exact receivers (design 78, 3.8 and 5.8).
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
//! an exact `self` already gets. Ordinary method definitions publish the
//! name their exact receiver selects on the function creation that already
//! carries the typed callee. A matching method load/call pair keeps its two
//! owned results and enters that callee directly.

use molt_ir::class_identity::{EXACT_FIELD_SLOTS_ATTR, ExactMethod, decode_field_slots};
use molt_ir::deterministic_hash::HashMap;
use molt_ir::{ClassId, FunctionId, OpKind, ParameterCustody};

use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;

/// The members whose lookup on an exact instance has a closed source proof.
#[derive(Default)]
pub struct ExactMembers {
    fields: HashMap<ClassId, HashMap<String, i64>>,
    methods: HashMap<ClassId, HashMap<String, Option<MethodEntry>>>,
}

#[derive(Clone, Copy)]
struct MethodEntry {
    callee: FunctionId,
    arity: usize,
}

fn declared_entry(op: &TirOp, module: &TirModule, bodies: &[Option<usize>]) -> Option<MethodEntry> {
    let callee = op.callee?;
    let body = &module.functions[bodies.get(callee.index()).copied().flatten()?];
    let AttrValue::Int(arity) = op.attrs.get("value")? else {
        return None;
    };
    let arity = usize::try_from(*arity).ok()?;
    let closure = usize::from(body.has_closure_parameter());
    if body.has_state_machine()
        || body.return_abi != molt_ir::FunctionReturnAbi::Value
        || body.execution_context == molt_ir::ExecutionContextPolicy::Inherited
        || (closure != 0) != (op.origin == Some(OpKind::FuncNewClosure))
        || arity.checked_add(closure)? != body.param_types.len()
        || !(closure..body.param_types.len())
            .all(|i| body.parameter_custody(i) == ParameterCustody::Transferred)
    {
        return None;
    }
    Some(MethodEntry { callee, arity })
}

pub fn exact_members(module: &TirModule, bodies: &[Option<usize>]) -> ExactMembers {
    let mut table = ExactMembers::default();
    let mut stamps = HashMap::default();
    for op in module
        .functions
        .iter()
        .flat_map(|f| f.blocks.values())
        .flat_map(|b| &b.ops)
    {
        if op.origin == Some(OpKind::ClassSetDefinition)
            && let Some(AttrValue::Str(definition)) = op.attrs.get("class_definition")
        {
            let class = ClassId::of_definition(definition);
            stamps
                .entry(class)
                .and_modify(|unique| *unique = false)
                .or_insert(true);
            if let Some(AttrValue::Bytes(bytes)) = op.attrs.get(EXACT_FIELD_SLOTS_ATTR) {
                let slots = decode_field_slots(bytes)
                    .expect("exact field slots decode from their own encoding");
                table.fields.insert(class, slots.into_iter().collect());
            }
        } else if let Ok(Some(ExactMethod(definition, name))) = ExactMethod::from_tir(op) {
            let class = ClassId::of_definition(&definition);
            // Multiple publications for the same binding make it ambiguous,
            // including a declaration whose physical entry we cannot use.
            table
                .methods
                .entry(class)
                .or_default()
                .entry(name)
                .and_modify(|entry| *entry = None)
                .or_insert_with(|| declared_entry(op, module, bodies));
        }
    }
    table
        .methods
        .retain(|class, _| stamps.get(class) == Some(&true));
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

/// Rewrite field accesses and complete method load/call pairs on exact receivers.
pub fn run(func: &mut TirFunction, table: &ExactMembers) -> usize {
    if !func.blocks.values().flat_map(|b| &b.ops).any(|op| {
        (!table.fields.is_empty() && field_rewrite(op).is_some())
            || (!table.methods.is_empty() && op.opcode == OpCode::LoadMethod)
    }) {
        return 0;
    }
    let exact = crate::tir::type_refine::extract_exact_class_map(func);
    if exact.is_empty() {
        return 0;
    }
    let mut rewritten = 0;
    let mut pairs = HashMap::default();
    if !table.methods.is_empty() {
        for op in func.blocks.values_mut().flat_map(|b| &mut b.ops) {
            if op.opcode == OpCode::LoadMethod
                && let [receiver] = op.operands.as_slice()
                && let [callable, bound_self] = op.results.as_slice()
                && let Some(TirType::Class(class)) = exact.get(receiver)
                && let Some(AttrValue::Str(name)) = op.attrs.get("name")
                && let Some(Some(entry)) = table.methods.get(class).and_then(|m| m.get(name))
            {
                pairs.insert((*callable, *bound_self), *entry);
                if !op.proven_target() {
                    op.set_proven_target(true);
                    rewritten += 1;
                }
            }
        }
    }
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            if op.opcode == OpCode::CallLoadedMethod
                && op.results.len() == 1
                && let [callable, bound_self, args @ ..] = op.operands.as_slice()
                && let Some(entry) = pairs.get(&(*callable, *bound_self))
                && entry.arity == args.len() + 1
                && (0..op.operands.len())
                    .all(|i| op.operand_custody(i) == ParameterCustody::Transferred)
            {
                // Keep the original load and both of its owned results. Python
                // captures them before any argument callback. The direct entry
                // uses that live function object's globals and closure.
                op.opcode = OpCode::Call;
                op.origin = Some(OpKind::CallGuarded);
                op.callee = Some(entry.callee);
                op.set_method_site(None);
                op.set_proven_target(true);
                rewritten += 1;
                continue;
            }
            let Some((field_op, discard_layout)) = field_rewrite(op) else {
                continue;
            };
            let Some(TirType::Class(class)) = exact.get(&op.operands[0]) else {
                continue;
            };
            let Some(AttrValue::Str(name)) = op.attrs.get("name") else {
                continue;
            };
            let Some(&offset) = table.fields.get(class).and_then(|slots| slots.get(name)) else {
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
