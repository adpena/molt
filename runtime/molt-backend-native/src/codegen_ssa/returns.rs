//! Function return, unwind and execution-frame custody.
use super::builder::CodegenBuilder;
use super::control::coerce;
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use molt_ir::frame_entry::FrameEntryKind;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::tir::values::ValueId;

/// Local custody is retired before popping any homes. A zero base means this
/// invocation failed before linking the frame.
#[derive(Clone, Copy)]
pub(crate) struct OwnedFrame<S> {
    pub(crate) slot: S,
    pub(crate) kind: Option<FrameEntryKind>,
}

/// An ordinary return may have the Python value None; an unwind has no
/// semantic result at all. Only a checked linkage may carry an inert raw
/// value across that absent-result edge.
#[derive(Clone, Copy)]
pub(crate) enum FunctionExit {
    Return(Option<ValueId>),
    Unwind,
    /// Entry preparation failed before the body could run. Existing
    /// every-exit entries return their ordinary None conversion here;
    /// checked entries instead carry an unavailable inert result.
    EntryAbort,
}

/// The machine result on an absent-result edge. Raw inert values belong
/// only to a linkage that requires its caller to check the exception state
/// before reading the result. No Python object is manufactured for them.
pub(crate) fn absent_return<B: CodegenBuilder>(
    b: &mut B,
    abi: &NativeFunctionLinkageAbi,
) -> Option<B::Value> {
    let carrier = HeldCarrier::of(abi.return_type.as_ref()?);
    assert!(
        carrier == HeldCarrier::Boxed || abi.checked_raw_return(),
        "an absent raw result requires checked linkage"
    );
    Some(match carrier {
        HeldCarrier::RawBool => {
            let bit = b.bit_constant(false);
            b.bit_value(bit)
        }
        HeldCarrier::RawI64 => {
            let zero = b.constant(0);
            b.word_value(zero)
        }
        HeldCarrier::RawF64 => b.float_constant(0.0),
        HeldCarrier::Boxed => {
            let none = b.constant(molt_codegen_abi::QNAN_TAG_NONE_I64 as u64);
            b.word_value(none)
        }
        HeldCarrier::Never => unreachable!("an absent result cannot have the Never carrier"),
    })
}

pub(crate) fn emit_return<B: CodegenBuilder>(
    cx: &mut FunctionCx<'_, B>,
    b: &mut B,
    abi: &NativeFunctionLinkageAbi,
    exit: FunctionExit,
    frame: Option<&OwnedFrame<B::Slot>>,
) {
    let result = match (&abi.return_type, exit) {
        (Some(target), FunctionExit::Unwind)
            if HeldCarrier::of(target) != HeldCarrier::Boxed && !abi.checked_raw_return() =>
        {
            cx.fatal("unwinds through an unchecked raw return linkage ABI");
            b.unreachable();
            return;
        }
        (_, FunctionExit::EntryAbort) if abi.checked_raw_return() => absent_return(b, abi),
        (_, FunctionExit::Unwind) => absent_return(b, abi),
        (Some(target), FunctionExit::Return(None))
            if HeldCarrier::of(target) != HeldCarrier::Boxed =>
        {
            cx.fatal("returns no value through a raw return linkage ABI");
            b.unreachable();
            return;
        }
        (None, FunctionExit::Return(None) | FunctionExit::EntryAbort) => None,
        (None, FunctionExit::Return(Some(_))) => {
            cx.fatal("returns a value through a native-void linkage ABI");
            b.unreachable();
            return;
        }
        (Some(target), exit) => {
            let id = match exit {
                FunctionExit::Return(id) => id,
                FunctionExit::EntryAbort => None,
                FunctionExit::Unwind => unreachable!("unwind was completed above"),
            };
            let (word, source) = match id {
                Some(id) => {
                    let operands = cx.operands();
                    (operands.resolve(b, id), operands.held(id))
                }
                None => {
                    let none = b.constant(molt_codegen_abi::QNAN_TAG_NONE_I64 as u64);
                    (b.word_value(none), HeldCarrier::Boxed)
                }
            };
            let Some(coerced) = coerce(b, cx.plan, word, source, id, target) else {
                cx.fatal(format!(
                    "no return conversion from {source:?} to {target:?}"
                ));
                b.unreachable();
                return;
            };
            Some(coerced)
        }
    };
    if let Some(frame) = frame {
        let base = b.load_slot(frame.slot);
        let cleared = b.constant(0);
        b.store_slot(frame.slot, cleared);
        b.pop_frame(base, frame.kind);
    }
    b.return_value(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen_ssa::scalar::tests::{Edge, Interpreter, Value};
    use crate::codegen_ssa::value_table::ValueTable;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::types::TirType;
    use molt_codegen_abi::word::WordBuilder;
    use molt_ir::linkage_abi::ReturnAvailability;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    fn emit(
        ty: Option<TirType>,
        availability: ReturnAvailability,
        exit: FunctionExit,
    ) -> (Interpreter, Vec<String>) {
        let mut b = Interpreter::default();
        let func = TirFunction::new(
            "exit".into(),
            Vec::new(),
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let plan = ProducerCarrierPlan::default();
        let mut values = ValueTable::with_capacity(0);
        let diagnostics = RefCell::new(Vec::new());
        let runtime = molt_ir::runtime_callable_symbols::RuntimeFnSet::default();
        let functions = molt_ir::FunctionTable::default();
        let linkage = BTreeMap::new();
        let sites = Cell::new(0);
        let abi = NativeFunctionLinkageAbi {
            source_signature: crate::ir::ExternFunctionSignature {
                arity: 0,
                has_closure: false,
                returns_value: ty.is_some(),
                execution_context: crate::ir::ExecutionContextPolicy::Local,
            },
            parameter_custody: Vec::new(),
            param_types: Vec::new(),
            return_type: ty,
            return_availability: availability,
        };
        let frame = OwnedFrame {
            slot: b.entry_slot("frame"),
            kind: None,
        };
        let base = b.constant(0x1000);
        b.store_slot(frame.slot, base);
        let mut cx = FunctionCx {
            func: &func,
            plan: &plan,
            values: &mut values,
            diagnostics: &diagnostics,
            runtime: &runtime,
            linkage: &linkage,
            functions: &functions,
            call_sites: &sites,
        };
        emit_return(&mut cx, &mut b, &abi, exit, Some(&frame));
        (b, diagnostics.into_inner())
    }

    #[test]
    fn checked_unwind_returns_inert_carriers_and_retires_the_frame_without_boxing() {
        for (ty, expected) in [
            (Some(TirType::I64), Some(Value::Word(0))),
            (Some(TirType::Bool), Some(Value::Bit(false))),
            (Some(TirType::F64), Some(Value::Float(0.0))),
            (Some(TirType::DynBox), Some(Value::Word(0x7ffb << 48))),
            (None, None),
        ] {
            for exit in [FunctionExit::Unwind, FunctionExit::EntryAbort] {
                let (b, diagnostics) = emit(ty.clone(), ReturnAvailability::SuccessOnly, exit);
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
                assert_eq!(b.edges, [Edge::Return(expected)]);
                assert!(b.runtime_calls.is_empty(), "no boxing or unboxing");
                assert!(b.refcounts.is_empty());
                assert_eq!(b.popped_frames, [(0x1000, None)]);
                assert_eq!(b.slots, [0], "the owned frame is retired once");
            }
        }
    }

    #[test]
    fn ordinary_missing_raw_results_and_unchecked_raw_unwinds_are_rejected() {
        for ty in [TirType::I64, TirType::Bool, TirType::F64] {
            for (availability, exit, message) in [
                (
                    ReturnAvailability::SuccessOnly,
                    FunctionExit::Return(None),
                    "returns no value through a raw return linkage ABI",
                ),
                (
                    ReturnAvailability::EveryExit,
                    FunctionExit::Unwind,
                    "unwinds through an unchecked raw return linkage ABI",
                ),
            ] {
                let (b, diagnostics) = emit(Some(ty.clone()), availability, exit);
                assert_eq!(diagnostics, [format!("exit: {message}")]);
                assert_eq!(b.edges, [Edge::Unreachable]);
                assert!(b.runtime_calls.is_empty());
            }
        }
    }

    #[test]
    fn an_ordinary_empty_return_keeps_the_boxed_none_and_native_void_abis() {
        for (ty, expected) in [
            (Some(TirType::DynBox), Some(Value::Word(0x7ffb << 48))),
            (None, None),
        ] {
            let (b, diagnostics) = emit(
                ty,
                ReturnAvailability::EveryExit,
                FunctionExit::Return(None),
            );
            assert!(diagnostics.is_empty());
            assert_eq!(b.edges, [Edge::Return(expected)]);
            assert!(b.runtime_calls.is_empty());
            assert_eq!(b.popped_frames, [(0x1000, None)]);
        }
    }

    #[test]
    fn an_every_exit_entry_abort_keeps_its_existing_none_conversion() {
        for (ty, expected) in [
            (Some(TirType::Bool), Some(Value::Bit(false))),
            (Some(TirType::DynBox), Some(Value::Word(0x7ffb << 48))),
            (None, None),
        ] {
            let (b, diagnostics) =
                emit(ty, ReturnAvailability::EveryExit, FunctionExit::EntryAbort);
            assert!(diagnostics.is_empty());
            assert_eq!(b.edges, [Edge::Return(expected)]);
            assert!(b.runtime_calls.is_empty());
            assert_eq!(b.popped_frames, [(0x1000, None)]);
        }
        let (b, diagnostics) = emit(
            Some(TirType::F64),
            ReturnAvailability::EveryExit,
            FunctionExit::EntryAbort,
        );
        assert!(diagnostics.is_empty());
        let [Edge::Return(Some(Value::Float(value)))] = b.edges.as_slice() else {
            panic!("the old float entry returns the reinterpreted None word");
        };
        assert_eq!(value.to_bits(), 0x7ffb << 48);
        assert!(b.runtime_calls.is_empty());
        assert_eq!(b.popped_frames, [(0x1000, None)]);
    }
}
