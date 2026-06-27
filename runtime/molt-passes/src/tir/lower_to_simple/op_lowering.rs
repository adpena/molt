use crate::ir::OpIR;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::simple_value_names::value_var;

use super::op_utils::{
    attr_bool, attr_bytes, attr_float, attr_int, attr_str, binary_op, operand_args,
    result_or_stream_out, unary_op,
};

// ---------------------------------------------------------------------------
// Op lowering
// ---------------------------------------------------------------------------

/// Convert a single TirOp to an OpIR. Returns None for ops that are
/// dialect-internal and have no SimpleIR equivalent (yet).
pub(super) fn lower_op_many(op: &TirOp) -> Vec<OpIR> {
    if op.opcode == OpCode::Copy
        && matches!(
            attr_str(&op.attrs, "_original_kind").as_deref(),
            Some("store_var")
        )
        && let Some(result) = op.results.first()
    {
        let args = operand_args(op);
        let source_var = args.first().cloned();
        return vec![
            OpIR {
                kind: "store_var".to_string(),
                args: Some(args.clone()),
                var: attr_str(&op.attrs, "_var").or_else(|| Some(value_var(*result))),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy_var".to_string(),
                var: source_var,
                out: Some(value_var(*result)),
                ..OpIR::default()
            },
        ];
    }
    lower_op(op).into_iter().collect()
}

fn lower_op(op: &TirOp) -> Option<OpIR> {
    // Map result (if any) to output variable.
    let out_var = op.results.first().map(|v| value_var(*v));

    match op.opcode {
        // Constants.
        OpCode::ConstInt => Some(OpIR {
            kind: "const".to_string(),
            value: attr_int(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstFloat => Some(OpIR {
            kind: "const_float".to_string(),
            f_value: attr_float(&op.attrs, "f_value").or_else(|| attr_float(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstStr => Some(OpIR {
            kind: "const_str".to_string(),
            s_value: attr_str(&op.attrs, "s_value").or_else(|| attr_str(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),
        // Arbitrary-precision int constant: decimal text in s_value. The
        // module phase re-lifts every function from SimpleIR, so this op
        // MUST round-trip (ssa.rs maps "const_bigint" back to ConstBigInt);
        // as a Copy fallback the TIR-consuming LLVM backend silently left
        // the result undefined (= the None sentinel).
        OpCode::ConstBigInt => Some(OpIR {
            kind: "const_bigint".to_string(),
            s_value: attr_str(&op.attrs, "s_value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBool => Some(OpIR {
            kind: "const_bool".to_string(),
            // Both the SSA lift and SCCP store ConstBool values as
            // AttrValue::Bool.  Legacy AttrValue::Int is handled for
            // backward compatibility with cached TIR artifacts.
            value: Some(match op.attrs.get("value") {
                Some(AttrValue::Bool(b)) => u8::from(*b) as i64,
                Some(AttrValue::Int(i)) => u8::from(*i != 0) as i64,
                _ => 0,
            }),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstNone => Some(OpIR {
            kind: "const_none".to_string(),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBytes => Some(OpIR {
            kind: "const_bytes".to_string(),
            bytes: attr_bytes(&op.attrs, "bytes").or_else(|| attr_bytes(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),

        // Arithmetic.
        OpCode::Add => Some(binary_op("add", op, out_var)),
        OpCode::Sub => Some(binary_op("sub", op, out_var)),
        OpCode::Mul => Some(binary_op("mul", op, out_var)),
        OpCode::InplaceAdd => Some(binary_op("inplace_add", op, out_var)),
        OpCode::InplaceSub => Some(binary_op("inplace_sub", op, out_var)),
        OpCode::InplaceMul => Some(binary_op("inplace_mul", op, out_var)),
        OpCode::Div => Some(binary_op("div", op, out_var)),
        // Canonical wire spelling is `floordiv` (the frontend emission); see the
        // op-kind registry (op_kinds.toml). Emitting the canonical here makes the
        // SimpleIR↔TIR round-trip idempotent (`kind_to_opcode("floordiv")` ->
        // OpCode::FloorDiv) and routes through the same `"floordiv"` dispatch arm
        // every backend already has — closing the floordiv/floor_div schism.
        OpCode::FloorDiv => Some(binary_op("floordiv", op, out_var)),
        OpCode::Mod => Some(binary_op("mod", op, out_var)),
        OpCode::Pow => Some(binary_op("pow", op, out_var)),
        OpCode::Neg => Some(unary_op("neg", op, out_var)),
        OpCode::Pos => Some(unary_op("pos", op, out_var)),

        // Checked arithmetic: two output vars, mirroring IterNextUnboxed —
        // `var` = results[0] (wrapping sum), `out` = results[1] (overflow
        // flag). The module phase re-lifts every function from SimpleIR, so
        // this op MUST round-trip (ssa.rs maps "checked_add" back to
        // OpCode::CheckedAdd with the same var/out → results order); without
        // the pair it would fall to the Copy fallback and silently vanish.
        OpCode::CheckedAdd => {
            let sum_var = op.results.first().map(|v| value_var(*v));
            let flag_var = op.results.get(1).map(|v| value_var(*v));
            Some(OpIR {
                kind: "checked_add".to_string(),
                args: Some(operand_args(op)),
                out: flag_var,
                var: sum_var,
                ..OpIR::default()
            })
        }
        // CheckedMul mirrors CheckedAdd exactly: `var` = results[0] (wrapping
        // product), `out` = results[1] (overflow flag). Same round-trip
        // requirement — ssa.rs maps "checked_mul" back to OpCode::CheckedMul.
        OpCode::CheckedMul => {
            let product_var = op.results.first().map(|v| value_var(*v));
            let flag_var = op.results.get(1).map(|v| value_var(*v));
            Some(OpIR {
                kind: "checked_mul".to_string(),
                args: Some(operand_args(op)),
                out: flag_var,
                var: product_var,
                ..OpIR::default()
            })
        }

        // Comparison.
        OpCode::Eq => Some(binary_op("eq", op, out_var)),
        OpCode::Ne => Some(binary_op("ne", op, out_var)),
        OpCode::Lt => Some(binary_op("lt", op, out_var)),
        OpCode::Le => Some(binary_op("le", op, out_var)),
        OpCode::Gt => Some(binary_op("gt", op, out_var)),
        OpCode::Ge => Some(binary_op("ge", op, out_var)),
        OpCode::Is => Some(binary_op("is", op, out_var)),
        OpCode::IsNot => Some(binary_op("is_not", op, out_var)),
        OpCode::In => Some(binary_op("in", op, out_var)),
        OpCode::NotIn => Some(binary_op("not_in", op, out_var)),

        // Bitwise.
        OpCode::BitAnd => Some(binary_op("bit_and", op, out_var)),
        OpCode::BitOr => Some(binary_op("bit_or", op, out_var)),
        OpCode::BitXor => Some(binary_op("bit_xor", op, out_var)),
        OpCode::BitNot => Some(unary_op("bit_not", op, out_var)),
        OpCode::Shl => Some(binary_op("lshift", op, out_var)),
        OpCode::Shr => Some(binary_op("rshift", op, out_var)),

        // Boolean.
        OpCode::And => Some(binary_op("and", op, out_var)),
        OpCode::Or => Some(binary_op("or", op, out_var)),
        OpCode::Not => Some(unary_op("not", op, out_var)),
        OpCode::Bool => Some(unary_op("bool", op, out_var)),

        // Memory.
        OpCode::LoadAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "get_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out: out_var,
                ic_index: attr_int(&op.attrs, "ic_index"),
                // Preserve the typed-slot class identity across the roundtrip so
                // the alias oracle's class+offset region is stable (S5-1.5).
                class_name: attr_str(&op.attrs, "_class"),
                ..OpIR::default()
            })
        }
        OpCode::StoreAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "set_attr".to_string());
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out,
                class_name: attr_str(&op.attrs, "_class"),
                ..OpIR::default()
            })
        }
        OpCode::Index => {
            let kind = attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "index".to_string());
            let mut opir = binary_op(&kind, op, out_var);
            // Restore semantic container_type from the preserved attr so
            // backend dispatch can use typed container facts.
            opir.container_type = attr_str(&op.attrs, "container_type");
            // Propagate BCE proof so codegen can skip bounds checks.
            opir.bce_safe = attr_bool(&op.attrs, "bce_safe");
            Some(opir)
        }
        OpCode::StoreIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "store_index".to_string());
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out,
                container_type: attr_str(&op.attrs, "container_type"),
                // Propagate BCE proof so codegen can skip bounds checks.
                bce_safe: attr_bool(&op.attrs, "bce_safe"),
                ..OpIR::default()
            })
        }
        OpCode::DeleteVar => Some(OpIR {
            kind: "delete_var".to_string(),
            args: Some(operand_args(op)),
            var: attr_str(&op.attrs, "_var").or_else(|| op.results.first().map(|v| value_var(*v))),
            ..OpIR::default()
        }),

        // Call — s_value holds the target function name, value holds the code_id.
        // Recover the original SimpleIR kind (call_func, call_indirect, etc.)
        // if it was preserved during the SSA lift.
        OpCode::Call => {
            let kind = attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "call".to_string());
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
                ..OpIR::default()
            })
        }
        OpCode::CallMethod => Some(OpIR {
            kind: "call_method".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallMethodIc => Some(OpIR {
            kind: "call_method_ic".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method").or_else(|| attr_str(&op.attrs, "s_value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallSuperMethodIc => Some(OpIR {
            kind: "call_super_method_ic".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method").or_else(|| attr_str(&op.attrs, "s_value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallBuiltin => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "call_builtin".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name"),
                out: out_var,
                ..OpIR::default()
            })
        }
        OpCode::OrdAt => Some(OpIR {
            kind: "ord_at".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Box/unbox — no-ops at SimpleIR level (type info discarded).
        OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard => {
            if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Copy: either a genuine copy_var or a passthrough for an unknown op
        // whose original kind was preserved in attrs.
        OpCode::Copy => {
            if let Some(original_kind) = attr_str(&op.attrs, "_original_kind") {
                if original_kind == "store_var" {
                    return Some(OpIR {
                        kind: original_kind,
                        args: Some(operand_args(op)),
                        var: attr_str(&op.attrs, "_var")
                            .or_else(|| op.results.first().map(|v| value_var(*v))),
                        ..OpIR::default()
                    });
                }
                if original_kind == "unpack_sequence" {
                    let mut args = operand_args(op);
                    args.extend(op.results.iter().map(|v| value_var(*v)));
                    return Some(OpIR {
                        kind: original_kind,
                        args: Some(args),
                        value: attr_int(&op.attrs, "value"),
                        f_value: attr_float(&op.attrs, "f_value"),
                        s_value: attr_str(&op.attrs, "s_value"),
                        bytes: attr_bytes(&op.attrs, "bytes"),
                        var: attr_str(&op.attrs, "_var"),
                        task_kind: attr_str(&op.attrs, "task_kind"),
                        container_type: attr_str(&op.attrs, "container_type"),
                        ic_index: attr_int(&op.attrs, "ic_index"),
                        ..OpIR::default()
                    });
                }
                // Passthrough: reconstruct the original SimpleIR op with all fields.
                Some(OpIR {
                    kind: original_kind,
                    args: if op.operands.is_empty() {
                        None
                    } else {
                        Some(operand_args(op))
                    },
                    out: out_var,
                    value: attr_int(&op.attrs, "value"),
                    f_value: attr_float(&op.attrs, "f_value"),
                    s_value: attr_str(&op.attrs, "s_value"),
                    bytes: attr_bytes(&op.attrs, "bytes"),
                    var: attr_str(&op.attrs, "_var"),
                    task_kind: attr_str(&op.attrs, "task_kind"),
                    container_type: attr_str(&op.attrs, "container_type"),
                    ic_index: attr_int(&op.attrs, "ic_index"),
                    // Named-local fact (#58) — container literals (`list_new`/
                    // `tuple_new`) ride this passthrough; losing the attr here
                    // silently degrades the scope-boundary deferral.
                    bound_local: attr_bool(&op.attrs, "bound_local"),
                    ..OpIR::default()
                })
            } else if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: attr_str(&op.attrs, "_var").or_else(|| Some(value_var(*src))),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Build containers.
        OpCode::BuildList => Some(OpIR {
            kind: "build_list".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildDict => Some(OpIR {
            kind: "build_dict".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildTuple => Some(OpIR {
            kind: "build_tuple".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSet => Some(OpIR {
            kind: "build_set".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSlice => Some(OpIR {
            kind: "build_slice".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Iteration.
        OpCode::GetIter => Some(unary_op("get_iter", op, out_var)),
        OpCode::IterNext => Some(unary_op("iter_next", op, out_var)),
        OpCode::IterNextUnboxed => {
            // Emit as iter_next_unboxed with two output vars:
            // results[0] = value, results[1] = done_flag.
            let val_var = op.results.first().map(|v| value_var(*v));
            let done_var = op.results.get(1).map(|v| value_var(*v));
            Some(OpIR {
                kind: "iter_next_unboxed".to_string(),
                args: Some(operand_args(op)),
                out: done_var,
                var: val_var,
                ..OpIR::default()
            })
        }
        OpCode::ForIter => Some(OpIR {
            kind: "for_iter".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Generator.
        OpCode::AllocTask => Some(OpIR {
            kind: "alloc_task".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            s_value: attr_str(&op.attrs, "s_value"),
            value: attr_int(&op.attrs, "value"),
            task_kind: attr_str(&op.attrs, "task_kind"),
            ..OpIR::default()
        }),
        OpCode::StateSwitch => Some(OpIR {
            kind: "state_switch".to_string(),
            ..OpIR::default()
        }),
        OpCode::StateTransition => Some(OpIR {
            kind: "state_transition".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateYield => Some(OpIR {
            kind: "state_yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ChanSendYield => Some(OpIR {
            kind: "chan_send_yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ChanRecvYield => Some(OpIR {
            kind: "chan_recv_yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ClosureLoad => Some(OpIR {
            kind: "closure_load".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ClosureStore => Some(OpIR {
            kind: "closure_store".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::Yield => Some(OpIR {
            kind: "yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::YieldFrom => Some(OpIR {
            kind: "yield_from".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Exception.
        OpCode::Raise => Some(OpIR {
            kind: "raise".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::CheckException => Some(OpIR {
            kind: "check_exception".to_string(),
            // Emit with None args (matching the original structured IR format).
            // The Cranelift backend manages live-value state implicitly from
            // the structured control flow context. Emitting the TIR operands
            // (which are all block-argument values captured at exception
            // boundaries) causes the backend to generate incorrect exception
            // handling state with inflated argument lists.
            args: None,
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ExceptionPending => Some(OpIR {
            // Reads the runtime exception-pending flag as a boolean value.
            // No operands; produces the condition consumed by the
            // `loop_break_if_exception` CondBranch.
            kind: "exception_pending".to_string(),
            args: None,
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::FunctionDefaultsVersion => Some(OpIR {
            // Reads a function object's `__defaults__`/`__kwdefaults__`
            // mutation version stamp as an inline int.  One operand (the
            // function object); produces the value the defaults-devirt deopt
            // guard compares against 0.  Non-foldable: it observes mutable
            // runtime state (`side_effecting` in the op-kind registry).
            kind: "function_defaults_version".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::TryStart => Some(OpIR {
            kind: "try_start".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::TryEnd => Some(OpIR {
            kind: "try_end".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateBlockStart => Some(OpIR {
            kind: "state_block_start".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateBlockEnd => Some(OpIR {
            kind: "state_block_end".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),

        // Import.
        OpCode::Import => {
            let args = operand_args(op);
            if args.is_empty() {
                Some(OpIR {
                    kind: "import".to_string(),
                    s_value: attr_str(&op.attrs, "module"),
                    out: out_var,
                    ..OpIR::default()
                })
            } else {
                Some(OpIR {
                    kind: "module_import".to_string(),
                    args: Some(args),
                    out: out_var,
                    ..OpIR::default()
                })
            }
        }
        OpCode::ImportFrom => Some(OpIR {
            kind: "import_from".to_string(),
            s_value: attr_str(&op.attrs, "name"),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ModuleCacheGet => Some(OpIR {
            kind: "module_cache_get".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            effect_proof: attr_str(&op.attrs, "effect_proof"),
            ..OpIR::default()
        }),
        OpCode::ModuleCacheSet => Some(OpIR {
            kind: "module_cache_set".to_string(),
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::default()
        }),
        OpCode::ModuleCacheDel => Some(OpIR {
            kind: "module_cache_del".to_string(),
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::default()
        }),
        OpCode::ModuleGetAttr => Some(OpIR {
            kind: "module_get_attr".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            effect_proof: attr_str(&op.attrs, "effect_proof"),
            ..OpIR::default()
        }),
        OpCode::ModuleImportFrom => Some(OpIR {
            kind: "module_import_from".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ModuleGetGlobal => Some(OpIR {
            kind: "module_get_global".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ModuleGetName => Some(OpIR {
            kind: "module_get_name".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ModuleSetAttr => Some(OpIR {
            kind: "module_set_attr".to_string(),
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::default()
        }),
        OpCode::ModuleDelGlobal => Some(OpIR {
            kind: "module_del_global".to_string(),
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::default()
        }),
        OpCode::ModuleDelGlobalIfPresent => Some(OpIR {
            kind: "module_del_global_if_present".to_string(),
            args: Some(operand_args(op)),
            out: Some("none".to_string()),
            ..OpIR::default()
        }),
        OpCode::WarnStderr => Some(OpIR {
            kind: "warn_stderr".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),

        // Refcount and allocation — preserve for native backend.
        OpCode::IncRef => Some(OpIR {
            kind: "inc_ref".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::DecRef => Some(OpIR {
            kind: "dec_ref".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        // Python lifetime boundary (`del x`, #58). Normally the drop phase
        // normalizes this away before back-conversion on drop-activated
        // targets; on the dormant-native lane it survives so the native
        // preanalysis can pin the local's last_use to the del statement
        // (codegen's default arm ignores the kind).
        OpCode::DelBoundary => Some(OpIR {
            kind: "del_boundary".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::Alloc => Some(OpIR {
            kind: "alloc".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            s_value: attr_str(&op.attrs, "s_value"),
            ..OpIR::default()
        }),
        OpCode::StackAlloc => Some(OpIR {
            kind: "stack_alloc".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ObjectNewBound => Some(OpIR {
            kind: "object_new_bound".to_string(),
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
            ..OpIR::default()
        }),
        OpCode::ObjectNewBoundStack => Some(OpIR {
            kind: "object_new_bound_stack".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            type_hint: attr_str(&op.attrs, "_type_hint"),
            // Inherited from the original `ObjectNewBound` — required
            // for the StackSlot lowering to know the payload size.
            value: attr_int(&op.attrs, "value"),
            stack_eligible: Some(true),
            ..OpIR::default()
        }),
        OpCode::Free => Some(OpIR {
            kind: "free".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),

        // SCF ops — handled separately via terminators in Phase 2.
        OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield => None,

        // Deopt — emit a hint but not critical.
        OpCode::Deopt => Some(OpIR {
            kind: "deopt".to_string(),
            ..OpIR::default()
        }),

        // Remaining attribute ops.
        OpCode::DelAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "del_attr".to_string());
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                out,
                ..OpIR::default()
            })
        }
        OpCode::DelIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "del_index".to_string());
            let out = result_or_stream_out(op, out_var);
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out,
                ..OpIR::default()
            })
        }
    }
}
