//! A successfully constructed builtin exception with an omitted cause needs
//! one owned result credit. Both fallible producers can return Python None.
use super::*;
use crate::tir::dominators::{build_pred_map, exception_label_to_block, exception_successors};
use crate::tir::ops::ARGUMENT_CUSTODY_ATTR;
use crate::tir::passes::guarded_value::select_guarded_result;

fn plain(op: &TirOp) -> bool {
    !op.is_async_work_poll()
        && op.callee.is_none()
        && !op.attrs.contains_key("runtime_symbol")
        && !op.attrs.contains_key(ARGUMENT_CUSTODY_ATTR)
}

fn synchronous_check(op: &TirOp) -> bool {
    op.opcode == OpCode::CheckException
        && op.results.is_empty()
        && plain(op)
        && label_of(op).is_some()
        && op.exception_edge_kind().is_some()
}

fn constructed_exception(block: &TirBlock) -> Option<ValueId> {
    let [.., constructor, check] = block.ops.as_slice() else {
        return None;
    };
    let arity = match constructor.origin? {
        OpKind::ExceptionNewBuiltinEmpty => 0,
        OpKind::ExceptionNewBuiltin | OpKind::ExceptionNewBuiltinOne => 1,
        _ => return None,
    };
    let [exception] = constructor.results[..] else {
        return None;
    };
    (constructor.opcode == OpCode::Copy
        && constructor.operands.len() == arity
        && constructor.runtime.is_none()
        && plain(constructor)
        && label_of(constructor).is_some()
        && synchronous_check(check))
    .then_some(exception)
}

fn omitted_preparation(block: &TirBlock) -> Option<(usize, ValueId, ValueId)> {
    let (missing, tail) = block.ops.split_first()?;
    let [cause] = missing.results[..] else {
        return None;
    };
    if !is_copy_of(missing, OpKind::Missing)
        || !missing.operands.is_empty()
        || missing.runtime.is_some()
        || !plain(missing)
    {
        return None;
    }
    let index = if tail.first()?.opcode == OpCode::FrameSetLocation && plain(&tail[0]) {
        2
    } else {
        1
    };
    let call = block.ops.get(index)?;
    let [exception, supplied] = call.operands[..] else {
        return None;
    };
    (call.opcode == OpCode::Call
        && call.runtime == Some(RuntimeFn::ExceptionPrepareRaise)
        && call.origin.is_none()
        && plain(call)
        && supplied == cause
        && call.results.len() == 1
        && synchronous_check(block.ops.get(index + 1)?))
    .then_some((index, exception, cause))
}

pub(super) fn specialize(func: &mut TirFunction, stats: &mut PassStats) {
    let mut sites = Vec::new();
    for from in func.blocks.values() {
        let Terminator::Branch { target, .. } = &from.terminator else {
            continue;
        };
        let Some(exception) = constructed_exception(from) else {
            continue;
        };
        let Some((index, supplied, cause)) = func.blocks.get(target).and_then(omitted_preparation)
        else {
            continue;
        };
        if supplied == exception && *target != func.entry_block {
            sites.push((from.id, *target, index, exception, cause));
        }
    }
    if sites.is_empty() {
        return;
    }
    let preds = build_pred_map(func);
    let labels = exception_label_to_block(func);
    sites.retain(|(from, target, ..)| {
        preds.get(target).is_some_and(|p| p.as_slice() == [*from])
            && !exception_successors(&func.blocks[from], &labels).contains(target)
    });
    sites.sort_by_key(|(_, block, ..)| block.0);
    for (_, block, index, exception, cause) in sites {
        let call = func.blocks[&block].ops[index].clone();
        let result = call.results[0];
        let ty = func
            .value_types
            .get(&result)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let none = func.fresh_value();
        func.value_types.insert(none, TirType::None);
        let mut constant = TirOp::new(OpCode::ConstNone, vec![], vec![none]);
        constant.inherit_source_from(&call);
        let guards = [exception, cause].map(|value| {
            let mut guard = TirOp::new(OpCode::IsNot, vec![value, none], vec![func.fresh_value()]);
            guard.inherit_source_from(&call);
            guard
        });
        // BindingAlias acquires the same independent reference as successful
        // normalize_raise_operand. Missing failure must run the original helper,
        // which distinguishes explicit None and may retry its singleton lookup.
        let mut alias = TirOp::new(OpCode::Copy, vec![exception], vec![result])
            .with_origin(OpKind::BindingAlias);
        alias.inherit_source_from(&call);
        let ops = &mut func.blocks.get_mut(&block).unwrap().ops;
        ops[index] = alias;
        ops.insert(index, constant);
        select_guarded_result(func, block, index + 1, guards, call, ty);
        stats.ops_added += 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> TirFunction {
        let program = molt_ir::ir::SimpleIR::from_json_str(r#"{"functions":[{
            "name":"raise_value", "params":["arg"], "return_abi":"value", "ops":[
            {"kind":"try_start","value":5},
            {"kind":"exception_new_builtin_one","args":["arg"],"out":"e","s_value":"ValueError","value":32},
            {"kind":"check_exception","exception_edge":"unwind","value":5},
            {"kind":"missing","out":"cause"},
            {"kind":"frame_set_location","value":14},
            {"kind":"call","s_value":"molt_exception_prepare_raise","args":["e","cause"],"out":"prepared"},
            {"kind":"check_exception","exception_edge":"origin","value":5},
            {"kind":"try_end","value":5},
            {"kind":"ret","var":"prepared"},
            {"kind":"label","value":5},{"kind":"try_end","value":5},
            {"kind":"exception_last_pending","out":"pending"},
            {"kind":"exception_clear"},{"kind":"ret","var":"pending"}
        ]}]}"#).unwrap();
        crate::tir::lower_from_simple::lower_to_tir_for_target(
            &program.functions[0],
            &crate::tir::target_info::TargetInfo::llvm_release_fast(),
        )
    }

    #[test]
    fn checked_builtin_omitted_cause_preserves_both_refusals_and_result_credit() {
        let base = source();
        let ctor = base
            .blocks
            .values()
            .find(|b| constructed_exception(b).is_some())
            .expect("checked constructor")
            .id;
        let Terminator::Branch { target, .. } = base.blocks[&ctor].terminator else {
            panic!("normal continuation")
        };
        let (index, exception, cause) =
            omitted_preparation(&base.blocks[&target]).expect("omitted raise");
        let original = &base.blocks[&target].ops[index];
        let result = original.results[0];
        let mut f = base.clone();
        assert_eq!(super::super::run(&mut f).ops_added, 4);
        assert_eq!(
            serde_json::to_value(&f.blocks[&ctor]).unwrap(),
            serde_json::to_value(&base.blocks[&ctor]).unwrap()
        );
        let mut block = target;
        let mut fallback = None;
        for operand in [exception, cause] {
            let guard = f.blocks[&block].ops.last().unwrap();
            assert_eq!(guard.opcode, OpCode::IsNot);
            assert_eq!(guard.operands[0], operand);
            assert_eq!(f.value_types[&guard.operands[1]], TirType::None);
            let Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                ..
            } = f.blocks[&block].terminator
            else {
                panic!("guard")
            };
            assert_eq!(cond, guard.results[0]);
            assert_eq!(*fallback.get_or_insert(else_block), else_block);
            block = then_block;
        }
        let fast = &f.blocks[&block];
        let cold = &f.blocks[&fallback.unwrap()];
        let owned = &fast.ops[0];
        assert_eq!(owned.origin, Some(OpKind::BindingAlias));
        assert!(
            crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(
                owned.origin.unwrap()
            )
        );
        assert_eq!(owned.operands, [exception]);
        let mut slow = cold.ops[0].clone();
        slow.results.clone_from(&original.results);
        assert_eq!(
            serde_json::to_value(&slow).unwrap(),
            serde_json::to_value(original).unwrap()
        );
        let Terminator::Branch { target: join, args } = &fast.terminator else {
            panic!("fast join")
        };
        assert_eq!(args, &owned.results);
        let Terminator::Branch {
            target: cold_join,
            args: cold_result,
        } = &cold.terminator
        else {
            panic!("generic join")
        };
        assert_eq!(cold_join, join);
        assert_eq!(cold_result, &cold.ops[0].results);
        assert_eq!(f.blocks[join].args[0].id, result);
        assert_eq!(
            serde_json::to_value(&f.blocks[join].ops).unwrap(),
            serde_json::to_value(&base.blocks[&target].ops[index + 1..]).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&f.blocks[join].terminator).unwrap(),
            serde_json::to_value(&base.blocks[&target].terminator).unwrap()
        );
        assert_eq!(super::super::run(&mut f).ops_added, 0);
        molt_ir::tir::verify::verify_function(&f).unwrap();
        crate::tir::exception_regions::verify_exception_regions(&f).unwrap();
        for refusal in 0..5 {
            let mut f = base.clone();
            match refusal {
                0 => {
                    f.blocks.get_mut(&target).unwrap().ops[0] =
                        TirOp::new(OpCode::ConstNone, vec![], vec![cause])
                }
                1 => {
                    f.blocks
                        .get_mut(&ctor)
                        .unwrap()
                        .ops
                        .iter_mut()
                        .find(|op| op.origin == Some(OpKind::ExceptionNewBuiltinOne))
                        .unwrap()
                        .origin = Some(OpKind::ExceptionNew)
                }
                2 => {
                    f.blocks
                        .get_mut(&ctor)
                        .unwrap()
                        .ops
                        .last_mut()
                        .unwrap()
                        .mark_async_work_poll();
                }
                3 => f.blocks.get_mut(&target).unwrap().ops[index].set_argument_custody(&[
                    molt_ir::ir::ParameterCustody::Transferred,
                    molt_ir::ir::ParameterCustody::Borrowed,
                ]),
                _ => {
                    f.label_id_map.insert(target.0, 91);
                    f.blocks
                        .get_mut(&ctor)
                        .unwrap()
                        .ops
                        .last_mut()
                        .unwrap()
                        .attrs
                        .insert("value".into(), AttrValue::Int(91));
                }
            }
            assert_eq!(super::super::run(&mut f).ops_added, 0, "refusal {refusal}");
        }
    }
}
