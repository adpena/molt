use crate::runtime_boxed_abi_generated::{RuntimeBoxedReturn, RuntimeFn};
use crate::runtime_calls::{RuntimeArgumentOwnership, RuntimeCall, RuntimeCallRealization};
use crate::tir::target_info::TargetKind;

#[test]
fn runtime_call_contract_preserves_python_meaning_and_result_custody() {
    // Independent obligations: GetIter raises and returns a fresh owned iterator;
    // the dict-storage primitive returns the original borrowed dict.
    for (call, function, arity, result) in [
        (
            RuntimeCall::Iter,
            RuntimeFn::IterChecked,
            1,
            RuntimeBoxedReturn::OwnedValue,
        ),
        (
            RuntimeCall::DictSet,
            RuntimeFn::DictSet,
            3,
            RuntimeBoxedReturn::BorrowedValue,
        ),
    ] {
        assert_eq!(RuntimeCall::from_op(call.op()), Some(call));
        for &kind in call.op_kinds() {
            assert_eq!(RuntimeCall::from_op(kind), Some(call));
        }
        assert_eq!(call.function(), function);
        let abi = function.boxed_abi();
        assert_eq!((abi.arity, abi.result), (arity, result));
        for i in 0..arity {
            assert_eq!(
                call.argument_ownership(i),
                RuntimeArgumentOwnership::Borrowed
            );
        }
        assert!(!call.effects().nothrow);
        assert!(call.effects().may_call_python);
        for target in [
            TargetKind::NativeCranelift,
            TargetKind::Llvm,
            TargetKind::Wasm,
        ] {
            assert_eq!(call.realization(target), RuntimeCallRealization::Runtime);
        }
        assert_eq!(
            call.realization(TargetKind::Rust),
            RuntimeCallRealization::Unsupported
        );
    }
    use crate::tir::ops::OpCode;
    assert_eq!(
        RuntimeCall::from_opcode(OpCode::GetIter),
        Some(RuntimeCall::Iter)
    );
    assert_eq!(RuntimeCall::from_opcode(OpCode::StoreIndex), None);
    assert_eq!(
        RuntimeCall::from_op(crate::OpKind::GetIter),
        Some(RuntimeCall::Iter)
    );
}
