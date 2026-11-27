//! Prove the normal path of a synchronous packed accessor before expanding its frame.
//! The original call is the residual on a guard miss. A cut reached after a
//! successful access must resume with that result and the original frame state.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::ir::GlobalSlotRef;
use molt_ir::literal_payload::LiteralPayload;
use molt_ir::{ClassId, FunctionId, OpKind};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::call_targets::static_call;
use crate::tir::dominators::{checked_exception_observer, exception_label_to_block};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::fast_path_expansion::{PackedAccess, packed_access};
use crate::tir::passes::{async_work_poll, value_identity};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::checked_scalar::CheckedScalar;
use super::sync_frame::SyncFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Site {
    pub block: BlockId,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    pub location: Option<i64>,
    /// (localsplus slot, zero-based formal) pairs in the original frame.
    pub homes: Vec<(i64, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum After {
    FrameEntry,
    Setup,
    FieldLoad,
    Convert,
    BufferAccess,
}

#[derive(Clone, Debug)]
pub(super) struct Origin {
    pub site: Site,
    pub after: After,
    pub target: BlockId,
    pub snapshot: Snapshot,
    /// Includes an authored call-return service not yet marked by placement.
    pub services_pending: bool,
    pub payload: Vec<ValueId>,
    pub checked_word: Option<ValueId>,
}

#[derive(Clone, Debug)]
pub(super) struct PackedRead {
    pub receiver: usize,
    pub row: usize,
    pub column: usize,
    /// Class whose fixed field layout the original load uses. The caller
    /// must prove its receiver reaches this specialization or guard its class.
    pub class: ClassId,
    pub offset: i64,
    pub field_load: Site,
    pub buffer_access: Site,
    pub conversion: Option<Conversion>,
    pub origins: Vec<Origin>,
    pub retirement: (Site, Snapshot),
}

#[derive(Clone, Debug)]
pub(super) struct Conversion {
    pub proof: CheckedScalar,
    pub cell: GlobalSlotRef,
    pub literal: Vec<u8>,
    pub formal: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Formal(usize),
    Converted(usize),
    Field,
    Access,
    Cell,
    Literal,
    None,
    Flag(bool),
}

fn formal(values: &HashMap<ValueId, Source>, value: ValueId) -> Option<usize> {
    match values.get(&value) {
        Some(Source::Formal(position)) => Some(*position),
        _ => None,
    }
}

fn snapshot(frame: &SyncFrame, site: Site, sources: &HashMap<ValueId, Source>) -> Option<Snapshot> {
    let point = frame.point(site.block, site.index)?;
    if point.published {
        return None;
    }
    let homes = point
        .homes
        .iter()
        .map(|&(slot, value)| match *sources.get(&value)? {
            Source::Formal(position) | Source::Converted(position) => Some((slot, position)),
            _ => None,
        })
        .collect::<Option<_>>()?;
    Some(Snapshot {
        location: point.location,
        homes,
    })
}

/// Match actual executable operations and each checked cut. Type annotations,
/// symbol spellings and successful-return forwarding alone cannot establish
/// this effect trace. All exceptional destinations remain in the original body.
pub(super) fn extract<'a>(
    func: &TirFunction,
    resolve: impl Fn(FunctionId) -> Option<&'a TirFunction>,
) -> Option<PackedRead> {
    let write = func.param_types.len() == 4 && func.return_type == TirType::None;
    let kind = if write {
        PackedAccess::Write
    } else {
        PackedAccess::Read
    };

    if !func.return_abi.returns_value()
        || func.has_closure_parameter()
        || (!write && func.param_types.len() != 3)
    {
        return None;
    }
    let frame = SyncFrame::analyze(func)?;
    let labels = exception_label_to_block(func);
    let entry = func.blocks.get(&func.entry_block)?;
    if entry.args.len() != func.param_types.len() {
        return None;
    }
    let mut sources: HashMap<_, _> = entry
        .args
        .iter()
        .enumerate()
        .map(|(position, arg)| (arg.id, Source::Formal(position)))
        .collect();
    let mut entered = false;
    let mut awaiting = None;
    let mut field = None;
    let mut access = None;
    let mut conversion: Option<Conversion> = None;
    let mut cell = None;
    let mut literal = None;
    let mut retirement = None;
    let mut origins = Vec::new();
    let mut visited = HashSet::default();
    let mut id = func.entry_block;
    loop {
        if !visited.insert(id) {
            return None;
        }
        let block = func.blocks.get(&id)?;
        for (index, op) in block.ops.iter().enumerate() {
            let site = Site { block: id, index };
            if retirement.is_some()
                || (op.callee.is_some() && (!write || op.opcode != OpCode::Call))
                || (op.runtime.is_some() && packed_access(op) != Some(kind))
                || !op.has_valid_shape()
                || (op.is_async_work_poll() && op.opcode != OpCode::CheckException)
                || (awaiting.is_some() && op.opcode != OpCode::CheckException)
            {
                return None;
            }
            if op.opcode == OpCode::CheckException {
                if op.exception_edge_kind()? != ExceptionEdgeKind::Origin {
                    return None;
                }
                let (after, call_poll) = awaiting.take().unwrap_or((After::Setup, false));
                // A checked invocation projects only this call's physical
                // result. Its exceptional payload remains on the authored op.
                if op.has_checked_invocation_binding()
                    && (after != After::BufferAccess
                        || op
                            .checked_invocation_source()
                            .and_then(|value| sources.get(&value))
                            != Some(&Source::Access))
                {
                    return None;
                }
                let services_pending = op.is_async_work_poll() || call_poll;
                if services_pending && !matches!(after, After::BufferAccess | After::Convert) {
                    return None;
                }
                let target = checked_exception_observer(op, |label| labels.get(&label).copied())?;
                if after == After::BufferAccess
                    && (!op.exception_payload().is_empty()
                        || op.has_checked_invocation_binding()
                        || !super::packed_wrapper::is_plain_unwind(func, target))
                {
                    return None;
                }
                origins.push(Origin {
                    site,
                    after,
                    services_pending,
                    payload: op.exception_payload().to_vec(),
                    checked_word: op.checked_invocation_source(),
                    target,
                    snapshot: snapshot(&frame, site, &sources)?,
                });
            } else if op.opcode == OpCode::Copy && op.origin == Some(OpKind::TraceEnterSlot) {
                if entered || id != func.entry_block || index != 0 {
                    return None;
                }
                entered = true;
                awaiting = Some((After::FrameEntry, false));
            } else if op.opcode == OpCode::Copy && op.origin == Some(OpKind::FrameHomeStore) {
                let ([value], [view]) = (&*op.operands, &*op.results) else {
                    return None;
                };
                let source = *sources.get(value)?;
                if !matches!(source, Source::Formal(_) | Source::Converted(_)) {
                    return None;
                }
                sources.insert(*view, source);
            } else if op.opcode == OpCode::Copy && op.origin == Some(OpKind::TraceExit) {
                access?;
                retirement = Some((site, snapshot(&frame, site, &sources)?));
            } else if (op.opcode == OpCode::FrameSetLocation
                || (op.opcode == OpCode::Copy && op.origin == Some(OpKind::Line)))
                && op.operands.is_empty()
                && op.results.is_empty()
            {
                // SyncFrame assigns the reaching location to each cut.
            } else if let Some(value) = value_identity::copy_value_source(op) {
                let source = *sources.get(&value)?;
                sources.insert(op.results[0], source);
            } else if write && op.opcode == OpCode::StaticCellLoad && op.operands.is_empty() {
                let slot = op.global_slot()?;
                if cell.is_some() || !(slot.cell && slot.init_once) || conversion.is_some() {
                    return None;
                }
                cell = Some(slot);
                sources.insert(*op.results.first()?, Source::Cell);
            } else if write && op.opcode == OpCode::ConstStr {
                if literal.is_some() || conversion.is_some() {
                    return None;
                }
                literal = Some(LiteralPayload::from_tir(op).ok()?.as_bytes().to_vec());
                sources.insert(op.results[0], Source::Literal);
                awaiting = Some((After::Setup, false));
            } else if let Some((receiver, offset)) = op.plain_typed_slot_load() {
                if field.is_some() || (write && conversion.is_none()) {
                    return None;
                }
                let position = formal(&sources, receiver)?;
                let TirType::Class(class) = func.exact_param_types.get(position)? else {
                    return None;
                };
                field = Some((site, position, *class, offset));
                sources.insert(op.results[0], Source::Field);
                awaiting = Some((After::FieldLoad, false));
            } else if packed_access(op) == Some(kind) {
                let ([payload, row, column, rest @ ..], [result]) = (&*op.operands, &*op.results)
                else {
                    return None;
                };
                if access.is_some() || sources.get(payload) != Some(&Source::Field) {
                    return None;
                }
                match (rest, &conversion) {
                    ([], None) if !write => {}
                    ([value], Some(convert))
                        if write
                            && sources.get(value) == Some(&Source::Converted(convert.formal)) => {}
                    _ => return None,
                }
                access = Some((site, formal(&sources, *row)?, formal(&sources, *column)?));
                sources.insert(*result, Source::Access);
                awaiting = Some((
                    After::BufferAccess,
                    async_work_poll::is_call_return_poll(op),
                ));
            } else if write && op.opcode == OpCode::Call {
                if field.is_some() || conversion.is_some() {
                    return None;
                }
                let call = static_call(op)?;
                let target = resolve(call.callee)?;
                let proof = CheckedScalar::summarize(target)?;
                let args = call.bind(target)?;
                let [first, second] = <[ValueId; 2]>::try_from(args.positional)
                    .ok()?
                    .map(|word| sources.get(&word).copied());
                let (value, diagnostic) = match proof.formal() {
                    0 => (first, second),
                    1 => (second, first),
                    _ => return None,
                };
                let Some(Source::Formal(formal)) = value else {
                    return None;
                };
                if args.first_positional() != 0
                    || diagnostic != Some(Source::Literal)
                    || sources.get(&call.callable?) != Some(&Source::Cell)
                {
                    return None;
                }
                let [result] = op.results[..] else {
                    return None;
                };
                conversion = Some(Conversion {
                    proof,
                    cell: cell.take()?,
                    literal: literal.take()?,
                    formal,
                });
                sources.insert(result, Source::Converted(formal));
                awaiting = Some((After::Convert, async_work_poll::is_call_return_poll(op)));
            } else if write && op.opcode == OpCode::ConstNone && access.is_some() {
                sources.insert(*op.results.first()?, Source::None);
            } else if op.opcode == OpCode::ConstBool {
                let Some(AttrValue::Bool(flag)) = op.attrs.get("value") else {
                    return None;
                };
                sources.insert(op.results[0], Source::Flag(*flag));
            } else {
                return None;
            }
        }
        let (next, args) = match &block.terminator {
            Terminator::Branch { target, args } => (*target, args),
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => match sources.get(cond) {
                Some(Source::Flag(true)) => (*then_block, then_args),
                Some(Source::Flag(false)) => (*else_block, else_args),
                _ => return None,
            },
            Terminator::Return { values } => {
                if awaiting.is_some()
                    || !matches!(&values[..], [v] if sources.get(v) == Some(&if write { Source::None } else { Source::Access }))
                {
                    return None;
                }
                let (field_load, receiver, class, offset) = field?;
                let (buffer_access, row, column) = access?;
                if receiver == row || receiver == column || row == column {
                    return None;
                }
                if let Some(convert) = &conversion
                    && [receiver, row, column].contains(&convert.formal)
                {
                    return None;
                }
                return Some(PackedRead {
                    receiver,
                    row,
                    column,
                    class,
                    offset,
                    field_load,
                    buffer_access,
                    conversion,
                    origins,
                    retirement: retirement?,
                });
            }
            Terminator::Switch { .. }
            | Terminator::StateDispatch { .. }
            | Terminator::Unwind
            | Terminator::Unreachable => return None,
        };
        let params = &func.blocks.get(&next)?.args;
        if params.len() != args.len() {
            return None;
        }
        for (param, arg) in params.iter().zip(args) {
            let source = *sources.get(arg)?;
            sources.insert(param.id, source);
        }
        id = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_ir::runtime_fn_generated::RuntimeFn;

    #[test]
    fn recognizes_the_ordered_read_and_rejects_changed_effects() {
        let ir: crate::ir::FunctionIR = serde_json::from_value(serde_json::json!({
            "name":"leaf", "return_abi":"value", "execution_context":"local",
            "params":["j","object","i"], "param_types":["Any","Any","Any"],
            "ops":[
                {"kind":"trace_enter_slot","s_value":"leaf","value":3},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_home_store","value":0,"args":["object"],"out":"receiver"},
                {"kind":"frame_home_store","value":1,"args":["i"],"out":"row"},
                {"kind":"frame_home_store","value":2,"args":["j"],"out":"column"},
                {"kind":"frame_set_location","value":7},
                {"kind":"load","value":16,"args":["receiver"],"out":"storage"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_set_location","value":6},
                {"kind":"call","s_value":RuntimeFn::Buffer2dGet.symbol(),
                    "args":["storage","row","column"],"out":"got"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_set_location","value":4},
                {"kind":"trace_exit"},{"kind":"ret","args":["got"]},
                {"kind":"label","value":9},{"kind":"trace_exit"},{"kind":"ret_void","unwind":true}
            ]
        }))
        .unwrap();
        let mut func = crate::tir::lower_from_simple::lower_to_tir(&ir);
        let class = ClassId(37);
        let ty = TirType::Class(class);
        let receiver = &mut func.blocks.get_mut(&func.entry_block).unwrap().args[1];
        receiver.ty = ty.clone();
        func.value_types.insert(receiver.id, ty.clone());
        func.exact_param_types = vec![TirType::DynBox, ty, TirType::DynBox];

        let plan = extract(&func, |_| None).expect("typed field and direct runtime read");
        assert_eq!((plan.receiver, plan.row, plan.column), (1, 2, 0));
        assert_eq!((plan.class, plan.offset), (class, 16));
        assert_eq!(
            plan.origins
                .iter()
                .map(|cut| (cut.after, cut.snapshot.location, cut.services_pending))
                .collect::<Vec<_>>(),
            [
                (After::FrameEntry, None, false),
                (After::FieldLoad, Some(7), false),
                (After::BufferAccess, Some(6), true)
            ]
        );
        assert_eq!(plan.origins[2].snapshot.homes, [(0, 1), (1, 2), (2, 0)]);
        assert_eq!(plan.retirement.1.location, Some(4));

        // Recognizing the constituent read never authorizes an observable
        // leaf to take the ordinary body-splicing path by itself.
        let module = crate::tir::function::TirModule::decode_spelled("m", vec![func.clone()]);
        let graph = crate::tir::call_graph::CallGraph::build(&module);
        let summaries = crate::tir::passes::ip_summary::ModuleSummaries::compute(&module, &graph);
        assert_eq!(
            super::super::classify_inline_eligibility(
                module.ids[0],
                &module.functions[0],
                &graph,
                &summaries,
                &crate::tir::target_info::TargetInfo::native_release_fast(),
            ),
            crate::tir::call_facts::InlineEligibility::WhyNot(
                crate::tir::call_facts::InlineWhyNot::ExecutionContext
            ),
        );

        let mut annotated = func.clone();
        annotated.param_types[1] = TirType::Class(class);
        annotated.exact_param_types.clear();
        assert!(
            extract(&annotated, |_| None).is_none(),
            "an annotation is not layout proof"
        );

        let mut changed = func.clone();
        let call = &mut changed
            .blocks
            .get_mut(&plan.buffer_access.block)
            .unwrap()
            .ops[plan.buffer_access.index];
        call.runtime = None;
        assert!(
            extract(&changed, |_| None).is_none(),
            "the printed name does not prove the target"
        );
        let mut changed = func.clone();
        let field =
            &mut changed.blocks.get_mut(&plan.field_load.block).unwrap().ops[plan.field_load.index];
        field.origin = None;
        assert!(
            extract(&changed, |_| None).is_none(),
            "a dynamic field may run Python"
        );
        let cut = plan.origins[1].site;
        func.blocks.get_mut(&cut.block).unwrap().ops[cut.index].mark_async_work_poll();
        assert!(
            extract(&func, |_| None).is_none(),
            "an extra service may replace the field before use"
        );
    }
}
