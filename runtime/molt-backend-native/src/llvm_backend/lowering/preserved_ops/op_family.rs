//! Exhaustive typed LLVM preserved-operation family routing.
#[cfg(test)]
use super::{callable_ops, container_ops, direct_ops};
use molt_ir::OpKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(super) enum LlvmPreservedOpFamily {
    Direct,
    Container,
    Callable,
}

#[cfg(test)]
const FAMILY_DISPATCH_TABLE: &[(LlvmPreservedOpFamily, &[OpKind])] = &[
    (LlvmPreservedOpFamily::Direct, direct_ops::HANDLED_KINDS),
    (
        LlvmPreservedOpFamily::Container,
        container_ops::HANDLED_KINDS,
    ),
    (LlvmPreservedOpFamily::Callable, callable_ops::HANDLED_KINDS),
];

pub(super) const fn llvm_preserved_op_family(kind: OpKind) -> Option<LlvmPreservedOpFamily> {
    match kind {
        super::direct_ops::handled_kind_pattern!() => Some(LlvmPreservedOpFamily::Direct),
        super::container_ops::handled_kind_pattern!() => Some(LlvmPreservedOpFamily::Container),
        super::callable_ops::handled_kind_pattern!() => Some(LlvmPreservedOpFamily::Callable),
        OpKind::Add
        | OpKind::Aiter
        | OpKind::All
        | OpKind::Alloc
        | OpKind::AllocClassStatic
        | OpKind::AllocClassTrusted
        | OpKind::AllocInstance
        | OpKind::AllocTask
        | OpKind::And
        | OpKind::Anext
        | OpKind::Any
        | OpKind::AsciiFromObj
        | OpKind::AsyncForEnd
        | OpKind::AsyncForStart
        | OpKind::AsyncWorkPoll
        | OpKind::Band
        | OpKind::Binop
        | OpKind::BinopAdd
        | OpKind::BinopFloorDiv
        | OpKind::BinopMod
        | OpKind::BinopMul
        | OpKind::BinopPow
        | OpKind::BinopSub
        | OpKind::BitAnd
        | OpKind::BitNot
        | OpKind::BitOr
        | OpKind::BitXor
        | OpKind::Bool
        | OpKind::BoolConst
        | OpKind::Bor
        | OpKind::BoundMethodNew
        | OpKind::Box
        | OpKind::BoxFromRawInt
        | OpKind::BrIf
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
        | OpKind::BuiltinBool
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
        | OpKind::BytearrayFillRange
        | OpKind::BytearrayFind
        | OpKind::BytearrayFindSlice
        | OpKind::BytearrayFromObj
        | OpKind::BytearrayFromStr
        | OpKind::BytearrayReplace
        | OpKind::BytearraySplit
        | OpKind::BytearraySplitMax
        | OpKind::BytearrayStartswith
        | OpKind::BytearrayStartswithSlice
        | OpKind::BytesCount
        | OpKind::BytesCountSlice
        | OpKind::BytesEndswith
        | OpKind::BytesEndswithSlice
        | OpKind::BytesFind
        | OpKind::BytesFindSlice
        | OpKind::BytesFromObj
        | OpKind::BytesFromStr
        | OpKind::BytesReplace
        | OpKind::BytesSplit
        | OpKind::BytesSplitMax
        | OpKind::BytesStartswith
        | OpKind::BytesStartswithSlice
        | OpKind::Call
        | OpKind::CallBind
        | OpKind::CallBuiltin
        | OpKind::CallFunc
        | OpKind::CallFunction
        | OpKind::CallGuarded
        | OpKind::CallIndirect
        | OpKind::CallInternal
        | OpKind::CallLoadedMethod
        | OpKind::CallMethod
        | OpKind::CallargsExpandKwstar
        | OpKind::CallargsExpandStar
        | OpKind::CallargsPushKw
        | OpKind::CallargsPushMethodReceiver
        | OpKind::CallargsPushPos
        | OpKind::CastBool
        | OpKind::CastFloat
        | OpKind::CastInt
        | OpKind::CastStr
        | OpKind::CheckException
        | OpKind::CheckedAdd
        | OpKind::CheckedMul
        | OpKind::Chr
        | OpKind::ClassLayoutFieldCount
        | OpKind::ClassLayoutSlotCount
        | OpKind::ClassmethodNew
        | OpKind::ClosureLoad
        | OpKind::ClosureStore
        | OpKind::CmpEq
        | OpKind::CmpGe
        | OpKind::CmpGt
        | OpKind::CmpLe
        | OpKind::CmpLt
        | OpKind::CmpNe
        | OpKind::CodeNew
        | OpKind::Compare
        | OpKind::ComplexFromObj
        | OpKind::Const
        | OpKind::ConstBigint
        | OpKind::ConstBool
        | OpKind::ConstBytes
        | OpKind::ConstFloat
        | OpKind::ConstInt
        | OpKind::ConstNone
        | OpKind::ConstStr
        | OpKind::ContextEnter
        | OpKind::ContextExit
        | OpKind::Copy
        | OpKind::DataclassGet
        | OpKind::DataclassSet
        | OpKind::DataclassSetClass
        | OpKind::DecRef
        | OpKind::DelAttr
        | OpKind::DelAttrGenericObj
        | OpKind::DelAttrGenericPtr
        | OpKind::DelAttrName
        | OpKind::DelBoundary
        | OpKind::DelIndex
        | OpKind::DelItem
        | OpKind::DeleteVar
        | OpKind::DictClear
        | OpKind::DictCopy
        | OpKind::DictFromObj
        | OpKind::DictGet
        | OpKind::DictItems
        | OpKind::DictKeys
        | OpKind::DictPop
        | OpKind::DictPopitem
        | OpKind::DictSet
        | OpKind::DictSetdefault
        | OpKind::DictSetdefaultEmptyList
        | OpKind::DictStrIntInc
        | OpKind::DictUnpack
        | OpKind::DictUpdate
        | OpKind::DictUpdateKwstar
        | OpKind::DictUpdateMissing
        | OpKind::DictValues
        | OpKind::Div
        | OpKind::DropInserted
        | OpKind::Else
        | OpKind::EndFor
        | OpKind::EndIf
        | OpKind::Enumerate
        | OpKind::Eq
        | OpKind::ExceptEnd
        | OpKind::ExceptStart
        | OpKind::ExceptionClass
        | OpKind::ExceptionContextSet
        | OpKind::ExceptionNew
        | OpKind::ExceptionNewFromClass
        | OpKind::ExceptionPending
        | OpKind::ExceptionRegionDropsInserted
        | OpKind::ExceptionSetLast
        | OpKind::ExceptionStackExit
        | OpKind::ExceptionStackSetDepth
        | OpKind::ExceptiongroupCombine
        | OpKind::ExceptiongroupMatch
        | OpKind::FinallyEnd
        | OpKind::FinallyStart
        | OpKind::Float
        | OpKind::FloatFromObj
        | OpKind::FloorDiv
        | OpKind::ForIter
        | OpKind::ForIterEnd
        | OpKind::ForIterStart
        | OpKind::ForRange
        | OpKind::FormatString
        | OpKind::FrameContextSet
        | OpKind::Free
        | OpKind::FrozensetAdd
        | OpKind::FunctionDefaultsVersion
        | OpKind::Ge
        | OpKind::GetAttr
        | OpKind::GetAttrGenericObj
        | OpKind::GetAttrGenericPtr
        | OpKind::GetAttrName
        | OpKind::GetItem
        | OpKind::GetIter
        | OpKind::Getframe
        | OpKind::Goto
        | OpKind::GpuBarrier
        | OpKind::GpuBlockDim
        | OpKind::GpuBlockId
        | OpKind::GpuGridDim
        | OpKind::GpuThreadId
        | OpKind::Gt
        | OpKind::GuardBool
        | OpKind::GuardFloat
        | OpKind::GuardInt
        | OpKind::GuardNone
        | OpKind::GuardStr
        | OpKind::GuardedFieldGet
        | OpKind::GuardedFieldSet
        | OpKind::GuardedLoad
        | OpKind::HasAttrName
        | OpKind::Id
        | OpKind::If
        | OpKind::IfNot
        | OpKind::Import
        | OpKind::ImportFrom
        | OpKind::ImportName
        | OpKind::In
        | OpKind::IncRef
        | OpKind::Index
        | OpKind::IndexSet
        | OpKind::InitInstance
        | OpKind::InplaceAdd
        | OpKind::InplaceMatmul
        | OpKind::InplaceMul
        | OpKind::InplaceSub
        | OpKind::InstanceGetField
        | OpKind::InstanceHasField
        | OpKind::InstanceSetField
        | OpKind::Int
        | OpKind::IntFromObj
        | OpKind::IntFromStrOfObj
        | OpKind::InvokeFfi
        | OpKind::Is
        | OpKind::IsBoundMethod
        | OpKind::IsCallable
        | OpKind::IsGenerator
        | OpKind::IsNativeAwaitable
        | OpKind::IsNot
        | OpKind::IsPending
        | OpKind::Isinstance
        | OpKind::Issubclass
        | OpKind::IterNext
        | OpKind::Jump
        | OpKind::Label
        | OpKind::Le
        | OpKind::ListAppend
        | OpKind::ListClear
        | OpKind::ListCopy
        | OpKind::ListCount
        | OpKind::ListExtend
        | OpKind::ListFillNew
        | OpKind::ListFromRange
        | OpKind::ListIndex
        | OpKind::ListIndexRange
        | OpKind::ListInsert
        | OpKind::ListIntNew
        | OpKind::ListPop
        | OpKind::ListRemove
        | OpKind::ListReverse
        | OpKind::Load
        | OpKind::LoadAttr
        | OpKind::LoadConst
        | OpKind::LoadLocal
        | OpKind::LoadMethod
        | OpKind::LoadSuperMethod
        | OpKind::LoadVar
        | OpKind::LoopBreak
        | OpKind::LoopBreakIfException
        | OpKind::LoopBreakIfFalse
        | OpKind::LoopBreakIfTrue
        | OpKind::LoopCarryInit
        | OpKind::LoopCarryUpdate
        | OpKind::LoopContinue
        | OpKind::LoopEnd
        | OpKind::LoopIndexEnd
        | OpKind::LoopIndexNext
        | OpKind::LoopIndexStart
        | OpKind::LoopStart
        | OpKind::Lshift
        | OpKind::Lt
        | OpKind::MAnd
        | OpKind::Matmul
        | OpKind::Mod
        | OpKind::Mod_
        | OpKind::ModuleCacheDel
        | OpKind::ModuleCacheGet
        | OpKind::ModuleCacheSet
        | OpKind::ModuleDelGlobal
        | OpKind::ModuleDelGlobalIfPresent
        | OpKind::ModuleGetAttr
        | OpKind::ModuleGetGlobal
        | OpKind::ModuleGetName
        | OpKind::ModuleImport
        | OpKind::ModuleImportFrom
        | OpKind::ModuleImportStar
        | OpKind::ModuleLoadCached
        | OpKind::ModuleNew
        | OpKind::ModuleSetAttr
        | OpKind::ModuleSlot
        | OpKind::Modulo
        | OpKind::Mul
        | OpKind::Ne
        | OpKind::Neg
        | OpKind::NoneConst
        | OpKind::Nop
        | OpKind::Not
        | OpKind::NotIn
        | OpKind::ObjectNewBound
        | OpKind::OperatorIndex
        | OpKind::Or
        | OpKind::Ord
        | OpKind::OrdAt
        | OpKind::PcallWrapBegin
        | OpKind::PcallWrapEnd
        | OpKind::Phi
        | OpKind::Pos
        | OpKind::Pow
        | OpKind::PowMod
        | OpKind::Print
        | OpKind::PrintNewline
        | OpKind::PropertyNew
        | OpKind::Raise
        | OpKind::RaiseFrom
        | OpKind::Range
        | OpKind::RangeNew
        | OpKind::ReprFromObj
        | OpKind::Reraise
        | OpKind::Ret
        | OpKind::RetVoid
        | OpKind::Reversed
        | OpKind::Round
        | OpKind::Rshift
        | OpKind::SetAdd
        | OpKind::SetAddProbe
        | OpKind::SetAttr
        | OpKind::SetAttrGenericObj
        | OpKind::SetAttrGenericPtr
        | OpKind::SetAttrName
        | OpKind::SetClear
        | OpKind::SetDifferenceUpdate
        | OpKind::SetDiscard
        | OpKind::SetIntersectionUpdate
        | OpKind::SetItem
        | OpKind::SetPop
        | OpKind::SetRemove
        | OpKind::SetSymdiffUpdate
        | OpKind::SetUpdate
        | OpKind::Shl
        | OpKind::Shr
        | OpKind::Slice
        | OpKind::SliceNew
        | OpKind::Sorted
        | OpKind::StackAlloc
        | OpKind::StateBlockEnd
        | OpKind::StateBlockStart
        | OpKind::StateLabel
        | OpKind::StateSet
        | OpKind::StateSwitch
        | OpKind::StateTransition
        | OpKind::StateYield
        | OpKind::StaticmethodNew
        | OpKind::Store
        | OpKind::StoreAttr
        | OpKind::StoreFast
        | OpKind::StoreIndex
        | OpKind::StoreLocal
        | OpKind::StoreSubscript
        | OpKind::Str
        | OpKind::StrFromObj
        | OpKind::StringCapitalize
        | OpKind::StringConcat
        | OpKind::StringConst
        | OpKind::StringCount
        | OpKind::StringCountSlice
        | OpKind::StringEndswith
        | OpKind::StringEndswithSlice
        | OpKind::StringEq
        | OpKind::StringFind
        | OpKind::StringFindSlice
        | OpKind::StringIndex
        | OpKind::StringIndexSlice
        | OpKind::StringJoin
        | OpKind::StringLower
        | OpKind::StringLstrip
        | OpKind::StringPartition
        | OpKind::StringRepeat
        | OpKind::StringReplace
        | OpKind::StringRfind
        | OpKind::StringRfindSlice
        | OpKind::StringRindex
        | OpKind::StringRindexSlice
        | OpKind::StringRpartition
        | OpKind::StringRstrip
        | OpKind::StringSplit
        | OpKind::StringSplitField
        | OpKind::StringSplitFieldEnd
        | OpKind::StringSplitFieldEq
        | OpKind::StringSplitFieldIsAscii
        | OpKind::StringSplitFieldLen
        | OpKind::StringSplitFieldLenFromBounds
        | OpKind::StringSplitFieldOrdAtBounds
        | OpKind::StringSplitFieldStart
        | OpKind::StringSplitFieldToInt
        | OpKind::StringSplitMax
        | OpKind::StringSplitSepDictInc
        | OpKind::StringSplitValidate
        | OpKind::StringSplitWsDictInc
        | OpKind::StringSplitlines
        | OpKind::StringStartswith
        | OpKind::StringStartswithSlice
        | OpKind::StringStrip
        | OpKind::StringUpper
        | OpKind::Sub
        | OpKind::Subscript
        | OpKind::Sum
        | OpKind::TaskWait
        | OpKind::TrueDiv
        | OpKind::Trunc
        | OpKind::TryEnd
        | OpKind::TryStart
        | OpKind::TupleCount
        | OpKind::TupleFromList
        | OpKind::TupleIndex
        | OpKind::TypeGuard
        | OpKind::UnaryInvert
        | OpKind::UnaryNeg
        | OpKind::UnaryNot
        | OpKind::UnaryOp
        | OpKind::UnaryPos
        | OpKind::Unbox
        | OpKind::UnboxToRawInt
        | OpKind::Unreachable
        | OpKind::VecMax
        | OpKind::VecMin
        | OpKind::VecProd
        | OpKind::VecSum
        | OpKind::WarnStderr
        | OpKind::WhileEnd
        | OpKind::WhileStart
        | OpKind::Yield
        | OpKind::YieldFrom
        | OpKind::Zip => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn family_dispatch_table_is_disjoint() {
        let mut seen: HashSet<OpKind> = HashSet::new();
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            for &kind in *kinds {
                assert!(
                    seen.insert(kind),
                    "kind `{kind}` claimed by more than one LLVM preserved-op family \
                     (last seen at {family:?})",
                );
            }
        }
    }

    #[test]
    fn each_family_row_is_non_empty() {
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            assert!(
                !kinds.is_empty(),
                "LLVM preserved-op family {family:?} has no routed kinds",
            );
        }
    }

    #[test]
    fn representative_kinds_route_to_their_owning_family() {
        assert_eq!(
            llvm_preserved_op_family(OpKind::Floordiv),
            Some(LlvmPreservedOpFamily::Direct),
        );
        assert_eq!(
            llvm_preserved_op_family(OpKind::DictNew),
            Some(LlvmPreservedOpFamily::Container),
        );
        assert_eq!(
            llvm_preserved_op_family(OpKind::FuncNew),
            Some(LlvmPreservedOpFamily::Callable),
        );
    }

    #[test]
    fn fixed_boxed_container_calls_bypass_dedicated_family_routing() {
        for (kind, arity) in [
            (OpKind::ListFromRange, 3),
            (OpKind::ListFillNew, 2),
            (OpKind::ListAppend, 2),
            (OpKind::ListExtend, 2),
            (OpKind::TupleFromList, 1),
            (OpKind::SetAdd, 2),
            (OpKind::SetAddProbe, 2),
            (OpKind::FrozensetAdd, 2),
            (OpKind::DictSet, 3),
            (OpKind::DictSetdefault, 3),
            (OpKind::DictSetdefaultEmptyList, 2),
            (OpKind::DictGet, 3),
            (OpKind::DictUnpack, 2),
            (OpKind::DictUpdate, 2),
            (OpKind::DictUpdateMissing, 3),
            (OpKind::DictUpdateKwstar, 2),
            (OpKind::DictClear, 1),
            (OpKind::DictCopy, 1),
            (OpKind::DictPopitem, 1),
            (OpKind::Slice, 3),
            (OpKind::SliceNew, 3),
            (OpKind::DictKeys, 1),
            (OpKind::DictValues, 1),
            (OpKind::DictItems, 1),
            (OpKind::Enumerate, 3),
            (OpKind::DictFromObj, 1),
        ] {
            assert_eq!(
                llvm_preserved_op_family(kind),
                None,
                "fixed boxed call `{kind}` must reach generated runtime ABI dispatch",
            );
            assert!(
                molt_ir::runtime_boxed_abi_generated::runtime_boxed_abi(
                    &format!("molt_{kind}"),
                    arity,
                )
                .is_some(),
                "deleted handler `{kind}` must have an exact generated boxed ABI",
            );
        }

        for kind in [
            OpKind::IterNextUnboxed,
            OpKind::Len,
            OpKind::ListNew,
            OpKind::DictNew,
            OpKind::TupleNew,
            OpKind::SetNew,
            OpKind::FrozensetNew,
            OpKind::Iter,
            OpKind::UnpackSequence,
        ] {
            assert_eq!(
                llvm_preserved_op_family(kind),
                Some(LlvmPreservedOpFamily::Container),
                "custom container protocol `{kind}` must keep dedicated lowering",
            );
        }
    }

    #[test]
    fn vector_reductions_use_the_shared_owned_boxed_abi() {
        use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_abi};

        for kind in [
            OpKind::VecSum,
            OpKind::VecProd,
            OpKind::VecMin,
            OpKind::VecMax,
        ] {
            assert_eq!(llvm_preserved_op_family(kind), None);
            let symbol = format!("molt_{kind}");
            let abi = runtime_boxed_abi(&symbol, 3)
                .unwrap_or_else(|| panic!("{kind} must have a three-object boxed ABI"));
            assert_eq!(abi.result, RuntimeBoxedReturn::OwnedValue);
            for arity in [0, 2, 4] {
                assert!(
                    runtime_boxed_abi(&symbol, arity).is_none(),
                    "{kind} must reject arity {arity}",
                );
            }
        }
    }

    #[test]
    fn exact_boxed_runtime_kinds_bypass_direct_and_callable_family_routing() {
        for (kind, arity) in [
            (OpKind::Aiter, 1),
            (OpKind::ContextExit, 2),
            (OpKind::ModuleNew, 1),
            (OpKind::ModuleCacheGet, 1),
            (OpKind::ModuleCacheSet, 2),
            (OpKind::ModuleCacheDel, 1),
            (OpKind::ModuleGetAttr, 2),
            (OpKind::ModuleImportFrom, 2),
            (OpKind::ModuleGetGlobal, 2),
            (OpKind::ModuleSetAttr, 3),
            (OpKind::ModuleDelGlobal, 2),
            (OpKind::ModuleDelGlobalIfPresent, 2),
            (OpKind::ExceptionClass, 1),
            (OpKind::ExceptionNew, 2),
            (OpKind::ExceptionStackSetDepth, 1),
            (OpKind::ExceptionStackExit, 1),
            (OpKind::ExceptionSetLast, 1),
            (OpKind::ExceptionContextSet, 1),
            (OpKind::StrFromObj, 1),
            (OpKind::ReprFromObj, 1),
            (OpKind::IntFromObj, 3),
            (OpKind::FloatFromObj, 1),
            (OpKind::AsciiFromObj, 1),
            (OpKind::ComplexFromObj, 3),
            (OpKind::IntFromStrOfObj, 3),
            (OpKind::Ord, 1),
            (OpKind::OrdAt, 2),
            (OpKind::StringJoin, 2),
            (OpKind::Isinstance, 2),
            (OpKind::Issubclass, 2),
            (OpKind::HasAttrName, 2),
            (OpKind::IsCallable, 1),
            (OpKind::CodeNew, 9),
            (OpKind::CallargsPushPos, 2),
            (OpKind::CallargsPushKw, 3),
            (OpKind::CallargsExpandStar, 2),
            (OpKind::CallargsExpandKwstar, 2),
        ] {
            assert_eq!(
                llvm_preserved_op_family(kind),
                None,
                "exact runtime kind `{kind}` must reach the admitted boxed-call route",
            );
            assert!(
                molt_ir::runtime_boxed_abi_generated::runtime_boxed_abi(
                    &format!("molt_{kind}"),
                    arity,
                )
                .is_some(),
                "retired handler `{kind}` must have an exact generated boxed ABI",
            );
        }
    }
}
