//! A guarded replacement of a synchronous fixed-field packed read. The
//! probes are non-owning and cannot run Python. Before entering the fast
//! activation, every miss can still execute the original call in full.

use molt_ir::ParameterCustody;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::activation::{Binding, prepare_activation};
use super::packed_read::{After, PackedRead, Site};
use super::sync_frame::SyncFrame;

pub(super) struct Hit {
    pub body: TirFunction,
    owner: String,
    shape: i64,
    offset: i64,
    receiver: ValueId,
    row: ValueId,
    column: ValueId,
    callable: ValueId,
}

/// Add one read-only test to a guarded path. The test's final result names
/// the predicate; any earlier results are available only on the accepted path.
pub(super) fn test_block(
    caller: &mut TirFunction,
    mut ops: Vec<TirOp>,
    test: TirOp,
    accepted: BlockId,
    refused: BlockId,
) -> BlockId {
    let predicate = *test
        .results
        .last()
        .expect("a guarded test yields a predicate");
    ops.push(test);
    caller.value_types.insert(predicate, TirType::Bool);
    let id = caller.fresh_block();
    caller.blocks.insert(
        id,
        TirBlock {
            id,
            args: vec![],
            ops,
            terminator: Terminator::CondBranch {
                cond: predicate,
                then_block: accepted,
                then_args: vec![],
                else_block: refused,
                else_args: vec![],
            },
        },
    );
    id
}

fn input(body: &mut TirFunction, ty: TirType) -> ValueId {
    let id = body.fresh_value();
    body.value_types.insert(id, ty.clone());
    body.param_names.push(String::new());
    body.param_types.push(ty.clone());
    if !body.exact_param_types.is_empty() {
        body.exact_param_types.push(TirType::DynBox);
    }
    body.blocks
        .get_mut(&body.entry_block)
        .unwrap()
        .args
        .push(TirValue { id, ty });
    id
}

fn field_used_only_for_read(func: &TirFunction, field: ValueId, read: Site) -> bool {
    func.blocks.iter().all(|(&id, block)| {
        let mut escapes = false;
        block
            .terminator
            .for_each_value(|value| escapes |= value == field);
        !escapes
            && block.ops.iter().enumerate().all(|(index, op)| {
                (id == read.block && index == read.index) || !op.operands.contains(&field)
            })
    })
}

impl Hit {
    pub(super) fn prepare(
        source: &TirFunction,
        read: &PackedRead,
        callable: ValueId,
        arguments: &[ValueId],
        bindings: &[Binding],
    ) -> Option<Self> {
        // The call binder selected this exact specialization. A physical
        // slot probe can establish addressability but cannot prove the class.
        if source.exact_param_types.get(read.receiver) != Some(&TirType::Class(read.class)) {
            return None;
        }
        let marker = source.blocks.get(&source.entry_block)?.ops.first()?;
        let AttrValue::Str(owner) = marker.attrs.get("s_value")? else {
            return None;
        };
        let AttrValue::Int(shape) = marker.attrs.get("value")? else {
            return None;
        };
        let cut = read
            .origins
            .iter()
            .find(|cut| cut.after == After::BufferRead)?;
        let check = &source.blocks[&cut.site.block].ops[cut.site.index];
        if check.has_checked_invocation_binding() || !cut.services_pending {
            return None;
        }
        let load_site = read.field_load;
        let load = &source.blocks[&load_site.block].ops[load_site.index];
        let held = *load.results.first()?;
        // Before terminal drops, the field has no explicit release.
        // Its sole use keeps the field through the return check. Other users,
        // explicit releases and exceptional field payloads require their own
        // lifetime, so retain the ordinary inlining decision for such bodies.
        if read.field_release.is_some() || !field_used_only_for_read(source, held, read.buffer_read)
        {
            return None;
        }
        // A full-range raw coordinate can acquire a heap identity when it
        // becomes a Python home. The entry probes admit inline words only.
        let mut body = source.clone();
        let field = input(&mut body, TirType::DynBox);
        let raw = input(&mut body, TirType::I64);
        let mut custody = read.parameter_custody.to_vec();
        custody.extend([ParameterCustody::Borrowed; 2]);
        body.set_parameter_custody(&custody);
        let load = &mut body.blocks.get_mut(&read.field_load.block)?.ops[read.field_load.index];
        let mut view = TirOp::new(OpCode::Copy, vec![field], vec![held]);
        view.source_span = load.source_span;
        *load = view;
        let get = &mut body.blocks.get_mut(&read.buffer_read.block)?.ops[read.buffer_read.index];
        let mut preserve = TirOp::new(OpCode::Copy, vec![raw], get.results.clone());
        preserve.source_span = get.source_span;
        preserve.mark_exact_result_proof(&TirType::I64);
        preserve.mark_inline_int_proof();
        *get = preserve;

        // Materializing the original checked call's deferred service remains
        // mandatory even when ordinary poll placement has not run yet.
        let ops = &mut body.blocks.get_mut(&cut.site.block)?.ops;
        ops[cut.site.index].mark_async_work_poll();
        let mut binding = bindings.to_vec();
        binding.extend([Binding::Direct; 2]);
        let body = prepare_activation(&body, &binding)?;
        // Home stores on the hit adopt their operands as physical home
        // stores do and retire those credits at the original trace exit.
        let frame = SyncFrame::analyze_with_owned_formals(&body, true)?;
        let body = super::frame_splice::prepare_with_frame(&body, frame, Some((cut.site, field)))?;
        Some(Self {
            body,
            owner: owner.clone(),
            shape: *shape,
            offset: read.offset,
            receiver: *arguments.get(read.receiver)?,
            row: *arguments.get(read.row)?,
            column: *arguments.get(read.column)?,
            callable,
        })
    }

    /// Build the read-only admission blocks backwards. Their SSA values
    /// dominate the spliced body exclusively along the all-guards-hit path.
    pub(super) fn guard(
        &self,
        caller: &mut TirFunction,
        hit: BlockId,
        miss: BlockId,
        span: Option<(u32, u32)>,
    ) -> (BlockId, [ValueId; 2]) {
        let field = caller.fresh_value();
        caller.value_types.insert(field, TirType::DynBox);
        let raw = caller.fresh_value();
        caller.value_types.insert(raw, TirType::I64);
        let mut guard = |opcode,
                         inputs,
                         mut results: Vec<_>,
                         attrs: &[(&str, AttrValue)],
                         pass: BlockId,
                         ops: Vec<TirOp>| {
            let flag = caller.fresh_value();
            results.push(flag);
            let mut op = TirOp::new(opcode, inputs, results);
            op.source_span = span;
            op.attrs.extend(
                attrs
                    .iter()
                    .map(|(name, value)| ((*name).into(), value.clone())),
            );
            test_block(caller, ops, op, pass, miss)
        };
        // The boxed runtime get can raise while allocating a wide integer.
        // A wide sample must execute that allocation and its original Origin.
        let fits = guard(OpCode::IsInlineInt, vec![raw], vec![], &[], hit, vec![]);
        let mut probe = TirOp::new(OpCode::ProbeInlineField, vec![self.receiver], vec![field]);
        probe.source_span = span;
        probe
            .attrs
            .insert("value".into(), AttrValue::Int(self.offset));
        let load = guard(
            OpCode::TryBuffer2dLoadI64,
            vec![field, self.row, self.column],
            vec![raw],
            &[],
            fits,
            vec![probe],
        );
        let col = guard(
            OpCode::IsInlineInt,
            vec![self.column],
            vec![],
            &[],
            load,
            vec![],
        );
        let row = guard(
            OpCode::IsInlineInt,
            vec![self.row],
            vec![],
            &[],
            col,
            vec![],
        );
        let entry = guard(
            OpCode::FrameEntryAdmits,
            vec![self.callable],
            vec![],
            &[
                ("s_value", AttrValue::Str(self.owner.clone())),
                ("value", AttrValue::Int(self.shape)),
            ],
            row,
            vec![],
        );
        (entry, [field, raw])
    }
}

/// The miss retains the original physical invocation followed immediately by
/// its own check. Its normal path and the fast check's normal path rejoin
/// after both observations, sharing the original result binding.
pub(super) fn ordinary_call(
    caller: &mut TirFunction,
    miss: BlockId,
    checked_hit: BlockId,
    mut call: TirOp,
) {
    let tail = caller.fresh_block();
    let joined = call.results[0];
    let hot = caller.fresh_value();
    let cold = caller.fresh_value();
    let block = caller
        .blocks
        .get_mut(&checked_hit)
        .expect("checked continuation");
    let ty = block.args[0].ty.clone();
    caller.value_types.insert(hot, ty.clone());
    caller.value_types.insert(cold, ty.clone());
    block.args[0].id = hot;
    let rest = block.ops.split_off(1);
    let check = block.ops.first_mut().expect("validated caller Origin");
    // Ordinary placement runs after module inlining on some targets.
    if super::super::async_work_poll::is_call_return_poll(&call) {
        check.mark_async_work_poll();
    }
    let mut original_check = check.clone();
    for value in &mut original_check.operands {
        if *value == joined {
            *value = cold;
        }
    }
    for value in &mut check.operands {
        if *value == joined {
            *value = hot;
        }
    }
    let term = std::mem::replace(
        &mut block.terminator,
        Terminator::Branch {
            target: tail,
            args: vec![hot],
        },
    );
    call.results[0] = cold;
    caller.blocks.insert(
        miss,
        TirBlock {
            id: miss,
            args: vec![],
            ops: vec![call, original_check],
            terminator: Terminator::Branch {
                target: tail,
                args: vec![cold],
            },
        },
    );
    caller.blocks.insert(
        tail,
        TirBlock {
            id: tail,
            args: vec![TirValue { id: joined, ty }],
            ops: rest,
            terminator: term,
        },
    );
    caller.move_loop_terminator(checked_hit, tail);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::passes::inliner::{activation::Activations, call_sites::CallSite, splice};

    fn lift(params: &[&str], ops: serde_json::Value) -> TirFunction {
        let ir = serde_json::from_value(serde_json::json!({
            "name":"fixture","return_abi":"value","execution_context":"local",
            "params":params,"param_types":vec!["Any";params.len()],"ops":ops
        }))
        .unwrap();
        crate::tir::lower_from_simple::lower_to_tir(&ir)
    }

    #[test]
    fn guarded_read_preserves_the_physical_miss_and_post_read_origin() {
        let mut leaf = lift(
            &["owner", "i", "j"],
            serde_json::json!([
                {"kind":"trace_enter_slot","s_value":"fixture","value":3},
                {"kind":"check_exception","exception_edge":"origin","value":8},
                {"kind":"frame_home_store","value":0,"args":["owner"],"out":"r"},
                {"kind":"frame_home_store","value":1,"args":["i"],"out":"x"},
                {"kind":"frame_home_store","value":2,"args":["j"],"out":"y"},
                {"kind":"frame_set_location","value":7},
                {"kind":"load","value":8,"args":["r"],"out":"field"},
                {"kind":"check_exception","exception_edge":"origin","value":8},
                {"kind":"frame_set_location","value":6},
                {"kind":"call","s_value":molt_ir::runtime_fn_generated::RuntimeFn::Buffer2dGet.symbol(),"args":["field","x","y"],"out":"value"},
                {"kind":"check_exception","exception_edge":"origin","value":8},
                {"kind":"frame_set_location","value":4},{"kind":"trace_exit"},
                {"kind":"ret","args":["value"]},
                {"kind":"label","value":8},{"kind":"trace_exit"},{"kind":"ret_void","unwind":true}
            ]),
        );
        let class = TirType::Class(molt_ir::ClassId(13));
        leaf.exact_param_types = vec![class.clone(), TirType::I64, TirType::I64];
        leaf.param_types[0] = class.clone();
        let parameter = &mut leaf.blocks.get_mut(&leaf.entry_block).unwrap().args[0];
        parameter.ty = class.clone();
        leaf.value_types.insert(parameter.id, class);

        let mut caller = lift(
            &["function", "r", "i", "j"],
            serde_json::json!([
                {"kind":"trace_enter_slot","s_value":"caller","value":0},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"call","s_value":"fixture","args":["function","r","i","j"],"out":"value"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"trace_exit"},{"kind":"ret","args":["value"]},
                {"kind":"label","value":9},{"kind":"trace_exit"},{"kind":"ret_void","unwind":true}
            ]),
        );
        let (block, index) = caller
            .blocks
            .iter()
            .find_map(|(&id, b)| {
                b.ops
                    .iter()
                    .position(|op| op.opcode == OpCode::Call)
                    .map(|i| (id, i))
            })
            .unwrap();
        let call = &mut caller.blocks.get_mut(&block).unwrap().ops[index];
        call.origin = Some(molt_ir::OpKind::CallGuarded);
        call.set_proven_target(true);
        let id = molt_ir::FunctionId(1);
        call.callee = Some(id);
        call.attrs.remove("s_value");
        let site = CallSite {
            block,
            op_index: index,
            callee: id,
        };
        let mut activations = Activations::default();
        activations.enter_caller([&site]);
        let (_, preserve) = splice::splice(&mut caller, &leaf, &site, &mut activations)
            .expect("fixed-field packed hit");
        assert!(preserve);
        let code = || caller.blocks.values().flat_map(|b| &b.ops);
        assert_eq!(
            code()
                .filter(|op| op.opcode == OpCode::FrameEntryAdmits)
                .count(),
            1
        );
        assert_eq!(
            code()
                .filter(|op| op.opcode == OpCode::ProbeInlineField)
                .count(),
            1
        );
        assert_eq!(
            code()
                .filter(|op| op.opcode == OpCode::TryBuffer2dLoadI64)
                .count(),
            1
        );
        assert!(code().any(|op| op.opcode == OpCode::AsyncWorkPending));
        let borrowed = code()
            .find(|op| op.opcode == OpCode::ProbeInlineField)
            .unwrap()
            .results[0];
        let credits: Vec<_> = caller
            .blocks
            .values()
            .filter_map(|block| {
                let alias = block.ops.iter().find(|op| {
                    op.origin == Some(molt_ir::OpKind::BindingAlias) && op.operands == [borrowed]
                })?;
                Some((block, alias.results[0]))
            })
            .collect();
        assert_eq!(credits.len(), 1);
        let (service, held) = credits[0];
        assert!(caller.blocks.values().any(|b| {
            matches!(b.terminator, Terminator::CondBranch { then_block, else_block, .. }
                if then_block == service.id && matches!(&caller.blocks[&else_block].ops[..],
                    [plain] if plain.opcode == OpCode::CheckException && !plain.is_async_work_poll()))
                && b.ops.last().is_some_and(|op| op.opcode == OpCode::AsyncWorkPending)
        }), "only the pending arm acquires the field");
        let (index, poll) = crate::tir::passes::spliced_frame::poll_service(
            &service.ops,
            crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin,
        )
        .expect("balanced field custody retains the paired Origin");
        assert_eq!(service.ops[index + 1].operands, [held]);
        let labels = crate::tir::dominators::exception_label_to_block(&caller);
        let target = crate::tir::dominators::exception_transfer_target(poll, &labels).unwrap();
        let cleanup = &caller.blocks[&target].ops;
        let capture = cleanup
            .iter()
            .position(|op| {
                op.runtime == Some(molt_ir::runtime_fn_generated::RuntimeFn::SplicedFrameOrigin)
            })
            .unwrap();
        let release = cleanup
            .iter()
            .position(|op| op.opcode == OpCode::DelBoundary && op.operands == [held])
            .unwrap();
        assert!(capture < release);
        assert!(cleanup[capture + 1..release].iter().any(
            |op| op.runtime == Some(molt_ir::runtime_fn_generated::RuntimeFn::SplicedFrameLink)
        ));
        let misses: Vec<_> = caller
            .blocks
            .values()
            .filter(|b| b.ops.iter().any(|op| op.callee == Some(id)))
            .collect();
        assert_eq!(misses.len(), 1);
        assert!(
            matches!(&misses[0].ops[..], [original,check] if original.callee==Some(id) && check.is_async_work_poll())
        );
        assert!(code().all(|op| op.runtime != Some(molt_ir::runtime_fn_generated::RuntimeFn::Buffer2dGet)));
        crate::tir::verify::verify_function(&caller).expect("guarded read CFG");
    }
}
