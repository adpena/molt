pub(super) use std::collections::HashSet;

pub(super) use molt_ir::ParameterCustody;
pub(super) use molt_passes::tir::analysis::AnalysisManager;
pub(super) use molt_passes::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
pub(super) use molt_passes::tir::function::TirFunction;
pub(super) use molt_passes::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub(super) use molt_passes::tir::passes::drop_insertion::{
    DROP_INSERTED_ATTR, EXCEPTION_REGION_DROPS_INSERTED_ATTR, run,
};
pub(super) use molt_passes::tir::passes::liveness::TirLiveness;
pub(super) use molt_passes::tir::types::TirType;
pub(super) use molt_passes::tir::values::{TirValue, ValueId};

use molt_passes::tir::op_kinds_generated::{
    copy_kind_mints_owned_alias_ref_table, kind_consumed_operand_table,
};

pub(super) fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

pub(super) fn const_str(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("x".into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

pub(super) fn finalizer_object(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("defines_del".into(), AttrValue::Bool(true));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

pub(super) fn finalizer_call_bind(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
    attrs.insert("defines_del".into(), AttrValue::Bool(true));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

pub(super) fn count_decrefs(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::DecRef)
        .count()
}
pub(super) fn count_increfs(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::IncRef)
        .count()
}

pub(super) fn original_copy(kind: &str, results: Vec<ValueId>) -> TirOp {
    let mut copy = op(OpCode::Copy, vec![], results);
    copy.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    copy
}

pub(super) fn original_copy_with_operands(
    kind: &str,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
) -> TirOp {
    let mut copy = op(OpCode::Copy, operands, results);
    copy.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    copy
}

pub(super) fn original_store_var(var: &str, operand: ValueId, result: ValueId) -> TirOp {
    let mut copy = original_copy_with_operands("store_var", vec![operand], vec![result]);
    copy.attrs.insert("_var".into(), AttrValue::Str(var.into()));
    copy
}

pub(super) fn try_start(label: i64) -> TirOp {
    let mut start = op(OpCode::TryStart, vec![], vec![]);
    start.attrs.insert("value".into(), AttrValue::Int(label));
    start
}

/// One ownership event along an executed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Event {
    /// A `WarnStderr` op, standing for an observable statement.
    Marker,
    /// The last reference to an object, numbered in creation order, went away.
    Freed(usize),
}

#[derive(Clone, Copy)]
enum Binding {
    /// A raw scalar carrier.
    Raw,
    Object(usize),
}

/// Objects and reference counts along one executed path.
#[derive(Default)]
struct RcPath {
    bindings: std::collections::HashMap<ValueId, Binding>,
    counts: Vec<i64>,
    /// What each frame home holds, by code slot.
    homes: std::collections::BTreeMap<i64, Binding>,
}

impl RcPath {
    fn create(&mut self) -> usize {
        self.counts.push(1);
        self.counts.len() - 1
    }

    /// The binding of a defined value whose object is still referenced.
    fn read(&self, value: ValueId) -> Result<Binding, String> {
        match self.bindings.get(&value) {
            None => Err(format!("{value:?} is undefined on this path")),
            Some(&Binding::Object(object)) if self.counts[object] <= 0 => {
                Err(format!("{value:?} names released object {object}"))
            }
            Some(&binding) => Ok(binding),
        }
    }

    /// Releases one reference to what `binding` names; a raw carrier holds none.
    fn release(&mut self, binding: Binding, events: &mut Vec<Event>) {
        if let Binding::Object(object) = binding {
            self.counts[object] -= 1;
            if self.counts[object] == 0 {
                events.push(Event::Freed(object));
            }
        }
    }

    /// Runs one frame home access, by the home kinds' runtime semantics.
    fn access_home(
        &mut self,
        access: HomeAccess,
        op: &TirOp,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        if let HomeAccess::Exit = access {
            for held in std::mem::take(&mut self.homes).into_values() {
                self.release(held, events);
            }
            return Ok(());
        }
        let slot = match op.attrs.get("value") {
            Some(&AttrValue::Int(slot)) => slot,
            _ => return Err(format!("{op:?} names no home slot")),
        };
        let unbound = || format!("home {slot} is unbound at {op:?}");
        match access {
            HomeAccess::Store => {
                // The home takes the operand's reference, then releases what it
                // held; the result views the stored object.
                let &[operand] = op.operands.as_slice() else {
                    return Err(format!("{op:?} stores no single operand"));
                };
                let stored = self.read(operand)?;
                let displaced = self.homes.insert(slot, stored);
                for &result in &op.results {
                    self.bindings.insert(result, stored);
                }
                if let Some(displaced) = displaced {
                    self.release(displaced, events);
                }
            }
            HomeAccess::Load => {
                let held = *self.homes.get(&slot).ok_or_else(unbound)?;
                for &result in &op.results {
                    self.bindings.insert(result, held);
                }
            }
            HomeAccess::Take => {
                let held = self.homes.remove(&slot).ok_or_else(unbound)?;
                for &result in &op.results {
                    self.bindings.insert(result, held);
                }
            }
            HomeAccess::Clear => {
                let held = self.homes.remove(&slot).ok_or_else(unbound)?;
                self.release(held, events);
            }
            HomeAccess::Exit => unreachable!("handled above"),
        }
        Ok(())
    }
}

/// An access to a frame home (frame-slot custody), modelled by the spelling's
/// runtime semantics rather than by the pass's ownership facts.
#[derive(Clone, Copy)]
enum HomeAccess {
    /// `frame_home_store`, `frame_home_cell`, `frame_home_private_cell`.
    Store,
    /// `frame_home_load`.
    Load,
    /// `frame_home_take`.
    Take,
    /// `frame_home_clear`, a `del`.
    Clear,
    /// `trace_exit`: the frame's exit releases every home.
    Exit,
}

fn home_access(op: &TirOp) -> Option<HomeAccess> {
    if op.opcode != OpCode::Copy {
        return None;
    }
    let Some(AttrValue::Str(kind)) = op.attrs.get("_original_kind") else {
        return None;
    };
    match kind.as_str() {
        "frame_home_store" | "frame_home_cell" | "frame_home_private_cell" => {
            Some(HomeAccess::Store)
        }
        "frame_home_load" => Some(HomeAccess::Load),
        "frame_home_take" => Some(HomeAccess::Take),
        "frame_home_clear" => Some(HomeAccess::Clear),
        "trace_exit" => Some(HomeAccess::Exit),
        _ => None,
    }
}

/// Reference-count model of one path through a drop-inserted function.
///
/// A borrowed parameter holds the caller's reference; a transferred one holds a
/// reference the function owns. Every other result names a new object holding
/// one reference, except that a single-operand `Copy` or no-op `TypeGuard`
/// names its operand's object (an owned alias with a reference of its own) and
/// a `Bool` or `I64` result is a raw carrier. Getter results are independent
/// owned references, as in the Python runtime ABI. RC operations, copies,
/// branch arguments and exception payloads move bits. `IncRef`/`DecRef` adjust
/// the object a value names. An op adopts the reference at each operand
/// position its typed custody or its generated consuming spelling declares: its
/// callee releases that reference before the op completes, whether or not a
/// later observation raises, and every operand the op borrows must survive the
/// release. A frame home owns what it holds, by the home spellings' runtime
/// semantics: a store takes its operand's reference into the home and then
/// releases what the home held, a load names what the home holds, a take moves
/// it out, a `del` releases it, and the frame's exit (`trace_exit`) releases
/// every home. A store's or a load's result is a view that holds no reference.
/// Block arguments name what their arc passes, and a raising observation binds
/// its handler's arguments to its operands. Reading an undefined value or a
/// released object, or an RC operation on a raw carrier, fails the path. At
/// `Return` every home must be empty, every object the path created or was
/// given must be released, except one the Return transfers, and every borrowed
/// parameter must still hold exactly the caller's reference.
///
/// `choices` decides each `CondBranch` (true takes `then`) and each
/// `CheckException` (true raises) in execution order. Once they run out,
/// branches take `else` and nothing raises. `state` selects the
/// `StateDispatch` case. `TryStart` registers its region and never transfers
/// control, as at runtime.
pub(super) fn execute(
    func: &TirFunction,
    state: i64,
    choices: &[bool],
) -> Result<Vec<Event>, String> {
    let labels: std::collections::HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&block, &label)| (label, BlockId(block)))
        .collect();
    let mut path = RcPath::default();
    let mut parameters: Vec<usize> = Vec::new();
    let mut events = Vec::new();
    let mut choices = choices.iter().copied();
    let mut current = func.entry_block;
    for (position, parameter) in func.blocks[&current].args.iter().enumerate() {
        let object = path.create();
        if func.parameter_custody(position) != ParameterCustody::Transferred {
            parameters.push(object);
        }
        path.bindings.insert(parameter.id, Binding::Object(object));
    }
    for _ in 0..10_000 {
        let body = &func.blocks[&current];
        let mut raised = None;
        for op in &body.ops {
            for &operand in &op.operands {
                path.read(operand)?;
            }
            if let Some(access) = home_access(op) {
                path.access_home(access, op, &mut events)?;
                continue;
            }
            match op.opcode {
                OpCode::IncRef | OpCode::DecRef => {
                    let Binding::Object(object) = path.read(op.operands[0])? else {
                        return Err(format!("{:?} of raw {:?}", op.opcode, op.operands[0]));
                    };
                    if op.opcode == OpCode::IncRef {
                        path.counts[object] += 1;
                    } else {
                        path.counts[object] -= 1;
                        if path.counts[object] == 0 {
                            events.push(Event::Freed(object));
                        }
                    }
                }
                OpCode::WarnStderr => events.push(Event::Marker),
                OpCode::TryStart => {}
                OpCode::CheckException => {
                    if choices.next().unwrap_or(false) {
                        let Some(AttrValue::Int(label)) = op.attrs.get("value") else {
                            return Err("observation without a handler label".into());
                        };
                        let handler = labels
                            .get(label)
                            .copied()
                            .ok_or_else(|| format!("unresolved handler label {label}"))?;
                        raised = Some((handler, op.operands.clone()));
                        break;
                    }
                }
                OpCode::Copy if op.operands.len() == 1 => {
                    let source = path.read(op.operands[0])?;
                    // An owned alias names the same object with a reference
                    // of its own, which its lowering retains.
                    let owned_alias = matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if copy_kind_mints_owned_alias_ref_table(kind)
                    );
                    if owned_alias && let Binding::Object(object) = source {
                        path.counts[object] += 1;
                    }
                    for &result in &op.results {
                        path.bindings.insert(result, source);
                    }
                }
                OpCode::TypeGuard
                    if op.operands.len() == 1 && !op.attrs.contains_key("_original_kind") =>
                {
                    let source = path.read(op.operands[0])?;
                    for &result in &op.results {
                        path.bindings.insert(result, source);
                    }
                }
                _ => {
                    for &result in &op.results {
                        let binding = match func.value_types.get(&result) {
                            Some(TirType::Bool | TirType::I64) => Binding::Raw,
                            _ => Binding::Object(path.create()),
                        };
                        path.bindings.insert(result, binding);
                    }
                }
            }
            adopt(&mut path, &mut events, op)?;
        }
        let (target, args) = match raised {
            Some(transfer) => transfer,
            None => match &body.terminator {
                Terminator::Branch { target, args } => (*target, args.clone()),
                Terminator::CondBranch {
                    cond,
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                } => {
                    path.read(*cond)?;
                    if choices.next().unwrap_or(false) {
                        (*then_block, then_args.clone())
                    } else {
                        (*else_block, else_args.clone())
                    }
                }
                Terminator::StateDispatch {
                    cases,
                    default,
                    default_args,
                } => cases
                    .iter()
                    .find(|(case, _, _)| *case == state)
                    .map(|(_, target, args)| (*target, args.clone()))
                    .unwrap_or_else(|| (*default, default_args.clone())),
                Terminator::Return { values } => {
                    if let Some(slot) = path.homes.keys().next() {
                        return Err(format!("home {slot} is still bound at Return"));
                    }
                    let mut returned = Vec::new();
                    for &value in values {
                        if let Binding::Object(object) = path.read(value)? {
                            returned.push(object);
                        }
                    }
                    for (object, &count) in path.counts.iter().enumerate() {
                        let expected = parameters.iter().filter(|&&owner| owner == object).count()
                            + returned.iter().filter(|&&owner| owner == object).count();
                        let expected = i64::try_from(expected).unwrap();
                        if count != expected {
                            return Err(format!(
                                "object {object} ends with {count} references, expected {expected}"
                            ));
                        }
                    }
                    return Ok(events);
                }
                other => return Err(format!("unsupported terminator {other:?}")),
            },
        };
        let incoming = args
            .iter()
            .map(|&value| path.read(value))
            .collect::<Result<Vec<_>, _>>()?;
        let entered = &func.blocks[&target];
        if entered.args.len() != incoming.len() {
            return Err(format!(
                "{target:?} binds {} arguments to {} values",
                entered.args.len(),
                incoming.len()
            ));
        }
        for (argument, binding) in entered.args.iter().zip(incoming) {
            path.bindings.insert(argument.id, binding);
        }
        current = target;
    }
    Err("the path does not return".into())
}

/// The references `op` adopts, at the operand positions its typed custody or
/// its generated consuming spelling declares, in operand order. The callee
/// releases each one before the op completes; a raw carrier holds none. Every
/// operand the op borrows must survive those releases.
fn adopt(path: &mut RcPath, events: &mut Vec<Event>, op: &TirOp) -> Result<(), String> {
    let consumed = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => kind_consumed_operand_table(kind, op.operands.len()),
        _ => None,
    };
    let adopted: Vec<bool> = (0..op.operands.len())
        .map(|position| {
            consumed == Some(position)
                || op.operand_custody(position) == ParameterCustody::Transferred
        })
        .collect();
    if !adopted.contains(&true) {
        return Ok(());
    }
    for (&operand, _) in op.operands.iter().zip(&adopted).filter(|&(_, &adopts)| adopts) {
        if let Binding::Object(object) = path.read(operand)? {
            path.counts[object] -= 1;
            if path.counts[object] == 0 {
                events.push(Event::Freed(object));
            }
        }
    }
    for (&operand, _) in op.operands.iter().zip(&adopted).filter(|&(_, &adopts)| !adopts) {
        path.read(operand)?;
    }
    Ok(())
}

/// Runs DropInsertion and checks that a second run inserts nothing more.
pub(super) fn insert(func: &mut TirFunction) {
    run(func, &mut AnalysisManager::new());
    let printed = molt_passes::tir::printer::print_function(func);
    run(func, &mut AnalysisManager::new());
    assert_eq!(
        molt_passes::tir::printer::print_function(func),
        printed,
        "{}: a second run must not insert again",
        func.name
    );
}

/// The ownership events along one path; a defect fails with the function.
pub(super) fn trace(func: &TirFunction, state: i64, choices: &[bool]) -> Vec<Event> {
    execute(func, state, choices).unwrap_or_else(|defect| {
        panic!(
            "{} along {choices:?}: {defect}\n{}",
            func.name,
            molt_passes::tir::printer::print_function(func)
        )
    })
}

mod activation;
mod call_custody;
mod core_rc;
mod equal_values;
mod exception_edges;
mod exception_regions;
mod frame_homes;
mod phi_transport;
mod point_availability;
mod python_lifetimes;
