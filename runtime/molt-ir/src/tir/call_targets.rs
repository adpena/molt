use super::op_kinds_generated::{SimpleIrCallTargetRole, simpleir_call_target_role};
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use crate::runtime_fn_generated::RuntimeFn;

/// One identity/result authority for lifting and every TIR GPU call consumer.
const GPU_RUNTIME_INTRINSICS: &[(crate::OpKind, RuntimeFn, TirType)] = &[
    (
        crate::OpKind::GpuThreadId,
        RuntimeFn::GpuThreadId,
        TirType::I64,
    ),
    (
        crate::OpKind::GpuBlockId,
        RuntimeFn::GpuBlockId,
        TirType::I64,
    ),
    (
        crate::OpKind::GpuBlockDim,
        RuntimeFn::GpuBlockDim,
        TirType::I64,
    ),
    (
        crate::OpKind::GpuGridDim,
        RuntimeFn::GpuGridDim,
        TirType::I64,
    ),
    (
        crate::OpKind::GpuBarrier,
        RuntimeFn::GpuBarrier,
        TirType::None,
    ),
];

/// Runtime helper produced when SimpleIR GPU intrinsics are lifted into
/// first-class TIR `Call` ops.
pub fn gpu_runtime_fn_for_simple_kind(kind: crate::OpKind) -> Option<RuntimeFn> {
    GPU_RUNTIME_INTRINSICS
        .iter()
        .find_map(|(source, function, _)| (*source == kind).then_some(*function))
}

/// Decode the frontend's exact builtin bound-method fact at its wire boundary.
pub(super) fn bound_builtin_method(hint: &str, operands: usize) -> Option<RuntimeFn> {
    let runtime = match hint {
        "BoundMethod:list:append" => RuntimeFn::FastListAppend,
        "BoundMethod:str:join" => RuntimeFn::FastStrJoin,
        "BoundMethod:dict:get" => RuntimeFn::FastDictGet,
        "BoundMethod:str:startswith" => RuntimeFn::FastStrStartswith,
        "BoundMethod:str:upper" => RuntimeFn::FastStrUpper,
        "BoundMethod:str:lower" => RuntimeFn::FastStrLower,
        "BoundMethod:str:strip" => RuntimeFn::FastStrStrip,
        _ => return None,
    };
    (runtime.params() == operands).then_some(runtime)
}

/// The runtime entry a SimpleIR operation executes, decoded once at TIR
/// ingest (HF-821 B1). A GPU intrinsic names its fixed helper; a call whose
/// generated role admits a runtime target (`call`), `builtin_func` and
/// `call_async` (its poll function) name theirs in `s_value`. Every other operation, including `call_internal`
/// (always a compiled function), has none. The result travels on the op's
/// typed `runtime` field; no backend decodes a symbol.
pub fn ingest_runtime_target(op: &crate::OpIR) -> Option<RuntimeFn> {
    if op.callee.is_some() {
        return None;
    }
    if op.kind == crate::OpKind::CallMethod {
        return bound_builtin_method(op.s_value.as_deref()?, op.args.as_deref()?.len());
    }
    if let Some(function) = gpu_runtime_fn_for_simple_kind(op.kind) {
        return Some(function);
    }
    if op.kind == crate::OpKind::AllocTask {
        let runtime = RuntimeFn::from_symbol(op.s_value.as_deref()?)?;
        return runtime.poll_key().map(|_| runtime);
    }
    let names_runtime = matches!(
        op.kind,
        crate::OpKind::BuiltinFunc | crate::OpKind::CallAsync
    ) || simpleir_call_target_role(op.kind)
        == Some(SimpleIrCallTargetRole::ExternalOrRuntime);
    if !names_runtime {
        return None;
    }
    RuntimeFn::from_symbol(op.s_value.as_deref()?)
}

/// Whether `op` is a direct call by its preserved source role. Absent
/// transport metadata denotes a canonical first-class TIR `Call`. Preserved
/// SimpleIR spellings must prove a direct role through the generated schema.
/// A registered direct call transported through `Copy` keeps its target, but
/// a plain Copy never gains call identity.
fn has_direct_role(op: &TirOp) -> bool {
    if !matches!(op.opcode, OpCode::Call | OpCode::Copy) || !op.has_valid_shape() {
        return false;
    }
    match op.origin {
        None => op.opcode == OpCode::Call,
        Some(kind) => matches!(
            simpleir_call_target_role(kind),
            Some(
                SimpleIrCallTargetRole::ExternalOrRuntime
                    | SimpleIrCallTargetRole::InternalRequired
            )
        ),
    }
}

/// The boxed runtime entry a direct call executes, with its physical arity.
pub fn direct_boxed_runtime(op: &TirOp) -> Option<RuntimeFn> {
    let runtime = op.runtime?;
    if op.callee.is_some()
        || !has_direct_role(op)
        || op.origin.is_some_and(|kind| {
            simpleir_call_target_role(kind) != Some(SimpleIrCallTargetRole::ExternalOrRuntime)
        })
        || runtime.params() != op.operands.len()
    {
        return None;
    }
    runtime.boxed_abi().map(|_| runtime)
}

/// The compiled function a direct call enters (HF-847 slice 1): its typed
/// callee, never a spelling. An opaque call never has one.
pub fn direct_callee(op: &TirOp) -> Option<crate::FunctionId> {
    op.callee.filter(|_| has_direct_role(op))
}

fn is_guarded_call(op: &TirOp) -> bool {
    op.opcode == OpCode::Call && op.origin == Some(crate::OpKind::CallGuarded)
}

/// The declaration and argument layout validated at a terminal call site.
/// A guarded candidate still needs its callable even before admission proves
/// that the compiled target will run.
pub fn final_call_arguments(
    op: &TirOp,
) -> Option<(crate::FunctionId, crate::ir::CompiledCallArguments)> {
    use crate::ir::CompiledCallArguments;
    let callee = op.callee?;
    let count = op.operands.len();
    let arguments = if is_guarded_call(op) {
        CompiledCallArguments::Guarded(count)
    } else if op.opcode == OpCode::DirectCall {
        CompiledCallArguments::Direct(count)
    } else {
        direct_callee(op)?;
        CompiledCallArguments::Parameters(count)
    };
    Some((callee, arguments))
}

/// A call whose callee and argument binding the program proves: the compiled
/// function it enters and the operands bound to that function's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticCall<'a> {
    pub callee: crate::FunctionId,
    /// The function object captured by a proven guarded call, when carried.
    pub callable: Option<super::values::ValueId>,
    pub arguments: &'a [super::values::ValueId],
}

/// The static call `op` makes (HF-773 slice D): a direct call, whose operands
/// are its arguments, or a `call_guarded` whose callee integration proved
/// (`proven_target`, HF-846 slice 2), whose operand 0 is the callee's function
/// object and whose remaining operands bind its parameters positionally. The
/// late stage enters a proven call's target directly, handing it the function
/// object for its frame record, so the call graph, the inliner and every other
/// whole-program consumer read both as the same edge.
pub fn static_call(op: &TirOp) -> Option<StaticCall<'_>> {
    if let Some(callee) = direct_callee(op) {
        return Some(StaticCall {
            callee,
            callable: None,
            arguments: &op.operands,
        });
    }
    if !is_guarded_call(op) || !op.has_valid_shape() || !op.proven_target() {
        return None;
    }
    let (&callable, arguments) = op.operands.split_first()?;
    Some(StaticCall {
        callee: op.callee?,
        callable: Some(callable),
        arguments,
    })
}

/// The runtime or external symbol a direct call names when it enters no
/// compiled function: a `call` whose wire symbol named no function of its
/// document (HF-821 B1 decodes the runtime entry from it), or a fixed GPU
/// intrinsic. Never an opaque call's incidental string, never a callee.
pub fn runtime_call_symbol(op: &TirOp) -> Option<&str> {
    if op.callee.is_some()
        || !matches!(op.opcode, OpCode::Call | OpCode::Copy)
        || !op.has_valid_shape()
    {
        return None;
    }
    let AttrValue::Str(symbol) = op.attrs.get("s_value")? else {
        return None;
    };
    if symbol.is_empty() {
        return None;
    }
    match op.origin {
        None => (op.opcode == OpCode::Call).then_some(symbol.as_str()),
        Some(kind) => match simpleir_call_target_role(kind) {
            Some(
                SimpleIrCallTargetRole::ExternalOrRuntime
                | SimpleIrCallTargetRole::InternalRequired,
            ) => Some(symbol),
            Some(SimpleIrCallTargetRole::Opaque) => None,
            None => (op.opcode == OpCode::Call
                && gpu_runtime_fn_for_simple_kind(kind).map(RuntimeFn::symbol)
                    == Some(symbol.as_str()))
            .then_some(symbol.as_str()),
        },
    }
}

/// Exact result shape of a fixed GPU call, independent of annotations.
/// GPU helpers have at most one captured result; variable-result `Call` schema
/// admission alone cannot authorize extra slots. Opaque calls never gain
/// intrinsic identity from their symbol spelling.
pub fn gpu_runtime_result_type_for_op(op: &TirOp) -> Option<TirType> {
    if op.opcode != OpCode::Call || op.results.len() > 1 {
        return None;
    }
    // Every genuine GPU lift preserves its source spelling. Requiring that
    // explicit provenance prevents an ordinary module call whose function name
    // happens to equal a runtime helper from minting a fixed scalar result.
    let kind = op.origin?;
    let symbol = runtime_call_symbol(op)?;
    GPU_RUNTIME_INTRINSICS
        .iter()
        .find_map(|(source, runtime, ty)| {
            (*source == kind && runtime.symbol() == symbol).then(|| ty.clone())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::values::ValueId;

    fn call(symbol: &str, original: Option<crate::OpKind>) -> TirOp {
        let mut attrs = AttrDict::default();
        attrs.insert("s_value".into(), AttrValue::Str(symbol.into()));
        TirOp {
            origin: original,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![ValueId(0)],
            attrs,
            source_span: None,
        }
    }

    #[test]
    fn gpu_simple_kinds_map_to_runtime_symbols() {
        for (kind, symbol, result_type) in [
            (
                crate::OpKind::GpuThreadId,
                "molt_gpu_thread_id",
                TirType::I64,
            ),
            (crate::OpKind::GpuBlockId, "molt_gpu_block_id", TirType::I64),
            (
                crate::OpKind::GpuBlockDim,
                "molt_gpu_block_dim",
                TirType::I64,
            ),
            (crate::OpKind::GpuGridDim, "molt_gpu_grid_dim", TirType::I64),
            (crate::OpKind::GpuBarrier, "molt_gpu_barrier", TirType::None),
        ] {
            assert_eq!(
                gpu_runtime_fn_for_simple_kind(kind).map(RuntimeFn::symbol),
                Some(symbol)
            );
            let intrinsic = call(symbol, Some(kind));
            assert_eq!(runtime_call_symbol(&intrinsic), Some(symbol));
            assert_eq!(
                gpu_runtime_result_type_for_op(&intrinsic),
                Some(result_type)
            );
            for original in [
                None,
                Some(crate::OpKind::Call),
                Some(crate::OpKind::CallInternal),
            ] {
                let op = call(symbol, original);
                assert_eq!(runtime_call_symbol(&op), Some(symbol));
                assert_eq!(gpu_runtime_result_type_for_op(&op), None);
            }
        }
        assert_eq!(gpu_runtime_fn_for_simple_kind(crate::OpKind::Call), None);
        assert_eq!(
            gpu_runtime_result_type_for_op(&call("user_function", None)),
            None
        );
    }

    #[test]
    fn opaque_original_kinds_cannot_mint_fixed_gpu_facts() {
        for kind in [
            crate::OpKind::CallFunc,
            crate::OpKind::CallFunction,
            crate::OpKind::CallIndirect,
            crate::OpKind::CallBind,
            crate::OpKind::CallGuarded,
            crate::OpKind::InvokeFfi,
        ] {
            for symbol in ["molt_gpu_thread_id", "molt_gpu_barrier", "module_function"] {
                let op = call(symbol, Some(kind));
                assert_eq!(runtime_call_symbol(&op), None, "{kind}: {symbol}");
                assert_eq!(
                    gpu_runtime_result_type_for_op(&op),
                    None,
                    "{kind}: {symbol}"
                );
            }
        }
    }

    #[test]
    fn copy_transport_requires_a_registered_direct_call_role() {
        for original in [
            None,
            Some(crate::OpKind::Copy),
            Some(crate::OpKind::CallFunc),
            Some(crate::OpKind::Call),
            Some(crate::OpKind::CallInternal),
            Some(crate::OpKind::GpuThreadId),
        ] {
            let mut op = call("molt_gpu_thread_id", original);
            op.opcode = OpCode::Copy;
            let direct = matches!(
                original,
                Some(crate::OpKind::Call | crate::OpKind::CallInternal)
            );
            assert_eq!(runtime_call_symbol(&op).is_some(), direct, "{original:?}");
            assert_eq!(gpu_runtime_result_type_for_op(&op), None, "{original:?}");
        }
    }

    #[test]
    fn a_compiled_callee_is_typed_and_never_a_runtime_symbol() {
        let mut op = call("ignored", Some(crate::OpKind::CallInternal));
        op.attrs.remove("s_value");
        op.callee = Some(crate::FunctionId(4));
        assert_eq!(direct_callee(&op), Some(crate::FunctionId(4)));
        assert_eq!(runtime_call_symbol(&op), None);
        op.origin = Some(crate::OpKind::CallGuarded);
        assert_eq!(
            direct_callee(&op),
            None,
            "an opaque call has no direct callee"
        );
        op.origin = None;
        op.opcode = OpCode::Copy;
        assert_eq!(
            direct_callee(&op),
            None,
            "a plain Copy gains no call identity"
        );
    }

    #[test]
    fn candidate_initializer_is_not_the_class_call_target() {
        let mut op = call("ignored", Some(crate::OpKind::CallClassCandidate));
        op.attrs.remove("s_value");
        op.opcode = OpCode::CallClassCandidate;
        op.callee = Some(crate::FunctionId(3));
        op.operands = vec![ValueId(8), ValueId(9)];
        assert_eq!(direct_callee(&op), None);
        assert_eq!(static_call(&op), None);
        assert!(final_call_arguments(&op).is_none());
    }

    #[test]
    fn a_proven_guarded_call_binds_the_operands_after_its_callee_word() {
        let mut op = call("ignored", Some(crate::OpKind::CallInternal));
        op.attrs.remove("s_value");
        op.callee = Some(crate::FunctionId(4));
        op.operands = vec![ValueId(7), ValueId(8)];
        assert_eq!(
            static_call(&op),
            Some(StaticCall {
                callee: crate::FunctionId(4),
                callable: None,
                arguments: &[ValueId(7), ValueId(8)],
            })
        );
        op.origin = Some(crate::OpKind::CallGuarded);
        assert_eq!(
            static_call(&op),
            None,
            "an unproven guarded call checks its target at run time"
        );
        op.set_proven_target(true);
        assert_eq!(
            static_call(&op),
            Some(StaticCall {
                callee: crate::FunctionId(4),
                callable: Some(ValueId(7)),
                arguments: &[ValueId(8)],
            })
        );
        assert_eq!(direct_callee(&op), None, "the operands are not arguments");
        op.callee = None;
        assert_eq!(static_call(&op), None);
    }

    #[test]
    fn gpu_identity_rejects_mismatched_metadata_opcode_and_result_shape() {
        let internal = call("molt_gpu_thread_id", Some(crate::OpKind::CallInternal));
        assert_eq!(runtime_call_symbol(&internal), Some("molt_gpu_thread_id"));
        assert_eq!(gpu_runtime_result_type_for_op(&internal), None);
        let mut op = call("molt_gpu_thread_id", Some(crate::OpKind::GpuBarrier));
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.origin = None;
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.opcode = OpCode::Copy;
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.opcode = OpCode::Call;
        op.results.push(ValueId(1));
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.results.clear();
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.origin = Some(crate::OpKind::GpuThreadId);
        assert_eq!(gpu_runtime_result_type_for_op(&op), Some(TirType::I64));
    }

    fn simple(kind: crate::OpKind, symbol: Option<&str>) -> crate::OpIR {
        crate::OpIR {
            s_value: symbol.map(Into::into),
            ..crate::OpIR::new(kind)
        }
    }

    #[test]
    fn allocation_admits_only_runtime_polls_without_a_compiled_provider() {
        let poll = RuntimeFn::AsyncSleepPoll;
        let mut allocation = simple(crate::OpKind::AllocTask, Some(poll.symbol()));
        assert_eq!(ingest_runtime_target(&allocation), Some(poll));
        allocation.callee = Some(crate::FunctionId::from_index(0));
        assert_eq!(ingest_runtime_target(&allocation), None);
        let ordinary = simple(crate::OpKind::AllocTask, Some(RuntimeFn::Add.symbol()));
        assert_eq!(ingest_runtime_target(&ordinary), None);
    }

    /// The ingest decode types exactly the ops whose role names a runtime
    /// target, and nothing it could mistake for one (HF-821 B1).
    #[test]
    fn ingest_types_runtime_targets_by_role() {
        use crate::OpKind;
        let len = RuntimeFn::Len.symbol();
        for (hint, arity, expected) in [
            ("BoundMethod:list:append", 2, RuntimeFn::FastListAppend),
            ("BoundMethod:dict:get", 3, RuntimeFn::FastDictGet),
            ("BoundMethod:str:join", 2, RuntimeFn::FastStrJoin),
            (
                "BoundMethod:str:startswith",
                2,
                RuntimeFn::FastStrStartswith,
            ),
            ("BoundMethod:str:upper", 1, RuntimeFn::FastStrUpper),
            ("BoundMethod:str:lower", 1, RuntimeFn::FastStrLower),
            ("BoundMethod:str:strip", 1, RuntimeFn::FastStrStrip),
        ] {
            let mut method = simple(OpKind::CallMethod, Some(hint));
            method.args = Some(vec!["arg".into(); arity]);
            assert_eq!(ingest_runtime_target(&method), Some(expected));
            method.args.as_mut().unwrap().push("extra".into());
            assert_eq!(ingest_runtime_target(&method), None);
        }
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::Call, Some(len))),
            Some(RuntimeFn::Len)
        );
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::BuiltinFunc, Some(len))),
            Some(RuntimeFn::Len)
        );
        assert_eq!(
            ingest_runtime_target(&simple(
                OpKind::CallAsync,
                Some(RuntimeFn::AsyncSleep.symbol())
            )),
            Some(RuntimeFn::AsyncSleep)
        );
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::GpuThreadId, None)),
            Some(RuntimeFn::GpuThreadId)
        );
        // A compiled callee, an internal call and an opaque call never gain a
        // runtime identity, even when the spelling matches an entry.
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::Call, Some("module__f"))),
            None
        );
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::CallInternal, Some(len))),
            None
        );
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::CallFunc, Some(len))),
            None
        );
        assert_eq!(
            ingest_runtime_target(&simple(OpKind::ConstStr, Some(len))),
            None
        );
    }
}
