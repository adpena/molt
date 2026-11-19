//! The walk over a function's final TIR (design:
//! `docs/design/foundation/codegen_walker.md`, W2 and W3): the one dispatch
//! table from each opcode the walker owns to its lowering. A backend's own
//! walk asks it first and lowers only what it answers `false` for, until the
//! last family moves and both native backends walk through it alone.

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::CodegenBuilder;
use super::constants;
use super::function::FunctionCx;
use crate::tir::ops::{OpCode, TirOp};

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Lower `op` when the walker owns its opcode, and answer whether it
    /// did. An opcode it does not own yet is left to the backend.
    pub(crate) fn walker_owned_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        match op.opcode {
            OpCode::ConstInt
            | OpCode::ModuleSlot
            | OpCode::ConstFloat
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstStr
            | OpCode::ConstBigInt
            | OpCode::ConstBytes => constants::lower(self, b, op),
            // Arithmetic, comparison, bitwise, unary, identity, membership,
            // selection, truth and the late stage's word primitives.
            OpCode::Add
            | OpCode::InplaceAdd
            | OpCode::Sub
            | OpCode::InplaceSub
            | OpCode::Mul
            | OpCode::InplaceMul
            | OpCode::Div
            | OpCode::FloorDiv
            | OpCode::Mod
            | OpCode::Pow
            | OpCode::CheckedAdd
            | OpCode::CheckedMul
            | OpCode::Neg
            | OpCode::Pos
            | OpCode::Not
            | OpCode::BitNot
            | OpCode::Eq
            | OpCode::Ne
            | OpCode::Lt
            | OpCode::Le
            | OpCode::Gt
            | OpCode::Ge
            | OpCode::Is
            | OpCode::IsNot
            | OpCode::In
            | OpCode::NotIn
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::BitXor
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::And
            | OpCode::Or
            | OpCode::Bool
            | OpCode::IsInlineInt
            | OpCode::UnboxInlineInt
            | OpCode::IsFloatWord
            | OpCode::UnboxFloatWord
            | OpCode::IntToFloat
            | OpCode::IsBoolWord
            | OpCode::UnboxBoolWord
            | OpCode::BoxInlineInt => self.operator_op(b, op),
            OpCode::CodeIs => self.code_is_op(b, op),
            OpCode::DirectCall => self.guarded_direct_call_op(b, op),
            OpCode::CallRuntime => {
                let runtime = op
                    .runtime
                    .expect("verified: CallRuntime carries its runtime entry");
                match runtime.boxed_abi() {
                    Some(abi) => self.boxed_runtime_call(b, op, abi),
                    None => self.raw_runtime_call(b, op, runtime),
                }
            }
            OpCode::BoxVal => self.box_op(b, op),
            OpCode::UnboxVal => self.unbox_op(b, op),
            OpCode::IncRef => self.refcount_op(b, op, RefcountOperation::Retain),
            // Surviving Python lifetime boundaries share terminal release
            // with inserted drops. Raw carriers have no runtime heap owner.
            OpCode::DelBoundary | OpCode::DecRef => {
                self.refcount_op(b, op, RefcountOperation::Release)
            }
            OpCode::LoadAttr
            | OpCode::StoreAttr
            | OpCode::DelAttr
            | OpCode::Index
            | OpCode::StoreIndex
            | OpCode::DelIndex => self.object_op(b, op),
            OpCode::Call => self.call_op(b, op),
            // DeleteVar defines the local's new SSA value as the missing
            // sentinel; the drop fact plane owns the previous occupant.
            OpCode::DeleteVar => self.delete_var_op(b, op),
            OpCode::Alloc => self.alloc_op(b, op),
            OpCode::CallMethod => self.call_method_op(b, op),
            OpCode::LoadMethod | OpCode::LoadSuperMethod => self.load_method_op(b, op),
            OpCode::CallLoadedMethod => self.call_loaded_method_op(b, op),
            OpCode::CallargsPushMethodReceiver => self.push_method_receiver_op(b, op),
            OpCode::CallBuiltin => self.call_builtin_op(b, op),
            // Fused `ord(container[index])`.
            OpCode::OrdAt => self.ord_at_op(b, op),
            // Raw boxed frame allocation is not implemented: a naked word of
            // stack cannot stand in for a tagged object payload.
            OpCode::StackAlloc => {
                panic!(
                    "{}",
                    crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED
                );
            }
            // Stack storage is reclaimed by the epilogue: nothing to emit.
            OpCode::Free => {}
            OpCode::BuildList | OpCode::BuildDict | OpCode::BuildTuple | OpCode::BuildSlice => {
                self.container_op(b, op)
            }
            OpCode::BuildSet => self.set_aggregate_op(b, op, RuntimeFn::SetNew, RuntimeFn::SetAdd),
            OpCode::Raise => self.raise_op(b, op),
            OpCode::WarnStderr => self.warn_stderr_op(b, op),
            // A function object's defaults mutation stamp; it observes
            // mutable runtime state, so the read always survives.
            OpCode::FunctionDefaultsVersion => self.function_defaults_version_op(b, op),
            OpCode::ModuleImportClosed
            | OpCode::GlobalSlotBase
            | OpCode::GlobalSlotGet
            | OpCode::StaticCellBase
            | OpCode::StaticCellLoad
            | OpCode::StaticCellSet => self.global_op(b, op),
            // Region markers: control flow already carries their meaning.
            OpCode::TryStart | OpCode::TryEnd | OpCode::StateBlockStart | OpCode::StateBlockEnd => {
            }
            // The families the backends still lower themselves, until each
            // moves here.
            OpCode::AllocTask
            | OpCode::AsyncWorkPending
            | OpCode::CallNative
            | OpCode::CheckException
            | OpCode::ClosureLoad
            | OpCode::ClosureStore
            | OpCode::Copy
            | OpCode::ExceptionPending
            | OpCode::ForIter
            | OpCode::FrameContextSet
            | OpCode::FrameSetLocation
            | OpCode::GetIter
            | OpCode::ImportFrom
            | OpCode::IsPending
            | OpCode::IterNext
            | OpCode::IterNextUnboxed
            | OpCode::IterNextUnpack2
            | OpCode::ModuleCacheDel
            | OpCode::ModuleCacheGet
            | OpCode::ModuleCacheSet
            | OpCode::ModuleDelGlobal
            | OpCode::ModuleDelGlobalIfPresent
            | OpCode::ModuleGetAttr
            | OpCode::ModuleGetGlobal
            | OpCode::ModuleGetName
            | OpCode::ModuleImportFrom
            | OpCode::ModuleSetAttr
            | OpCode::NativeSymbolAddr
            | OpCode::ObjectNewBound
            | OpCode::RecursionAdmits
            | OpCode::RecursionCount
            | OpCode::RecursionRelease
            | OpCode::ScfFor
            | OpCode::ScfIf
            | OpCode::ScfWhile
            | OpCode::ScfYield
            | OpCode::StateSet
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::TaskWait
            | OpCode::UnpackSequence
            | OpCode::Yield
            | OpCode::YieldFrom => return false,
        }
        true
    }
}
