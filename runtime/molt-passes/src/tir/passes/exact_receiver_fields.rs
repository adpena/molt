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

mod late_cells;
use crate::tir::blocks::BlockId;
use crate::tir::values::ValueId;
pub(super) use late_cells::CellRequests;
use molt_ir::class_identity::{EXACT_FIELD_SLOTS_ATTR, ExactMethod, decode_field_slots};
use molt_ir::deterministic_hash::HashMap;
use molt_ir::ir::GlobalSlotRef;
use molt_ir::tir::dominators::ProgramPointDominance;
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Maker {
    module: String,
    body: usize,
    block: BlockId,
    value: ValueId,
}

impl ExactMembers {
    pub(super) fn late_modules(&self) -> impl Iterator<Item = &str> {
        self.methods
            .values()
            .flat_map(|methods| methods.values().flatten())
            .filter_map(|entry| entry.maker.as_ref().map(|maker| maker.module.as_str()))
    }
}

struct MethodEntry {
    callee: FunctionId,
    arity: usize,
    cell: Option<GlobalSlotRef>,
    maker: Option<Maker>,
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
    Some(MethodEntry {
        callee,
        arity,
        cell: None,
        maker: None,
    })
}

pub fn exact_members(module: &TirModule, bodies: &[Option<usize>]) -> ExactMembers {
    let mut table = ExactMembers::default();
    let mut stamps = HashMap::default();
    let mut cell_stores = HashMap::default();
    for (body_index, function) in module.functions.iter().enumerate() {
        let mut publications = HashMap::default();
        let mut class_points = HashMap::default();
        let mut methods = Vec::new();
        for (&block, body) in &function.blocks {
            for (index, op) in body.ops.iter().enumerate() {
                if op.origin == Some(OpKind::ClassSetDefinition)
                    && let Some(AttrValue::Str(definition)) = op.attrs.get("class_definition")
                {
                    let class = ClassId::of_definition(definition);
                    stamps
                        .entry(class)
                        .and_modify(|unique| *unique = false)
                        .or_insert(true);
                    class_points.insert(class, (block, index));
                    if let Some(AttrValue::Bytes(bytes)) = op.attrs.get(EXACT_FIELD_SLOTS_ATTR) {
                        let slots = decode_field_slots(bytes)
                            .expect("exact field slots decode from their own encoding");
                        table.fields.insert(class, slots.into_iter().collect());
                    }
                } else if (op.opcode == OpCode::StaticCellSet
                    || matches!(op.origin, Some(OpKind::ModuleSetAttr | OpKind::DictSet)))
                    && let Some(slot) = op.global_slot()
                    && slot.cell
                {
                    *cell_stores
                        .entry((slot.module.clone(), slot.index))
                        .or_insert(0usize) += 1;
                    if op.opcode == OpCode::StaticCellSet
                        && slot.init_once
                        && let [_, value] = op.operands[..]
                    {
                        publications.insert(value, (block, index, slot));
                    }
                } else if let Ok(Some(ExactMethod(definition, name, owner))) =
                    ExactMethod::from_tir(op)
                {
                    methods.push((
                        op,
                        ClassId::of_definition(&definition),
                        name,
                        owner,
                        block,
                        index,
                    ));
                }
            }
        }
        let mut dominance = None;
        for (op, class, name, owner, block, index) in methods {
            // The existing function cell holds this very creation's object.
            // Its publication must precede the class stamp on every path: a
            // receiver bearing that class then witnesses an initialized cell.
            let mut entry = declared_entry(op, module, bodies);
            if let Some(entry) = entry.as_mut()
                && let Some((store_block, store, slot)) = publications.get(&op.results[0])
                && let Some(&(stamp_block, stamp)) = class_points.get(&class)
                && dominance
                    .get_or_insert_with(|| ProgramPointDominance::compute(function))
                    .definition_available(*store_block, Some(*store), stamp_block, stamp)
            {
                entry.cell = Some(slot.clone());
            }
            if let Some(entry) = entry.as_mut()
                && entry.cell.is_none()
                && let Some(module) = owner
                // The maker can fail. Only the continuation of its own
                // exception check has an object to publish.
                && let Some(check) = function.blocks[&block].ops.get(index + 1)
                && super::exception_observation::check_label(check).is_some()
                && !check.is_async_work_poll()
                && let Some(&(stamp_block, stamp)) = class_points.get(&class)
                && dominance
                    .get_or_insert_with(|| ProgramPointDominance::compute(function))
                    .definition_available(block, Some(index + 1), stamp_block, stamp)
            {
                entry.maker = Some(Maker {
                    module,
                    body: body_index,
                    block,
                    value: op.results[0],
                });
            }
            table
                .methods
                .entry(class)
                .or_default()
                .entry(name)
                .and_modify(|previous| *previous = None)
                .or_insert(entry);
        }
    }
    table.methods.retain(|class, methods| {
        for entry in methods.values_mut().flatten() {
            if entry
                .cell
                .as_ref()
                .is_some_and(|slot| cell_stores.get(&(slot.module.clone(), slot.index)) != Some(&1))
            {
                entry.cell = None;
            }
        }
        stamps.get(class) == Some(&true)
    });
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
    run_with_cells(func, table, &mut CellRequests::default())
}

pub(super) fn run_with_cells(
    func: &mut TirFunction,
    table: &ExactMembers,
    cells: &mut CellRequests,
) -> usize {
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
                pairs.insert((*callable, *bound_self), entry);
                if !op.proven_target() {
                    op.set_proven_target(true);
                    rewritten += 1;
                }
            }
        }
    }
    let mut cell_reads = HashMap::default();
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
                // Keep the captured operands. The direct entry uses the live
                // function object's globals and closure.
                if entry.cell.is_some() || entry.maker.is_some() {
                    cell_reads.insert((*callable, *bound_self), *entry);
                }
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
    rewritten + replace_cell_loads(func, &cell_reads, cells)
}

fn replace_cell_loads(
    func: &mut TirFunction,
    reads: &HashMap<(ValueId, ValueId), &MethodEntry>,
    cells: &mut CellRequests,
) -> usize {
    if reads.is_empty() {
        return 0;
    }
    let uses = super::dce::build_use_counts(func);
    let mut count = 0;
    let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for block in blocks {
        let ops = std::mem::take(&mut func.blocks.get_mut(&block).unwrap().ops);
        let mut result = Vec::with_capacity(ops.len());
        for op in ops {
            if op.opcode == OpCode::LoadMethod
                && let [callable, bound_self] = op.results[..]
                && let Some(entry) = reads.get(&(callable, bound_self))
                // Retain the pair protocol for builders, phi edges, and
                // any additional consumer which still needs the loaded pair.
                && uses.get(&callable) == Some(&1)
                && uses.get(&bound_self) == Some(&1)
                && let Some(slot) = entry.cell.as_ref().or_else(|| cells.request(entry.maker.as_ref()?))
            {
                let view = func.fresh_value();
                func.value_types.insert(view, TirType::DynBox);
                let mut load = TirOp::new(OpCode::StaticCellLoad, vec![], vec![view]);
                load.set_global_slot(Some(slot));
                load.inherit_source_from(&op);
                result.push(load);
                // LOAD_METHOD acquired two owned references before any
                // argument ran. A cell read is borrowed; retain it and the
                // original receiver at exactly that point.
                for (from, to) in [(view, callable), (op.operands[0], bound_self)] {
                    let mut alias = TirOp::new(OpCode::Copy, vec![from], vec![to])
                        .with_origin(OpKind::BindingAlias);
                    alias.inherit_source_from(&op);
                    result.push(alias);
                }
                count += 1;
            } else {
                result.push(op);
            }
        }
        func.blocks.get_mut(&block).unwrap().ops = result;
    }
    count
}
