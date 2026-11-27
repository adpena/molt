//! The executable successful path of a checked wrapper around a packed read.
//! All sites and SSA operands belong to the indicated immutable module body.
//! A consumer must preserve the listed observations even on a fast hit.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::frame_entry::{FrameEntryKind, FramePrefix, FrameShape};
use molt_ir::ir::GlobalSlotRef;
use molt_ir::literal_payload::LiteralPayload;
use molt_ir::{ClassId, FunctionId, OpKind};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::call_targets::static_call;
use crate::tir::dominators::{checked_exception_observer, exception_label_to_block};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::{async_work_poll, value_identity};
use crate::tir::type_refine::ExactScalarPlan;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::packed_read;
use super::sync_frame::SyncFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    pub function: FunctionId,
    pub block: BlockId,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub entry: Site,
    pub owner: String,
    pub shape: FrameShape,
    pub prefix: FramePrefix,
    formals: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Formal(usize),
    Literal(Site),
    Cell(Site),
    CallResult(Site),
    Flag(bool),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Argument {
    pub word: ValueId,
    pub source: Source,
}

#[derive(Clone, Debug)]
pub(crate) struct Invocation {
    pub site: Site,
    pub target: FunctionId,
    /// The suspended physical parent at this invocation.
    pub location: Option<i64>,
    pub homes: Vec<(i64, usize)>,
    /// The physical operand and its dominating, immutable cell publication.
    /// An owned LOAD_METHOD alias is recorded separately in the event stream.
    pub callable: Argument,
    pub arguments: Vec<Argument>,
}

#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub site: Site,
    pub exceptional: BlockId,
    pub location: Option<i64>,
    /// Reaching homes in the formal numbering of the check's own function.
    pub homes: Vec<(i64, usize)>,
    pub services_pending: bool,
    pub payload: Vec<ValueId>,
    pub checked_word: Option<ValueId>,
}

#[derive(Clone, Debug)]
pub(crate) enum Event {
    Enter(Site),
    /// Transparent copies, home stores, locations and bound-result projections.
    Transport,
    OwnedAlias {
        site: Site,
        source: Source,
        result: ValueId,
    },
    Cell {
        site: Site,
        slot: GlobalSlotRef,
    },
    Literal {
        site: Site,
        bytes: Vec<u8>,
    },
    Invoke(Site),
    Check(Check),
    LoadField(Site),
    ReadPacked(Site),
    Retire {
        site: Site,
        location: Option<i64>,
        homes: Vec<(i64, usize)>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PackedWrapper {
    pub wrapper: Frame,
    pub identity: Frame,
    pub leaf: Frame,
    pub identity_return_parameter: usize,
    /// Receiver, row and column positions in the leaf signature.
    pub leaf_formals: [usize; 3],
    pub identity_call: Invocation,
    pub leaf_call: Invocation,
    pub receiver: usize,
    pub row: usize,
    pub column: usize,
    pub class: ClassId,
    pub offset: i64,
    pub events: Vec<Event>,
}

fn frame(func: &TirFunction, id: FunctionId, prefix: FramePrefix) -> Option<Frame> {
    let entry = func.blocks.get(&func.entry_block)?.ops.first()?;
    if !func.return_abi.returns_value()
        || func.has_closure_parameter()
        || entry.origin != Some(OpKind::TraceEnterSlot)
    {
        return None;
    }
    let AttrValue::Int(encoded) = entry.attrs.get("value")? else {
        return None;
    };
    let AttrValue::Str(owner) = entry.attrs.get("s_value")? else {
        return None;
    };
    let shape = FrameEntryKind::from_entry_value(*encoded).shape()?;
    if owner.is_empty() || shape.entry_value() != *encoded {
        return None;
    }
    Some(Frame {
        entry: Site {
            function: id,
            block: func.entry_block,
            index: 0,
        },
        owner: owner.clone(),
        shape,
        prefix,
        formals: func.param_types.len(),
    })
}

struct Trace {
    events: Vec<Event>,
    calls: Vec<Invocation>,
    returned: Source,
    forwarded: Option<ForwardedReceiver>,
}

#[derive(Clone, Copy)]
struct ForwardedReceiver {
    call: Site,
    parameter: usize,
    formal: usize,
}

impl ForwardedReceiver {
    fn prove(wrapper: &TirFunction, call: &Invocation, callee: &TirFunction) -> Option<Self> {
        let scalar = ExactScalarPlan::new(callee);
        let parameter = scalar
            .return_parameter_search()?
            .advance(&scalar, |_| Ok::<_, ()>(None))
            .ok()??;
        let Source::Formal(formal) = call.arguments.get(parameter)?.source else {
            return None;
        };
        let class @ TirType::Class(_) = wrapper.exact_param_types.get(formal)? else {
            return None;
        };
        if callee.exact_param_types.get(parameter) != Some(class)
            || !call
                .arguments
                .iter()
                .all(|arg| matches!(arg.source, Source::Formal(_) | Source::Literal(_)))
        {
            return None;
        }
        Some(Self {
            call: call.site,
            parameter,
            formal,
        })
    }
}

/// A committed packed read can leave through the captured frame chain. This
/// is equivalent to the authored exceptional successor only when it merely
/// clears this activation and unwinds. Payload cleanup, a handler, or any
/// additional Python operation needs the original continuation.
fn forwards_exception(func: &TirFunction, check: &Check) -> bool {
    if !check.payload.is_empty() || check.checked_word.is_some() {
        return false;
    }
    let mut visited = HashSet::default();
    let mut current = check.exceptional;
    let mut exited = false;
    while visited.insert(current) {
        let Some(block) = func.blocks.get(&current) else {
            return false;
        };
        if !block.args.is_empty() {
            return false;
        }
        for op in &block.ops {
            if !op.operands.is_empty()
                || !op.results.is_empty()
                || op.callee.is_some()
                || op.runtime.is_some()
                || op.is_async_work_poll()
                || op.opcode != OpCode::Copy
            {
                return false;
            }
            match op.origin {
                Some(OpKind::Line) => {}
                Some(OpKind::TraceExit) if !exited => exited = true,
                _ => return false,
            }
        }
        match &block.terminator {
            Terminator::Branch { target, args } if args.is_empty() => current = *target,
            Terminator::Unwind => return exited,
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::StateDispatch { .. }
            | Terminator::Return { .. }
            | Terminator::Unreachable => return false,
        }
    }
    false
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Role {
    Wrapper,
    Identity,
}

fn snapshot(
    state: &SyncFrame,
    site: Site,
    sources: &HashMap<ValueId, Source>,
    forwarded: Option<ForwardedReceiver>,
) -> Option<(Option<i64>, Vec<(i64, usize)>)> {
    let point = state.point(site.block, site.index)?;
    if point.published {
        return None;
    }
    let homes = point
        .homes
        .iter()
        .map(|&(slot, word)| match *sources.get(&word)? {
            Source::Formal(position) => Some((slot, position)),
            Source::CallResult(call) => {
                let proof = forwarded.filter(|proof| proof.call == call)?;
                Some((slot, proof.formal))
            }
            _ => None,
        })
        .collect::<Option<_>>()?;
    Some((point.location, homes))
}

/// Walk the sole proven normal path. Unknown branches, including polymorphic
/// type checks in a helper that happens to return its argument, refuse.
fn trace<'a>(
    func: &'a TirFunction,
    frame: &Frame,
    role: Role,
    resolve: &impl Fn(FunctionId) -> Option<&'a TirFunction>,
) -> Option<Trace> {
    let state = SyncFrame::analyze(func)?;
    let labels = exception_label_to_block(func);
    let entry = &func.blocks.get(&func.entry_block)?.args;
    if entry.len() != frame.formals {
        return None;
    }
    let mut sources: HashMap<_, _> = entry
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, Source::Formal(i)))
        .collect();
    let mut visited = HashSet::default();
    let mut calls: Vec<Invocation> = Vec::new();
    let mut events = Vec::new();
    let mut forwarded = None;
    let mut id = func.entry_block;
    let mut retired = false;
    // Some effects require an immediately authored check. Its successful
    // word cannot be used until this observation has executed.
    let mut required: Option<(Option<ValueId>, bool)> = None;
    loop {
        if !visited.insert(id) {
            return None;
        }
        let block = func.blocks.get(&id)?;
        for (index, op) in block.ops.iter().enumerate() {
            let site = Site {
                function: frame.entry.function,
                block: id,
                index,
            };
            if retired
                || !op.has_valid_shape()
                || op.runtime.is_some()
                || op.source_span.is_some_and(|(a, b)| a > b)
                || (op.is_async_work_poll() && op.opcode != OpCode::CheckException)
                || (op.callee.is_some() && op.opcode != OpCode::Call)
                || (required.is_some() && op.opcode != OpCode::CheckException)
            {
                return None;
            }
            if op.opcode == OpCode::CheckException {
                if op.exception_edge_kind()? != ExceptionEdgeKind::Origin {
                    return None;
                }
                let (word, must_service) = required.take().unwrap_or((None, false));
                if op.has_checked_invocation_binding()
                    && (word.is_none() || op.checked_invocation_source() != word)
                {
                    return None;
                }
                let (location, homes) = snapshot(&state, site, &sources, forwarded)?;
                events.push(Event::Check(Check {
                    site,
                    exceptional: checked_exception_observer(op, |l| labels.get(&l).copied())?,
                    location,
                    homes,
                    services_pending: must_service || op.is_async_work_poll(),
                    payload: op.exception_payload().to_vec(),
                    checked_word: op.checked_invocation_source(),
                }));
            } else if site == frame.entry {
                events.push(Event::Enter(site));
                required = Some((None, false));
            } else if op.opcode == OpCode::Copy && op.origin == Some(OpKind::TraceExit) {
                let (location, homes) = snapshot(&state, site, &sources, forwarded)?;
                events.push(Event::Retire {
                    site,
                    location,
                    homes,
                });
                retired = true;
            } else if (op.opcode == OpCode::FrameSetLocation
                || (op.opcode == OpCode::Copy && op.origin == Some(OpKind::Line)))
                && op.operands.is_empty()
                && op.results.is_empty()
            {
                // The frame analysis assigns the actual reaching location to each Origin.
                events.push(Event::Transport);
            } else if op.opcode == OpCode::Copy
                && matches!(
                    op.origin,
                    Some(OpKind::FrameHomeStore | OpKind::BindingAlias)
                )
            {
                let source = value_identity::forwarded_python_value_source(op)?;
                let value = *sources.get(&source)?;
                // A binding alias may have to box a wide integer. The only
                // owned aliases we commute are known object words.
                if op.origin == Some(OpKind::BindingAlias)
                    && !match value {
                        Source::Formal(i) => {
                            matches!(func.exact_param_types.get(i), Some(TirType::Class(_)))
                        }
                        Source::Cell(_) => true,
                        Source::CallResult(origin) => {
                            calls.first().is_some_and(|call| call.site == origin)
                        }
                        _ => false,
                    }
                {
                    return None;
                }
                let result = *op.results.first()?;
                sources.insert(result, value);
                if op.origin == Some(OpKind::BindingAlias) {
                    events.push(Event::OwnedAlias {
                        site,
                        source: value,
                        result,
                    });
                } else {
                    events.push(Event::Transport);
                }
            } else if let Some(v) = value_identity::copy_value_source(op) {
                let value = *sources.get(&v)?;
                if op.checked_call_result_source().is_some()
                    && !matches!(value, Source::CallResult(_))
                {
                    return None;
                }
                sources.insert(op.results[0], value);
                events.push(Event::Transport);
            } else if op.opcode == OpCode::ConstBool {
                let flag = match op.attrs.get("value")? {
                    AttrValue::Bool(v) => *v,
                    AttrValue::Int(v) => *v != 0,
                    _ => return None,
                };
                sources.insert(op.results[0], Source::Flag(flag));
                events.push(Event::Transport);
            } else if role == Role::Wrapper
                && op.opcode == OpCode::StaticCellLoad
                && op.operands.is_empty()
            {
                let slot = op.global_slot()?;
                if !(slot.cell && slot.init_once) {
                    return None;
                }
                sources.insert(*op.results.first()?, Source::Cell(site));
                events.push(Event::Cell { site, slot });
            } else if role == Role::Wrapper && op.opcode == OpCode::ConstStr {
                let bytes = LiteralPayload::from_tir(op).ok()?.as_bytes().to_vec();
                sources.insert(op.results[0], Source::Literal(site));
                events.push(Event::Literal { site, bytes });
                required = Some((None, false));
            } else if role == Role::Wrapper && op.opcode == OpCode::Call {
                if calls.len() == 2 {
                    return None;
                }
                let call = static_call(op)?;
                let callee = resolve(call.callee)?;
                let bound = call.bind(callee)?;
                let [word] = op.results[..] else {
                    return None;
                };
                let function = call.callable?;
                if bound.first_positional() != 0
                    || !matches!(sources.get(&function), Some(Source::Cell(_)))
                {
                    return None;
                }
                let arg = |word: ValueId| {
                    Some(Argument {
                        word,
                        source: *sources.get(&word)?,
                    })
                };
                let arguments = bound
                    .positional
                    .iter()
                    .map(|&value| arg(value))
                    .collect::<Option<_>>()?;
                let (location, homes) = snapshot(&state, site, &sources, forwarded)?;
                let invocation = Invocation {
                    location,
                    homes,
                    site,
                    target: call.callee,
                    callable: arg(function)?,
                    arguments,
                };
                if calls.is_empty() {
                    forwarded = Some(ForwardedReceiver::prove(func, &invocation, callee)?);
                }
                calls.push(invocation);
                sources.insert(word, Source::CallResult(site));
                events.push(Event::Invoke(site));
                required = Some((Some(word), async_work_poll::is_call_return_poll(op)));
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
            } if required.is_none() => match sources.get(cond)? {
                Source::Flag(true) => (*then_block, then_args),
                Source::Flag(false) => (*else_block, else_args),
                _ => return None,
            },
            Terminator::Return { values } if retired && required.is_none() => {
                let [v] = values[..] else {
                    return None;
                };
                return Some(Trace {
                    events,
                    calls,
                    returned: *sources.get(&v)?,
                    forwarded,
                });
            }
            Terminator::CondBranch { .. }
            | Terminator::Return { .. }
            | Terminator::Switch { .. }
            | Terminator::StateDispatch { .. }
            | Terminator::Unwind
            | Terminator::Unreachable => return None,
        };
        let params = &func.blocks.get(&next)?.args;
        if params.len() != args.len() {
            return None;
        }
        let incoming = args
            .iter()
            .map(|v| sources.get(v).copied())
            .collect::<Option<Vec<_>>>()?;
        sources.extend(params.iter().zip(incoming).map(|(p, value)| (p.id, value)));
        id = next;
    }
}

/// Compose only immutable, unexpanded source bodies. Function-relative SSA
/// numbers remain distinct even when two callees use the same value numbers.
pub(crate) fn summarize<'a>(
    wrapper_id: FunctionId,
    wrapper: &'a TirFunction,
    resolve: impl Fn(FunctionId) -> Option<&'a TirFunction>,
) -> Option<PackedWrapper> {
    let wrapper_frame = frame(wrapper, wrapper_id, FramePrefix::default())?;
    let outer = trace(wrapper, &wrapper_frame, Role::Wrapper, &resolve)?;
    let ForwardedReceiver {
        formal: receiver,
        parameter: forwarded,
        ..
    } = outer.forwarded?;
    let [identity_call, leaf_call]: [Invocation; 2] = outer.calls.try_into().ok()?;
    if identity_call.target == wrapper_id
        || leaf_call.target == wrapper_id
        || identity_call.target == leaf_call.target
        || outer.returned != Source::CallResult(leaf_call.site)
    {
        return None;
    }
    let mut after_leaf = outer
        .events
        .iter()
        .skip_while(|event| !matches!(event, Event::Invoke(site) if *site == leaf_call.site));
    if !matches!(after_leaf.nth(1), Some(Event::Check(check)) if forwards_exception(wrapper, check))
    {
        return None;
    }

    let nested_prefix = wrapper_frame.prefix.enter(wrapper_frame.shape)?;
    let helper = resolve(identity_call.target)?;
    let identity = frame(helper, identity_call.target, nested_prefix)?;
    let mut helper_path = trace(helper, &identity, Role::Identity, &resolve)?;
    if helper_path.returned != Source::Formal(forwarded) {
        return None;
    }

    let body = resolve(leaf_call.target)?;
    let leaf = frame(body, leaf_call.target, nested_prefix)?;
    let packed = packed_read::extract(body)?;
    let class = TirType::Class(packed.class);
    if wrapper.exact_param_types.get(receiver) != Some(&class)
        || leaf_call.arguments.get(packed.receiver)?.source
            != Source::CallResult(identity_call.site)
    {
        return None;
    }
    let Source::Formal(row) = leaf_call.arguments.get(packed.row)?.source else {
        return None;
    };
    let Source::Formal(column) = leaf_call.arguments.get(packed.column)?.source else {
        return None;
    };

    let leaf_site = |site: packed_read::Site| Site {
        function: leaf_call.target,
        block: site.block,
        index: site.index,
    };
    let mut leaf_events = vec![Event::Enter(leaf.entry)];
    for origin in packed.origins {
        match origin.after {
            packed_read::After::FieldLoad => {
                leaf_events.push(Event::LoadField(leaf_site(packed.field_load)))
            }
            packed_read::After::BufferRead => {
                leaf_events.push(Event::ReadPacked(leaf_site(packed.buffer_read)))
            }
            _ => {}
        }
        let site = leaf_site(origin.site);
        let check = body.blocks.get(&site.block)?.ops.get(site.index)?;
        let observed = Check {
            site,
            exceptional: origin.target,
            location: origin.snapshot.location,
            homes: origin.snapshot.homes,
            services_pending: origin.services_pending,
            payload: check.exception_payload().to_vec(),
            checked_word: check.checked_invocation_source(),
        };
        if origin.after == packed_read::After::BufferRead && !forwards_exception(body, &observed) {
            return None;
        }
        leaf_events.push(Event::Check(observed));
    }
    leaf_events.push(Event::Retire {
        site: leaf_site(packed.retirement.0),
        location: packed.retirement.1.location,
        homes: packed.retirement.1.homes,
    });
    let mut events = Vec::new();
    for event in outer.events {
        let child = match &event {
            Event::Invoke(site) if *site == identity_call.site => Some(&mut helper_path.events),
            Event::Invoke(site) if *site == leaf_call.site => Some(&mut leaf_events),
            _ => None,
        };
        events.push(event);
        if let Some(child) = child {
            events.append(child);
        }
    }
    Some(PackedWrapper {
        wrapper: wrapper_frame,
        identity,
        leaf,
        identity_return_parameter: forwarded,
        leaf_formals: [packed.receiver, packed.row, packed.column],
        identity_call,
        leaf_call,
        receiver,
        row,
        column,
        class: packed.class,
        offset: packed.offset,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::TirOp;
    use molt_ir::runtime_fn_generated::RuntimeFn;
    use serde_json::{Value, json};

    fn body(
        owner: &str,
        formals: &[&str],
        receiver: usize,
        ops: Value,
        result: &str,
    ) -> TirFunction {
        let mut all = vec![
            json!({"kind":"trace_enter_slot","s_value":owner,"value":formals.len()}),
            json!({"kind":"check_exception","exception_edge":"origin","value":90}),
        ];
        for (i, formal) in formals.iter().enumerate() {
            all.push(json!({"kind":"frame_home_store","value":i,"args":[formal],"out":format!("h_{formal}")}));
        }
        all.extend_from_slice(ops.as_array().unwrap());
        all.extend([
            json!({"kind":"trace_exit"}),
            json!({"kind":"ret","args":[result]}),
            json!({"kind":"label","value":90}),
            json!({"kind":"trace_exit"}),
            json!({"kind":"ret_void","unwind":true}),
        ]);
        let ir = serde_json::from_value(
            json!({"name":owner,"return_abi":"value","execution_context":"local",
            "params":formals,"param_types":vec!["Any";formals.len()],"ops":all}),
        )
        .unwrap();
        let mut body = crate::tir::lower_from_simple::lower_to_tir(&ir);
        let class = TirType::Class(ClassId(123));
        body.exact_param_types = vec![TirType::DynBox; formals.len()];
        body.exact_param_types[receiver] = class.clone();
        let arg = &mut body.blocks.get_mut(&body.entry_block).unwrap().args[receiver];
        arg.ty = class.clone();
        body.value_types.insert(arg.id, class);
        body
    }

    #[test]
    fn composes_proven_captures_and_preserves_nested_check_order() {
        let outer_id = FunctionId(0);
        let helper_id = FunctionId(1);
        let leaf_id = FunctionId(2);
        let helper = body(
            "second",
            &["label", "object"],
            1,
            json!([
                {"kind":"frame_set_location","value":8},
                {"kind":"binding_alias","args":["h_object"],"out":"alias"}
            ]),
            "alias",
        );
        let leaf = body(
            "third",
            &["column", "object", "row"],
            1,
            json!([
                {"kind":"frame_set_location","value":7},
                {"kind":"load","value":16,"args":["h_object"],"out":"field"},
                {"kind":"check_exception","exception_edge":"origin","value":90},
                {"kind":"frame_set_location","value":6},
                {"kind":"call","s_value":RuntimeFn::Buffer2dGet.symbol(),"args":["field","h_row","h_column"],"out":"got"},
                {"kind":"check_exception","exception_edge":"origin","value":90},
                {"kind":"frame_set_location","value":4}
            ]),
            "got",
        );
        let mut wrapper = body(
            "first",
            &["row", "column", "object"],
            2,
            json!([
                {"kind":"const_none","out":"first_cell"},
                {"kind":"frame_set_location","value":2},
                {"kind":"const_str","bytes":[237,160,128],"out":"literal"},
                {"kind":"check_exception","exception_edge":"origin","value":90},
                {"kind":"frame_set_location","value":3},
                {"kind":"call","s_value":"second","args":["first_cell","literal","h_object"],"out":"checked"},
                {"kind":"check_exception","exception_edge":"origin","value":90},
                {"kind":"frame_home_store","value":3,"args":["checked"],"out":"checked_home"},
                {"kind":"const_none","out":"second_cell"},
                {"kind":"binding_alias","args":["second_cell"],"out":"held_fn"},
                {"kind":"binding_alias","args":["checked_home"],"out":"held_object"},
                {"kind":"frame_set_location","value":5},
                {"kind":"call","s_value":"third","args":["held_fn","h_column","held_object","h_row"],"out":"got"},
                {"kind":"check_exception","exception_edge":"origin","value":90},
                {"kind":"frame_set_location","value":1}
            ]),
            "got",
        );

        // Install the same typed publications supplied by module integration.
        for op in wrapper.blocks.values_mut().flat_map(|b| &mut b.ops) {
            if op.origin == Some(OpKind::TraceEnterSlot) {
                op.attrs.insert("value".into(), AttrValue::Int(4));
            } else if op.opcode == OpCode::ConstNone {
                let Some(AttrValue::Str(name)) = op.attrs.get("_simple_out") else {
                    continue;
                };
                let index = match name.as_str() {
                    "first_cell" => 7,
                    "second_cell" => 20,
                    _ => continue,
                };
                *op = TirOp::new(OpCode::StaticCellLoad, vec![], op.results.clone());
                op.set_global_slot(Some(&GlobalSlotRef {
                    module: "fixture".into(),
                    index,
                    cell: true,
                    init_once: true,
                }));
            } else if op.opcode == OpCode::Call {
                let Some(AttrValue::Str(name)) = op.attrs.remove("s_value") else {
                    panic!("typed call")
                };
                op.callee = Some(match name.as_str() {
                    "second" => helper_id,
                    "third" => leaf_id,
                    _ => unreachable!(),
                });
                op.origin = Some(OpKind::CallGuarded);
                op.set_proven_target(true);
            }
        }
        let resolve = |id| {
            if id == helper_id {
                Some(&helper)
            } else if id == leaf_id {
                Some(&leaf)
            } else {
                None
            }
        };
        let plan = summarize(outer_id, &wrapper, resolve).expect("the composed checked path");
        assert_eq!((plan.receiver, plan.row, plan.column), (2, 0, 1));
        assert_eq!(plan.identity_return_parameter, 1);
        assert_eq!(plan.leaf_formals, [1, 2, 0]);
        assert_eq!((plan.class, plan.offset), (ClassId(123), 16));
        assert_eq!(
            plan.identity.prefix,
            plan.wrapper.prefix.enter(plan.wrapper.shape).unwrap()
        );
        assert_eq!(plan.leaf.prefix, plan.identity.prefix);
        assert_eq!(
            plan.leaf_call.arguments[1].source,
            Source::CallResult(plan.identity_call.site)
        );
        assert_eq!(plan.identity_call.homes, [(0, 0), (1, 1), (2, 2)]);
        assert_eq!(plan.leaf_call.homes, [(0, 0), (1, 1), (2, 2), (3, 2)]);
        assert!(plan.events.iter().any(|event| matches!(
            event,
            Event::Check(check)
                if check.site.function == outer_id
                    && check.location == Some(5)
                    && check.homes == plan.leaf_call.homes
        )));
        let sites: Vec<_> = plan
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Check(check) if check.services_pending => {
                    Some((check.site.function, check.location))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            sites,
            [(outer_id, Some(3)), (leaf_id, Some(6)), (outer_id, Some(5))]
        );
        let observed: Vec<_> = plan
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Cell { slot, .. } => Some(slot.index as i64),
                Event::Literal { bytes, .. } if bytes == &[237, 160, 128] => Some(-1),
                Event::Check(check)
                    if check.site.function == outer_id && check.location == Some(2) =>
                {
                    Some(-2)
                }
                Event::Check(check)
                    if check.site.function == outer_id && check.services_pending =>
                {
                    check.location.map(|l| -l)
                }
                _ => None,
            })
            .collect();
        assert_eq!(observed, [7, -1, -2, -3, 20, -5]);

        super::super::packed_wrapper_prefix::verify_prefix(&wrapper, &plan);

        let mut effect = helper.clone();
        let ret = effect
            .blocks
            .values_mut()
            .find(|b| matches!(b.terminator, Terminator::Return { .. }))
            .unwrap();
        let exit = ret
            .ops
            .iter()
            .position(|op| op.origin == Some(OpKind::TraceExit))
            .unwrap();
        ret.ops
            .insert(exit, TirOp::new(OpCode::IncRef, vec![ValueId(1)], vec![]));
        assert!(
            summarize(outer_id, &wrapper, |id| if id == helper_id {
                Some(&effect)
            } else {
                resolve(id)
            })
            .is_none()
        );
        let mut dynamic = wrapper.clone();
        let capture = plan.leaf_call.site;
        dynamic.blocks.get_mut(&capture.block).unwrap().ops[capture.index].set_proven_target(false);
        assert!(summarize(outer_id, &dynamic, resolve).is_none());
        let mut annotated = wrapper.clone();
        annotated.param_types[2] = TirType::Class(plan.class);
        annotated.exact_param_types.clear();
        assert!(summarize(outer_id, &annotated, resolve).is_none());
    }
}
