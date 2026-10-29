//! Exhaustive typed routing for native code generation.
//! Handler declarations own both their patterns and their census slices.

use molt_ir::OpKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(in crate::native_backend::function_compiler) enum NativeOpFamily {
    Arith,
    ArithDivision,
    BitwiseShift,
    MatrixOps,
    Sequence,
    Generators,
    ScalarBuiltins,
    Callargs,
    ConstLiterals,
    ListOps,
    DictOps,
    SetOps,
    SubscriptGet,
    SubscriptStore,
    SliceOps,
    BoxedRuntimeCalls,
    TextTransform,
    RuntimeOps,
    TypeConversions,
    Dataclass,
    Compare,
    UnaryLogic,
    Coroutine,
    Funcobj,
    ObjectConstruct,
    GpuIntrinsic,
    Calls,
    ValueTransfer,
    Modules,
    ClassOps,
    TypeChecks,
    Exceptions,
    ContextMgmt,
    ExceptionStack,
    ExceptionControl,
    ControlFlow,
    Loops,
    Memory,
    Attrs,
    RetJump,
    NoCodegen,
    Unsupported,
}

pub(in crate::native_backend::function_compiler) fn native_op_family(
    kind: OpKind,
) -> NativeOpFamily {
    match kind {
        OpKind::DropInserted | OpKind::ExceptionRegionDropsInserted | OpKind::Nop => {
            NativeOpFamily::NoCodegen
        }
        super::context_mgmt::handled_kinds_pattern!() => NativeOpFamily::ContextMgmt,
        super::matrix_ops::handled_kinds_pattern!() => NativeOpFamily::MatrixOps,
        super::class_ops::handled_kinds_pattern!() => NativeOpFamily::ClassOps,
        super::vec_reductions::handled_kinds_pattern!() => NativeOpFamily::Arith,
        super::compare::handled_kinds_pattern!() => NativeOpFamily::Compare,
        super::slice_ops::handled_kinds_pattern!() => NativeOpFamily::SliceOps,
        super::funcobj::gpu_intrinsic_handled_kinds_pattern!() => NativeOpFamily::GpuIntrinsic,
        super::scalar_builtins::handled_kinds_pattern!() => NativeOpFamily::ScalarBuiltins,
        super::control_flow::handled_kinds_pattern!() => NativeOpFamily::ControlFlow,
        super::arith::handled_kinds_pattern!() => NativeOpFamily::Arith,
        super::arith_division::handled_kinds_pattern!() => NativeOpFamily::ArithDivision,
        super::bitwise_shift::handled_kinds_pattern!() => NativeOpFamily::BitwiseShift,
        super::sequence_ops::handled_kinds_pattern!() => NativeOpFamily::Sequence,
        super::generators::handled_kinds_pattern!() => NativeOpFamily::Generators,
        super::callargs::handled_kinds_pattern!() => NativeOpFamily::Callargs,
        super::const_literals::handled_kinds_pattern!() => NativeOpFamily::ConstLiterals,
        super::list_ops::handled_kinds_pattern!() => NativeOpFamily::ListOps,
        super::dict_ops::handled_kinds_pattern!() => NativeOpFamily::DictOps,
        super::set_ops::handled_kinds_pattern!() => NativeOpFamily::SetOps,
        super::subscript_get::handled_kinds_pattern!() => NativeOpFamily::SubscriptGet,
        super::subscript_store::handled_kinds_pattern!() => NativeOpFamily::SubscriptStore,
        super::text_transform::handled_kinds_pattern!() => NativeOpFamily::TextTransform,
        super::runtime_ops::handled_kinds_pattern!() => NativeOpFamily::RuntimeOps,
        super::type_conversions::handled_kinds_pattern!() => NativeOpFamily::TypeConversions,
        super::dataclass::handled_kinds_pattern!() => NativeOpFamily::Dataclass,
        super::unary_logic::handled_kinds_pattern!() => NativeOpFamily::UnaryLogic,
        super::coroutine::handled_kinds_pattern!() => NativeOpFamily::Coroutine,
        super::funcobj::handled_kinds_pattern!() => NativeOpFamily::Funcobj,
        super::object_construct::handled_kinds_pattern!() => NativeOpFamily::ObjectConstruct,
        super::calls::handled_kinds_pattern!() => NativeOpFamily::Calls,
        super::value_transfer::handled_kinds_pattern!() => NativeOpFamily::ValueTransfer,
        super::modules::handled_kinds_pattern!() => NativeOpFamily::Modules,
        super::type_checks::handled_kinds_pattern!() => NativeOpFamily::TypeChecks,
        super::exceptions::handled_kinds_pattern!() => NativeOpFamily::Exceptions,
        super::exception_stack::handled_kinds_pattern!() => NativeOpFamily::ExceptionStack,
        super::exception_control::handled_kinds_pattern!() => NativeOpFamily::ExceptionControl,
        super::loops::handled_kinds_pattern!() => NativeOpFamily::Loops,
        super::memory::handled_kinds_pattern!() => NativeOpFamily::Memory,
        super::attrs::handled_kinds_pattern!() => NativeOpFamily::Attrs,
        super::ret_jump::handled_kinds_pattern!() => NativeOpFamily::RetJump,
        OpKind::Ret | OpKind::RetVoid => NativeOpFamily::RetJump,
        OpKind::All
        | OpKind::AllocClassStatic
        | OpKind::AllocClassTrusted
        | OpKind::AllocInstance
        | OpKind::Any
        | OpKind::AsyncForEnd
        | OpKind::AsyncForStart
        | OpKind::Band
        | OpKind::Binop
        | OpKind::BinopAdd
        | OpKind::BinopMod
        | OpKind::BinopMul
        | OpKind::BinopPow
        | OpKind::BinopSub
        | OpKind::BitNot
        | OpKind::BoolConst
        | OpKind::Bor
        | OpKind::BoxFromRawInt
        | OpKind::Branch
        | OpKind::BranchFalse
        | OpKind::BranchTrue
        | OpKind::BuildDict
        | OpKind::BuildList
        | OpKind::BuildSet
        | OpKind::BuildSlice
        | OpKind::BuildTuple
        | OpKind::BuiltinAbs
        | OpKind::BuiltinAll
        | OpKind::BuiltinAny
        | OpKind::BuiltinFloat
        | OpKind::BuiltinInt
        | OpKind::BuiltinLen
        | OpKind::BuiltinPrint
        | OpKind::BuiltinRange
        | OpKind::BuiltinReversed
        | OpKind::BuiltinSorted
        | OpKind::BuiltinStr
        | OpKind::BuiltinSum
        | OpKind::Bxor
        | OpKind::BytearrayCount
        | OpKind::BytearrayCountSlice
        | OpKind::BytearrayEndswith
        | OpKind::BytearrayEndswithSlice
        | OpKind::BytearrayFind
        | OpKind::BytearrayFindSlice
        | OpKind::BytearrayStartswith
        | OpKind::BytearrayStartswithSlice
        | OpKind::BytesCount
        | OpKind::BytesCountSlice
        | OpKind::BytesEndswith
        | OpKind::BytesEndswithSlice
        | OpKind::BytesFind
        | OpKind::BytesFindSlice
        | OpKind::BytesStartswith
        | OpKind::BytesStartswithSlice
        | OpKind::CallBuiltin
        | OpKind::CallFunction
        | OpKind::CastFloat
        | OpKind::CastInt
        | OpKind::CastStr
        | OpKind::ClassLayoutFieldCount
        | OpKind::ClassLayoutSlotCount
        | OpKind::CmpEq
        | OpKind::CmpGe
        | OpKind::CmpGt
        | OpKind::CmpLe
        | OpKind::CmpLt
        | OpKind::CmpNe
        | OpKind::Compare
        | OpKind::DelItem
        | OpKind::EndFor
        | OpKind::ExceptEnd
        | OpKind::ExceptStart
        | OpKind::FinallyEnd
        | OpKind::FinallyStart
        | OpKind::Float
        | OpKind::ForIter
        | OpKind::ForIterEnd
        | OpKind::ForIterStart
        | OpKind::ForRange
        | OpKind::FormatString
        | OpKind::Free
        | OpKind::GetItem
        | OpKind::GetIter
        | OpKind::Goto
        | OpKind::GuardBool
        | OpKind::GuardFloat
        | OpKind::GuardInt
        | OpKind::GuardNone
        | OpKind::GuardStr
        | OpKind::IfNot
        | OpKind::Import
        | OpKind::ImportFrom
        | OpKind::ImportName
        | OpKind::In
        | OpKind::IndexSet
        | OpKind::InitInstance
        | OpKind::InstanceGetField
        | OpKind::InstanceHasField
        | OpKind::InstanceSetField
        | OpKind::Int
        | OpKind::IsNot
        | OpKind::LoadAttr
        | OpKind::LoadLocal
        | OpKind::LoopCarryInit
        | OpKind::LoopCarryUpdate
        | OpKind::LoopIndexEnd
        | OpKind::MAnd
        | OpKind::Mod_
        | OpKind::ModuleLoadCached
        | OpKind::Modulo
        | OpKind::NoneConst
        | OpKind::NotIn
        | OpKind::PcallWrapBegin
        | OpKind::PcallWrapEnd
        | OpKind::RaiseFrom
        | OpKind::Range
        | OpKind::Reraise
        | OpKind::Reversed
        | OpKind::SetClear
        | OpKind::SetItem
        | OpKind::Sorted
        | OpKind::StateBlockEnd
        | OpKind::StateBlockStart
        | OpKind::StoreAttr
        | OpKind::StoreFast
        | OpKind::StoreLocal
        | OpKind::StoreSubscript
        | OpKind::Str
        | OpKind::StringConcat
        | OpKind::StringConst
        | OpKind::StringCount
        | OpKind::StringCountSlice
        | OpKind::StringEndswith
        | OpKind::StringEndswithSlice
        | OpKind::StringFind
        | OpKind::StringFindSlice
        | OpKind::StringIndex
        | OpKind::StringIndexSlice
        | OpKind::StringPartition
        | OpKind::StringRepeat
        | OpKind::StringRfind
        | OpKind::StringRfindSlice
        | OpKind::StringRindex
        | OpKind::StringRindexSlice
        | OpKind::StringRpartition
        | OpKind::StringSplitlines
        | OpKind::StringStartswith
        | OpKind::StringStartswithSlice
        | OpKind::Subscript
        | OpKind::Sum
        | OpKind::TrueDiv
        | OpKind::TypeGuard
        | OpKind::UnaryInvert
        | OpKind::UnaryNot
        | OpKind::UnaryOp
        | OpKind::UnboxToRawInt
        | OpKind::Unreachable
        | OpKind::WhileEnd
        | OpKind::WhileStart
        | OpKind::Yield
        | OpKind::YieldFrom
        | OpKind::Zip => {
            if molt_ir::runtime_boxed_abi_generated::runtime_boxed_op_abi(kind).is_some() {
                NativeOpFamily::BoxedRuntimeCalls
            } else {
                NativeOpFamily::Unsupported
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FAMILIES: &[(NativeOpFamily, &[OpKind])] = &[
        (
            NativeOpFamily::ContextMgmt,
            super::super::context_mgmt::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::MatrixOps,
            super::super::matrix_ops::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ClassOps,
            super::super::class_ops::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Arith,
            super::super::vec_reductions::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Compare,
            super::super::compare::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::SliceOps,
            super::super::slice_ops::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::GpuIntrinsic,
            super::super::funcobj::GPU_INTRINSIC_HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ScalarBuiltins,
            super::super::scalar_builtins::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ControlFlow,
            super::super::control_flow::HANDLED_KINDS,
        ),
        (NativeOpFamily::Arith, super::super::arith::HANDLED_KINDS),
        (
            NativeOpFamily::ArithDivision,
            super::super::arith_division::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::BitwiseShift,
            super::super::bitwise_shift::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Sequence,
            super::super::sequence_ops::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Generators,
            super::super::generators::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Callargs,
            super::super::callargs::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ConstLiterals,
            super::super::const_literals::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ListOps,
            super::super::list_ops::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::DictOps,
            super::super::dict_ops::HANDLED_KINDS,
        ),
        (NativeOpFamily::SetOps, super::super::set_ops::HANDLED_KINDS),
        (
            NativeOpFamily::SubscriptGet,
            super::super::subscript_get::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::SubscriptStore,
            super::super::subscript_store::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::TextTransform,
            super::super::text_transform::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::RuntimeOps,
            super::super::runtime_ops::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::TypeConversions,
            super::super::type_conversions::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Dataclass,
            super::super::dataclass::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::UnaryLogic,
            super::super::unary_logic::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Coroutine,
            super::super::coroutine::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Funcobj,
            super::super::funcobj::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ObjectConstruct,
            super::super::object_construct::HANDLED_KINDS,
        ),
        (NativeOpFamily::Calls, super::super::calls::HANDLED_KINDS),
        (
            NativeOpFamily::ValueTransfer,
            super::super::value_transfer::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Modules,
            super::super::modules::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::TypeChecks,
            super::super::type_checks::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::Exceptions,
            super::super::exceptions::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ExceptionStack,
            super::super::exception_stack::HANDLED_KINDS,
        ),
        (
            NativeOpFamily::ExceptionControl,
            super::super::exception_control::HANDLED_KINDS,
        ),
        (NativeOpFamily::Loops, super::super::loops::HANDLED_KINDS),
        (NativeOpFamily::Memory, super::super::memory::HANDLED_KINDS),
        (NativeOpFamily::Attrs, super::super::attrs::HANDLED_KINDS),
        (
            NativeOpFamily::RetJump,
            super::super::ret_jump::HANDLED_KINDS,
        ),
    ];

    #[test]
    fn handler_families_are_disjoint_and_route_to_their_owner() {
        let mut seen = std::collections::HashSet::new();
        for &(family, kinds) in FAMILIES {
            for &kind in kinds {
                assert!(seen.insert(kind), "duplicate native handler for {kind}");
                assert_eq!(native_op_family(kind), family, "{kind}");
            }
        }
    }

    #[test]
    fn generated_boxed_calls_have_an_explicit_native_route() {
        for &kind in molt_ir::runtime_boxed_abi_generated::RUNTIME_BOXED_OP_KINDS {
            assert_eq!(
                native_op_family(kind),
                NativeOpFamily::BoxedRuntimeCalls,
                "{kind}"
            );
        }
    }
}
