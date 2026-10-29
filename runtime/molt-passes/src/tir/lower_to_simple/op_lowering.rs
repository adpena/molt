use crate::ir::OpIR;
use crate::tir::op_kinds_generated::{
    SimpleIrVarFieldRole, simpleir_first_trailing_result_arg_table, simpleir_var_field_role_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::simple_value_names::{local_slot_var, value_var};

use super::op_utils::{
    attr_bool, attr_bytes, attr_float, attr_int, attr_str, binary_op, operand_args,
    result_or_stream_out, unary_op,
};

// ---------------------------------------------------------------------------
// Op lowering
// ---------------------------------------------------------------------------

fn local_destination(op: &TirOp) -> String {
    let source = attr_str(&op.attrs, "_var")
        .filter(|name| !name.is_empty())
        .expect("named-local mutation requires an explicit _var destination");
    local_slot_var(&source)
}

fn assign_multi_result_transport(simple: &mut OpIR, op: &TirOp) {
    let names = op.results.iter().map(|value| value_var(*value));
    if simpleir_var_field_role_table(simple.kind) == SimpleIrVarFieldRole::Result {
        let mut names = names;
        simple.var = names.next();
        simple.out = names.next();
        debug_assert!(
            names.next().is_none(),
            "var/out transport holds exactly two results"
        );
    } else if let Some(first_result) = simpleir_first_trailing_result_arg_table(simple.kind) {
        let args = simple.args.get_or_insert_with(Vec::new);
        debug_assert_eq!(args.len(), first_result);
        args.extend(names);
    } else {
        debug_assert!(op.results.len() <= 1);
        simple.out = names.into_iter().next();
    }
}

/// Convert a single TirOp to an OpIR. Returns None for ops that are
/// dialect-internal and have no SimpleIR equivalent (yet).
pub(super) fn lower_op_many(op: &TirOp) -> Vec<OpIR> {
    if op.opcode == OpCode::Copy && matches!(op.origin, Some(molt_ir::OpKind::StoreVar)) {
        assert_eq!(op.operands.len(), 1, "store_var requires one SSA source");
        assert!(
            op.results.len() <= 1,
            "store_var permits at most one alias result"
        );
        let mut lowered = vec![OpIR {
            kind: molt_ir::OpKind::StoreVar,
            args: Some(operand_args(op)),
            var: Some(local_destination(op)),
            ..OpIR::new(molt_ir::OpKind::StoreVar)
        }];
        if let Some(result) = op.results.first() {
            lowered.push(OpIR {
                kind: molt_ir::OpKind::CopyVar,
                var: Some(value_var(op.operands[0])),
                out: Some(value_var(*result)),
                ..OpIR::new(molt_ir::OpKind::CopyVar)
            });
        }
        return lowered;
    }
    lower_op(op).into_iter().collect()
}

fn lower_op(op: &TirOp) -> Option<OpIR> {
    // Map result (if any) to output variable.
    let out_var = op.results.first().map(|v| value_var(*v));

    match op.opcode {
        // Constants.
        OpCode::ModuleSlot => Some(OpIR {
            kind: molt_ir::OpKind::ModuleSlot,
            s_value: attr_str(&op.attrs, "s_value"),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ModuleSlot)
        }),
        OpCode::ConstInt => Some(OpIR {
            kind: molt_ir::OpKind::Const,
            value: attr_int(&op.attrs, "value"),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::Const)
        }),
        OpCode::ConstFloat => Some(OpIR {
            kind: molt_ir::OpKind::ConstFloat,
            f_value: attr_float(&op.attrs, "f_value").or_else(|| attr_float(&op.attrs, "value")),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ConstFloat)
        }),
        OpCode::ConstStr | OpCode::ConstBytes | OpCode::ConstBigInt => Some(
            molt_ir::literal_payload::lower_tir_literal(op, out_var)
                .unwrap_or_else(|error| panic!("{error}")),
        ),
        OpCode::ConstBool => Some(OpIR {
            kind: molt_ir::OpKind::ConstBool,
            // Both the SSA lift and SCCP store ConstBool values as
            // AttrValue::Bool.  Legacy AttrValue::Int is handled for
            // backward compatibility with cached TIR artifacts.
            value: Some(match op.attrs.get("value") {
                Some(AttrValue::Bool(b)) => u8::from(*b) as i64,
                Some(AttrValue::Int(i)) => u8::from(*i != 0) as i64,
                _ => 0,
            }),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ConstBool)
        }),
        OpCode::ConstNone => Some(OpIR {
            kind: molt_ir::OpKind::ConstNone,
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ConstNone)
        }),

        // Arithmetic.
        OpCode::Add => Some(binary_op(molt_ir::OpKind::Add, op, out_var)),
        OpCode::Sub => Some(binary_op(molt_ir::OpKind::Sub, op, out_var)),
        OpCode::Mul => Some(binary_op(molt_ir::OpKind::Mul, op, out_var)),
        OpCode::InplaceAdd => Some(binary_op(molt_ir::OpKind::InplaceAdd, op, out_var)),
        OpCode::InplaceSub => Some(binary_op(molt_ir::OpKind::InplaceSub, op, out_var)),
        OpCode::InplaceMul => Some(binary_op(molt_ir::OpKind::InplaceMul, op, out_var)),
        OpCode::Div => Some(binary_op(molt_ir::OpKind::Div, op, out_var)),
        // Canonical wire spelling is `floordiv` (the frontend emission); see the
        // op-kind registry (op_kinds.toml). Emitting the canonical here makes the
        // SimpleIR↔TIR round-trip idempotent (`kind_to_opcode(molt_ir::OpKind::Floordiv)` ->
        // OpCode::FloorDiv) and routes through the same `"floordiv"` dispatch arm
        // every backend already has — closing the floordiv/floor_div schism.
        OpCode::FloorDiv => Some(binary_op(molt_ir::OpKind::Floordiv, op, out_var)),
        OpCode::Mod => Some(binary_op(molt_ir::OpKind::Mod, op, out_var)),
        OpCode::Pow => Some(binary_op(molt_ir::OpKind::Pow, op, out_var)),
        OpCode::Neg => Some(unary_op(molt_ir::OpKind::Neg, op, out_var)),
        OpCode::Pos => Some(unary_op(molt_ir::OpKind::Pos, op, out_var)),

        // Checked arithmetic: two output vars, mirroring IterNextUnboxed —
        // `var` = results[0] (wrapping sum), `out` = results[1] (overflow
        // flag). The module phase re-lifts every function from SimpleIR, so
        // this op MUST round-trip (ssa.rs maps "checked_add" back to
        // OpCode::CheckedAdd with the same var/out → results order); without
        // the pair it would fall to the Copy fallback and silently vanish.
        OpCode::CheckedAdd => {
            let mut simple = OpIR {
                kind: molt_ir::OpKind::CheckedAdd,
                args: Some(operand_args(op)),
                ..OpIR::new(molt_ir::OpKind::CheckedAdd)
            };
            assign_multi_result_transport(&mut simple, op);
            Some(simple)
        }
        // CheckedMul mirrors CheckedAdd exactly: `var` = results[0] (wrapping
        // product), `out` = results[1] (overflow flag). Same round-trip
        // requirement — ssa.rs maps "checked_mul" back to OpCode::CheckedMul.
        OpCode::CheckedMul => {
            let mut simple = OpIR {
                kind: molt_ir::OpKind::CheckedMul,
                args: Some(operand_args(op)),
                ..OpIR::new(molt_ir::OpKind::CheckedMul)
            };
            assign_multi_result_transport(&mut simple, op);
            Some(simple)
        }

        // Comparison.
        OpCode::Eq => Some(binary_op(molt_ir::OpKind::Eq, op, out_var)),
        OpCode::Ne => Some(binary_op(molt_ir::OpKind::Ne, op, out_var)),
        OpCode::Lt => Some(binary_op(molt_ir::OpKind::Lt, op, out_var)),
        OpCode::Le => Some(binary_op(molt_ir::OpKind::Le, op, out_var)),
        OpCode::Gt => Some(binary_op(molt_ir::OpKind::Gt, op, out_var)),
        OpCode::Ge => Some(binary_op(molt_ir::OpKind::Ge, op, out_var)),
        OpCode::Is => Some(binary_op(molt_ir::OpKind::Is, op, out_var)),
        OpCode::IsNot => Some(binary_op(molt_ir::OpKind::IsNot, op, out_var)),
        OpCode::In => Some(binary_op(molt_ir::OpKind::In, op, out_var)),
        OpCode::NotIn => Some(binary_op(molt_ir::OpKind::NotIn, op, out_var)),

        // Bitwise.
        OpCode::BitAnd => Some(binary_op(molt_ir::OpKind::BitAnd, op, out_var)),
        OpCode::BitOr => Some(binary_op(molt_ir::OpKind::BitOr, op, out_var)),
        OpCode::BitXor => Some(binary_op(molt_ir::OpKind::BitXor, op, out_var)),
        OpCode::BitNot => Some(unary_op(molt_ir::OpKind::BitNot, op, out_var)),
        OpCode::Shl => Some(binary_op(molt_ir::OpKind::Lshift, op, out_var)),
        OpCode::Shr => Some(binary_op(molt_ir::OpKind::Rshift, op, out_var)),

        // Boolean.
        OpCode::And => Some(binary_op(molt_ir::OpKind::And, op, out_var)),
        OpCode::Or => Some(binary_op(molt_ir::OpKind::Or, op, out_var)),
        OpCode::Not => Some(unary_op(molt_ir::OpKind::Not, op, out_var)),
        OpCode::Bool => Some(unary_op(molt_ir::OpKind::Bool, op, out_var)),

        // Memory.
        OpCode::LoadAttr => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::GetAttr);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out: out_var,
                ..OpIR::new(kind)
            })
        }
        OpCode::LoadMethod | OpCode::LoadSuperMethod => {
            // `var` carries the callable and `out` the self-or-null word.
            let mut simple = OpIR {
                kind: if op.opcode == OpCode::LoadMethod {
                    molt_ir::OpKind::LoadMethod
                } else {
                    molt_ir::OpKind::LoadSuperMethod
                },
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name"),
                ..OpIR::new(if op.opcode == OpCode::LoadMethod {
                    molt_ir::OpKind::LoadMethod
                } else {
                    molt_ir::OpKind::LoadSuperMethod
                })
            };
            assign_multi_result_transport(&mut simple, op);
            Some(simple)
        }
        OpCode::StoreAttr => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::SetAttr);
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out,
                ..OpIR::new(kind)
            })
        }
        OpCode::Index => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::Index);
            let mut opir = binary_op(kind, op, out_var);
            // Restore semantic container_type from the preserved attr so
            // backend dispatch can use typed container facts.
            opir.container_type = attr_str(&op.attrs, "container_type");
            // Propagate BCE proof so codegen can skip bounds checks.
            opir.bce_safe = attr_bool(&op.attrs, "bce_safe");
            Some(opir)
        }
        OpCode::StoreIndex => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::StoreIndex);
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out,
                container_type: attr_str(&op.attrs, "container_type"),
                // Propagate BCE proof so codegen can skip bounds checks.
                bce_safe: attr_bool(&op.attrs, "bce_safe"),
                ..OpIR::new(kind)
            })
        }
        OpCode::DeleteVar => Some(OpIR {
            kind: molt_ir::OpKind::DeleteVar,
            args: Some(operand_args(op)),
            var: Some(local_destination(op)),
            ..OpIR::new(molt_ir::OpKind::DeleteVar)
        }),

        // Direct calls carry the target function name in s_value. Dynamic call
        // forms may additionally carry their diagnostic call-site ID in value.
        // Recover the original SimpleIR kind (call_func, call_indirect, etc.)
        // if it was preserved during the SSA lift.
        OpCode::Call => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::Call);
            Some(OpIR {
                kind,
                s_value: attr_str(&op.attrs, "s_value"),
                args: Some(operand_args(op)),
                out: out_var,
                value: attr_int(&op.attrs, "value"),
                // Preserve the finalizer fact across the round-trip (the
                // GENERIC class-instantiation `call_bind` carries it exactly
                // like `object_new_bound` — #58): a re-lift must still seed
                // `finalizer_alloc_roots` from this result, or the ownership
                // lattice goes blind after the first SimpleIR round-trip and
                // the deferred Return-boundary release silently degrades to
                // SSA-last-use.
                defines_del: attr_bool(&op.attrs, "defines_del"),
                bound_local: attr_bool(&op.attrs, "bound_local"),
                native_callable_export: attr_str(&op.attrs, "native_callable_export"),
                native_callable_binding: attr_str(&op.attrs, "native_callable_binding"),
                native_callable_symbol: attr_str(&op.attrs, "native_callable_symbol"),
                native_callable_abi: attr_str(&op.attrs, "native_callable_abi"),
                ..OpIR::new(kind)
            })
        }
        OpCode::CallMethod => Some(OpIR {
            kind: molt_ir::OpKind::CallMethod,
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method"),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::CallMethod)
        }),
        OpCode::CallLoadedMethod => Some(OpIR {
            kind: molt_ir::OpKind::CallLoadedMethod,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::CallLoadedMethod)
        }),
        OpCode::CallargsPushMethodReceiver => Some(OpIR {
            kind: molt_ir::OpKind::CallargsPushMethodReceiver,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::CallargsPushMethodReceiver)
        }),
        OpCode::CallBuiltin => {
            let call = op
                .builtin_call()
                .expect("malformed CallBuiltin identity or argument contract");
            Some(OpIR {
                kind: call.wire_kind,
                args: Some(operand_args(op)),
                s_value: call.named_target().map(str::to_string),
                out: out_var,
                ..OpIR::new(call.wire_kind)
            })
        }
        OpCode::OrdAt => Some(OpIR {
            kind: molt_ir::OpKind::OrdAt,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::OrdAt)
        }),

        // Keep the generated representation operation and its effects through
        // re-emission. Native carrier planning can retain raw i64 inputs, whose
        // boxing may allocate even when the result binding is discarded.
        OpCode::BoxVal => Some(unary_op(molt_ir::OpKind::Box, op, out_var)),
        OpCode::UnboxVal => Some(unary_op(molt_ir::OpKind::Unbox, op, out_var)),
        OpCode::TypeGuard => {
            if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: molt_ir::OpKind::CopyVar,
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::new(molt_ir::OpKind::CopyVar)
                })
            } else {
                None
            }
        }

        // Copy: either a genuine copy_var or a passthrough for an unknown op
        // whose original kind was preserved in attrs.
        OpCode::Copy => {
            if let Some(original_kind) = op.origin.filter(|kind| {
                !matches!(
                    kind,
                    molt_ir::OpKind::Copy | molt_ir::OpKind::CopyVar | molt_ir::OpKind::LoadVar
                )
            }) {
                // Reconstruct field roles, not their pre-SSA spellings. Lift
                // records the exact operand only when var resolved to an SSA
                // read; sentinel/literal metadata must not consume an argument.
                // A definition is independent mutable storage.
                let original_var = attr_str(&op.attrs, "_var");
                let var_role = simpleir_var_field_role_table(original_kind);
                let var_operand = match op.attrs.get("_simple_var_operand") {
                    Some(AttrValue::Int(index)) => {
                        assert!(
                            var_role == SimpleIrVarFieldRole::Read && original_var.is_some(),
                            "passthrough var operand requires an authored read field"
                        );
                        let index = usize::try_from(*index)
                            .expect("passthrough var operand index must be nonnegative");
                        assert!(
                            index < op.operands.len(),
                            "passthrough var operand index is out of bounds"
                        );
                        Some(index)
                    }
                    None => None,
                    _ => panic!("passthrough var operand index must be an integer"),
                };
                let var = match (var_role, original_var) {
                    (SimpleIrVarFieldRole::Definition, Some(source)) => {
                        Some(local_slot_var(&source))
                    }
                    (SimpleIrVarFieldRole::Read, Some(source)) => Some(
                        var_operand
                            .map(|index| value_var(op.operands[index]))
                            .unwrap_or(source),
                    ),
                    (_, original) => original,
                };
                let args: Vec<_> = op
                    .operands
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| Some(*index) != var_operand)
                    .map(|(_, value)| value_var(*value))
                    .collect();
                Some(OpIR {
                    kind: original_kind,
                    args: (!args.is_empty()).then_some(args),
                    out: out_var,
                    value: attr_int(&op.attrs, "value"),
                    f_value: attr_float(&op.attrs, "f_value"),
                    s_value: attr_str(&op.attrs, "s_value"),
                    bytes: attr_bytes(&op.attrs, "bytes"),
                    var,
                    task_kind: attr_str(&op.attrs, "task_kind"),
                    task_closure_size: attr_int(&op.attrs, "task_closure_size"),
                    container_type: attr_str(&op.attrs, "container_type"),
                    // Named-local fact (#58) — container literals (`list_new`/
                    // `tuple_new`) ride this passthrough; losing the attr here
                    // silently degrades the scope-boundary deferral.
                    bound_local: attr_bool(&op.attrs, "bound_local"),
                    ..OpIR::new(original_kind)
                })
            } else if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: molt_ir::OpKind::CopyVar,
                    // The SSA operand is the value authority. `_var` may be
                    // the original local spelling, or unrelated metadata when
                    // the authored load/copy supplied an explicit args[0]. It
                    // must never replace the resolved (and possibly rewritten)
                    // SSA source during re-emission.
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::new(molt_ir::OpKind::CopyVar)
                })
            } else {
                None
            }
        }

        // Build containers.
        OpCode::BuildList => Some(OpIR {
            kind: molt_ir::OpKind::BuildList,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::BuildList)
        }),
        OpCode::BuildDict => Some(OpIR {
            kind: molt_ir::OpKind::BuildDict,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::BuildDict)
        }),
        OpCode::BuildTuple => Some(OpIR {
            kind: molt_ir::OpKind::BuildTuple,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::BuildTuple)
        }),
        OpCode::BuildSet => Some(OpIR {
            kind: molt_ir::OpKind::BuildSet,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::BuildSet)
        }),
        OpCode::BuildSlice => Some(OpIR {
            kind: molt_ir::OpKind::BuildSlice,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::BuildSlice)
        }),

        // Iteration.
        OpCode::GetIter => Some(unary_op(molt_ir::OpKind::GetIter, op, out_var)),
        OpCode::IterNext => Some(unary_op(molt_ir::OpKind::IterNext, op, out_var)),
        OpCode::IterNextUnboxed => {
            let mut simple = OpIR {
                kind: molt_ir::OpKind::IterNextUnboxed,
                args: Some(operand_args(op)),
                ..OpIR::new(molt_ir::OpKind::IterNextUnboxed)
            };
            assign_multi_result_transport(&mut simple, op);
            Some(simple)
        }
        OpCode::UnpackSequence => {
            let mut simple = OpIR {
                kind: molt_ir::OpKind::UnpackSequence,
                args: Some(operand_args(op)),
                value: attr_int(&op.attrs, "value"),
                ..OpIR::new(molt_ir::OpKind::UnpackSequence)
            };
            assign_multi_result_transport(&mut simple, op);
            Some(simple)
        }
        OpCode::ForIter => Some(OpIR {
            kind: molt_ir::OpKind::ForIter,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ForIter)
        }),

        // Generator.
        OpCode::AllocTask => Some(OpIR {
            kind: molt_ir::OpKind::AllocTask,
            args: Some(operand_args(op)),
            out: out_var,
            s_value: attr_str(&op.attrs, "s_value"),
            value: attr_int(&op.attrs, "value"),
            task_kind: attr_str(&op.attrs, "task_kind"),
            ..OpIR::new(molt_ir::OpKind::AllocTask)
        }),
        OpCode::StateSwitch => Some(OpIR {
            kind: molt_ir::OpKind::StateSwitch,
            ..OpIR::new(molt_ir::OpKind::StateSwitch)
        }),
        OpCode::StateTransition => Some(OpIR {
            kind: molt_ir::OpKind::StateTransition,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::StateTransition)
        }),
        OpCode::StateYield => Some(OpIR {
            kind: molt_ir::OpKind::StateYield,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::StateYield)
        }),
        OpCode::StateSet => Some(OpIR {
            kind: molt_ir::OpKind::StateSet,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::StateSet)
        }),
        OpCode::IsPending => Some(OpIR {
            kind: molt_ir::OpKind::IsPending,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::IsPending)
        }),
        OpCode::TaskWait => Some(OpIR {
            kind: molt_ir::OpKind::TaskWait,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::TaskWait)
        }),
        OpCode::ClosureLoad => Some(OpIR {
            kind: molt_ir::OpKind::ClosureLoad,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::ClosureLoad)
        }),
        OpCode::FrameContextSet => Some(OpIR {
            kind: molt_ir::OpKind::FrameContextSet,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::FrameContextSet)
        }),
        OpCode::ClosureStore => Some(OpIR {
            kind: molt_ir::OpKind::ClosureStore,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::ClosureStore)
        }),
        OpCode::Yield => Some(OpIR {
            kind: molt_ir::OpKind::Yield,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::Yield)
        }),
        OpCode::YieldFrom => Some(OpIR {
            kind: molt_ir::OpKind::YieldFrom,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::YieldFrom)
        }),

        // Exception.
        OpCode::Raise => Some(OpIR {
            kind: molt_ir::OpKind::Raise,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::Raise)
        }),
        OpCode::CheckException => Some(OpIR {
            kind: if op.is_async_work_poll() {
                molt_ir::OpKind::AsyncWorkPoll
            } else {
                molt_ir::OpKind::CheckException
            },
            // Emit with None args (matching the original structured IR format).
            // The Cranelift backend manages live-value state implicitly from
            // the structured control flow context. Emitting the TIR operands
            // (which are all block-argument values captured at exception
            // boundaries) causes the backend to generate incorrect exception
            // handling state with inflated argument lists.
            args: None,
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(if op.is_async_work_poll() {
                molt_ir::OpKind::AsyncWorkPoll
            } else {
                molt_ir::OpKind::CheckException
            })
        }),
        OpCode::ExceptionPending => Some(OpIR {
            // Reads the runtime exception-pending flag as a boolean value.
            // No operands; produces the condition consumed by the
            // `loop_break_if_exception` CondBranch.
            kind: molt_ir::OpKind::ExceptionPending,
            args: None,
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ExceptionPending)
        }),
        OpCode::FunctionDefaultsVersion => Some(OpIR {
            // Reads a function object's `__defaults__`/`__kwdefaults__`
            // mutation version stamp as an inline int.  One operand (the
            // function object); produces the value the defaults-devirt deopt
            // guard compares against 0.  Non-foldable: it observes mutable
            // runtime state (`side_effecting` in the op-kind registry).
            kind: molt_ir::OpKind::FunctionDefaultsVersion,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::FunctionDefaultsVersion)
        }),
        OpCode::TryStart => Some(OpIR {
            kind: molt_ir::OpKind::TryStart,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::TryStart)
        }),
        OpCode::TryEnd => Some(OpIR {
            kind: molt_ir::OpKind::TryEnd,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::TryEnd)
        }),
        OpCode::StateBlockStart => Some(OpIR {
            kind: molt_ir::OpKind::StateBlockStart,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::StateBlockStart)
        }),
        OpCode::StateBlockEnd => Some(OpIR {
            kind: molt_ir::OpKind::StateBlockEnd,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::StateBlockEnd)
        }),

        // Import.
        OpCode::Import => {
            let args = operand_args(op);
            if args.is_empty() {
                Some(OpIR {
                    kind: molt_ir::OpKind::Import,
                    s_value: attr_str(&op.attrs, "module"),
                    out: out_var,
                    ..OpIR::new(molt_ir::OpKind::Import)
                })
            } else {
                Some(OpIR {
                    kind: molt_ir::OpKind::ModuleImport,
                    args: Some(args),
                    out: out_var,
                    ..OpIR::new(molt_ir::OpKind::ModuleImport)
                })
            }
        }
        OpCode::ImportFrom => Some(OpIR {
            kind: molt_ir::OpKind::ImportFrom,
            s_value: attr_str(&op.attrs, "name"),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ImportFrom)
        }),
        OpCode::ModuleCacheGet => Some(OpIR {
            kind: molt_ir::OpKind::ModuleCacheGet,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ModuleCacheGet)
        }),
        OpCode::ModuleCacheSet => Some(OpIR {
            kind: molt_ir::OpKind::ModuleCacheSet,
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ModuleCacheSet)
        }),
        OpCode::ModuleCacheDel => Some(OpIR {
            kind: molt_ir::OpKind::ModuleCacheDel,
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ModuleCacheDel)
        }),
        OpCode::ModuleGetAttr => Some(OpIR {
            kind: molt_ir::OpKind::ModuleGetAttr,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
        }),
        OpCode::ModuleImportFrom => Some(OpIR {
            kind: molt_ir::OpKind::ModuleImportFrom,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ModuleImportFrom)
        }),
        OpCode::ModuleGetGlobal => Some(OpIR {
            kind: molt_ir::OpKind::ModuleGetGlobal,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ModuleGetGlobal)
        }),
        OpCode::ModuleGetName => Some(OpIR {
            kind: molt_ir::OpKind::ModuleGetName,
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::new(molt_ir::OpKind::ModuleGetName)
        }),
        OpCode::ModuleSetAttr => Some(OpIR {
            kind: molt_ir::OpKind::ModuleSetAttr,
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ModuleSetAttr)
        }),
        OpCode::ModuleDelGlobal => Some(OpIR {
            kind: molt_ir::OpKind::ModuleDelGlobal,
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ModuleDelGlobal)
        }),
        OpCode::ModuleDelGlobalIfPresent => Some(OpIR {
            kind: molt_ir::OpKind::ModuleDelGlobalIfPresent,
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ModuleDelGlobalIfPresent)
        }),
        OpCode::WarnStderr => Some(OpIR {
            kind: molt_ir::OpKind::WarnStderr,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::WarnStderr)
        }),

        // Refcount and allocation — preserve for native backend.
        OpCode::IncRef => Some(OpIR {
            kind: molt_ir::OpKind::IncRef,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::IncRef)
        }),
        OpCode::DecRef => Some(OpIR {
            kind: molt_ir::OpKind::DecRef,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::DecRef)
        }),
        // Python lifetime boundary (`del x`, #58). Normally the drop phase
        // normalizes this away before back-conversion on drop-activated
        // targets; on the dormant-native lane it survives so the native
        // preanalysis can pin the local's last_use to the del statement
        // (codegen's default arm ignores the kind).
        OpCode::DelBoundary => Some(OpIR {
            kind: molt_ir::OpKind::DelBoundary,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::DelBoundary)
        }),
        OpCode::Alloc => Some(OpIR {
            kind: molt_ir::OpKind::Alloc,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            s_value: attr_str(&op.attrs, "s_value"),
            ..OpIR::new(molt_ir::OpKind::Alloc)
        }),
        OpCode::StackAlloc => Some(OpIR {
            kind: molt_ir::OpKind::StackAlloc,
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::new(molt_ir::OpKind::StackAlloc)
        }),
        OpCode::ObjectNewBound => Some(OpIR {
            kind: molt_ir::OpKind::ObjectNewBound,
            args: Some(operand_args(op)),
            out: out_var,
            // The frontend stores the result class id in the SimpleIR
            // `type_hint` field (`type_hint=class_id`).  The SSA lift
            // round-trips it through the `_type_hint` attribute (see
            // `tir/ssa.rs:1133`); restore it here so downstream backend
            // preanalysis still sees the class identity.
            type_hint: attr_str(&op.attrs, "_type_hint"),
            // Carry the static class-instance payload size in bytes
            // (header NOT included).  The heap arm ignores it; the
            // stack arm (rewritten by escape analysis) uses it to size
            // the Cranelift StackSlot.
            value: attr_int(&op.attrs, "value"),
            // Preserve the finalizer fact across the round-trip so a
            // re-lowering still sees that this instance's class defines
            // `__del__` and must not be stack-promoted / RC-stripped.
            defines_del: attr_bool(&op.attrs, "defines_del"),
            bound_local: attr_bool(&op.attrs, "bound_local"),
            ..OpIR::new(molt_ir::OpKind::ObjectNewBound)
        }),
        OpCode::Free => Some(OpIR {
            kind: molt_ir::OpKind::Free,
            args: Some(operand_args(op)),
            ..OpIR::new(molt_ir::OpKind::Free)
        }),

        // SCF ops — handled separately via terminators in Phase 2.
        OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield => None,

        // Remaining attribute ops.
        OpCode::DelAttr => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::DelAttr);
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                out,
                ..OpIR::new(kind)
            })
        }
        OpCode::DelIndex => {
            let kind = op.origin.unwrap_or(molt_ir::OpKind::DelIndex);
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out,
                ..OpIR::new(kind)
            })
        }
    }
}
