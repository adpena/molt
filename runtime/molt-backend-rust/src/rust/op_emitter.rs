use super::emit_helpers::{
    arg0, args2, declare_molt_value, is_assignable_var, out_var, rust_clone, rust_slot_key,
    rust_string_literal, rust_value, var_ref,
};
use super::lowering::op_definition_vars;
use super::runtime_surface::runtime_value_call_for_kind;
use super::{RustBackend, rust_ident};
use crate::OpIR;
use std::collections::BTreeSet;

mod builtins;
mod calls;
mod control;
mod exceptions;
mod gaps;
mod modules;
mod numeric;
mod values;

impl RustBackend {
    // Op dispatch stays here; lowering families live in sibling modules.

    fn op_prefers_integer_runtime_lane(&self, op: &OpIR) -> bool {
        self.current_scalar_plan
            .as_ref()
            .is_some_and(|plan| plan.op_prefers_integer_runtime_lane(op))
    }

    pub(super) fn emit_unsupported_op(&mut self, op: &OpIR, reason: impl Into<String>) {
        let reason = reason.into();
        // Record failure at dispatch and deliberately emit no source.
        // `emit_source` is private and `compile_checked` rejects this record,
        // so no caller can observe either a partial program or a fabricated
        // value for an unsupported operation.
        self.unsupported_ops
            .push(format!("`{}` (rust backend): {reason}", op.kind));
    }

    pub(super) fn emit_op(&mut self, op: &OpIR) {
        for definition in op_definition_vars(op) {
            self.clear_alias(&definition);
        }

        if self.emit_op_literal(op) {
            return;
        }
        match op.kind {
            molt_ir::OpKind::ModuleSlot => self.emit_op_module_slot(op),
            molt_ir::OpKind::ConstNotImplemented => self.emit_op_const_not_implemented(op),
            molt_ir::OpKind::ConstEllipsis => self.emit_op_const_ellipsis(op),
            molt_ir::OpKind::Missing => self.emit_op_missing(op),
            molt_ir::OpKind::Box
            | molt_ir::OpKind::BoxFromRawInt
            | molt_ir::OpKind::Unbox
            | molt_ir::OpKind::UnboxToRawInt => self.emit_op_representation_copy(op),
            molt_ir::OpKind::Copy
            | molt_ir::OpKind::LoadLocal
            | molt_ir::OpKind::LoadVar
            | molt_ir::OpKind::CopyVar
            | molt_ir::OpKind::IdentityAlias
            | molt_ir::OpKind::BindingAlias => self.emit_op_local_copy(op),
            molt_ir::OpKind::StoreVar => self.emit_op_store_var(op),
            molt_ir::OpKind::Pos | molt_ir::OpKind::UnaryPos | molt_ir::OpKind::TypeGuard => {
                self.emit_op_local_copy(op)
            }
            molt_ir::OpKind::Load | molt_ir::OpKind::GuardedLoad => self.emit_op_load(op),
            molt_ir::OpKind::ClosureLoad => self.emit_op_closure_load(op),
            molt_ir::OpKind::StoreLocal => self.emit_op_store_local(op),
            molt_ir::OpKind::Store => self.emit_op_store(op),
            molt_ir::OpKind::ClosureStore => self.emit_op_closure_store(op),
            molt_ir::OpKind::Phi => self.emit_op_phi(op),
            molt_ir::OpKind::Add | molt_ir::OpKind::InplaceAdd | molt_ir::OpKind::BinopAdd => {
                self.emit_op_add(op)
            }
            molt_ir::OpKind::Sub | molt_ir::OpKind::InplaceSub | molt_ir::OpKind::BinopSub => {
                self.emit_op_sub(op)
            }
            molt_ir::OpKind::Mul | molt_ir::OpKind::InplaceMul | molt_ir::OpKind::BinopMul => {
                self.emit_op_mul(op)
            }
            molt_ir::OpKind::Div | molt_ir::OpKind::TrueDiv => self.emit_op_div(op),
            molt_ir::OpKind::FloorDiv
            | molt_ir::OpKind::Floordiv
            | molt_ir::OpKind::BinopFloorDiv => self.emit_op_floor_div(op),
            molt_ir::OpKind::Mod | molt_ir::OpKind::Modulo | molt_ir::OpKind::BinopMod => {
                self.emit_op_mod(op)
            }
            molt_ir::OpKind::Pow | molt_ir::OpKind::BinopPow => self.emit_op_pow(op),
            molt_ir::OpKind::Neg | molt_ir::OpKind::UnaryNeg => self.emit_op_neg(op),
            molt_ir::OpKind::UnaryNot | molt_ir::OpKind::Not => self.emit_op_unary_not(op),
            molt_ir::OpKind::Band | molt_ir::OpKind::BitAnd => self.emit_op_band(op),
            molt_ir::OpKind::Bor | molt_ir::OpKind::BitOr => self.emit_op_bor(op),
            molt_ir::OpKind::Bxor | molt_ir::OpKind::BitXor => self.emit_op_bxor(op),
            molt_ir::OpKind::Lshift | molt_ir::OpKind::Shl => self.emit_op_lshift(op),
            molt_ir::OpKind::Rshift | molt_ir::OpKind::Shr => self.emit_op_rshift(op),
            molt_ir::OpKind::Eq | molt_ir::OpKind::CmpEq => self.emit_op_eq(op),
            molt_ir::OpKind::Ne | molt_ir::OpKind::CmpNe => self.emit_op_ne(op),
            molt_ir::OpKind::Lt | molt_ir::OpKind::CmpLt => self.emit_op_lt(op),
            molt_ir::OpKind::Le | molt_ir::OpKind::CmpLe => self.emit_op_le(op),
            molt_ir::OpKind::Gt | molt_ir::OpKind::CmpGt => self.emit_op_gt(op),
            molt_ir::OpKind::Ge | molt_ir::OpKind::CmpGe => self.emit_op_ge(op),
            molt_ir::OpKind::Is | molt_ir::OpKind::IsNot => self.emit_op_is(op),
            molt_ir::OpKind::In | molt_ir::OpKind::NotIn => self.emit_op_in(op),
            molt_ir::OpKind::Contains => self.emit_op_contains(op),
            molt_ir::OpKind::And | molt_ir::OpKind::MAnd => self.emit_op_and(op),
            molt_ir::OpKind::Or => self.emit_op_or(op),
            molt_ir::OpKind::If => self.emit_op_if(op),
            molt_ir::OpKind::IfNot => self.emit_op_if_not(op),
            molt_ir::OpKind::Else => self.emit_op_else(op),
            molt_ir::OpKind::EndIf => self.emit_op_end_if(op),
            molt_ir::OpKind::LoopStart | molt_ir::OpKind::WhileStart => self.emit_op_loop_start(op),
            molt_ir::OpKind::LoopEnd | molt_ir::OpKind::WhileEnd => self.emit_op_loop_end(op),
            molt_ir::OpKind::LoopBreakIfFalse => self.emit_op_loop_break_if_false(op),
            molt_ir::OpKind::LoopBreakIfTrue => self.emit_op_loop_break_if_true(op),
            molt_ir::OpKind::LoopBreakIfException => self.emit_op_loop_break_if_exception(op),
            molt_ir::OpKind::LoopBreak => self.emit_op_loop_break(op),
            molt_ir::OpKind::LoopContinue
            | molt_ir::OpKind::LoopCarryUpdate
            | molt_ir::OpKind::LoopCarryInit => self.emit_op_loop_continue(op),
            molt_ir::OpKind::LoopIndexNext => self.emit_op_loop_index_next(op),
            molt_ir::OpKind::LoopIndexStart => self.emit_op_loop_index_start(op),
            molt_ir::OpKind::Iter => self.emit_op_iter(op),
            molt_ir::OpKind::IterNext => self.emit_op_iter_next(op),
            molt_ir::OpKind::ForRange => self.emit_op_for_range(op),
            molt_ir::OpKind::ForIter => self.emit_op_for_iter(op),
            molt_ir::OpKind::RangeNew => self.emit_op_range_new(op),
            molt_ir::OpKind::EndFor => self.emit_op_end_for(op),
            molt_ir::OpKind::Unreachable => {
                self.emit_line("unreachable!(\"entered unreachable Molt control flow\");")
            }
            molt_ir::OpKind::Ret => self.emit_op_return(op),
            molt_ir::OpKind::RetVoid => self.emit_op_ret_void(op),
            molt_ir::OpKind::Call | molt_ir::OpKind::CallFunc | molt_ir::OpKind::CallInternal => {
                self.emit_op_call(op)
            }
            molt_ir::OpKind::CallGuarded => self.emit_op_dynamic_call(op),
            molt_ir::OpKind::CallMethod => self.emit_op_call_method(op),
            molt_ir::OpKind::CallBind | molt_ir::OpKind::CallIndirect => self.emit_op_call_bind(op),
            molt_ir::OpKind::CallargsNew => self.emit_op_callargs_new(op),
            molt_ir::OpKind::CallargsPushPos => self.emit_op_callargs_push_pos(op),
            molt_ir::OpKind::CallargsExpandStar => self.emit_op_callargs_expand_star(op),
            molt_ir::OpKind::FuncNew | molt_ir::OpKind::FuncNewClosure => self.emit_op_func_new(op),
            molt_ir::OpKind::CodeNew => self.emit_op_code_new(op),
            molt_ir::OpKind::CodeSlotsInit => self.emit_op_code_slots_init(op),
            molt_ir::OpKind::CodeSlotSet => self.emit_op_code_slot_set(op),
            molt_ir::OpKind::ExceptionLast
            | molt_ir::OpKind::ExceptionLastPending
            | molt_ir::OpKind::ExceptionFinallyPendingObserver => self.emit_op_exception_last(op),
            molt_ir::OpKind::ExceptionStackDepth | molt_ir::OpKind::ExceptionStackEnter => {
                self.emit_op_exception_stack_depth(op)
            }
            molt_ir::OpKind::ExceptionClear => self.emit_op_exception_clear(op),
            molt_ir::OpKind::ExceptionStackExit => self.emit_op_exception_stack_exit(op),
            molt_ir::OpKind::ExceptionStackSetDepth => self.emit_op_exception_stack_set_depth(op),
            molt_ir::OpKind::ExceptionStackClear => self.emit_op_exception_stack_clear(op),
            molt_ir::OpKind::ExceptionSetLast => self.emit_op_exception_set_last(op),
            molt_ir::OpKind::ExceptionActive => self.emit_op_exception_active(op),
            molt_ir::OpKind::TraceEnterSlot => self.emit_op_trace_enter_slot(op),
            molt_ir::OpKind::TraceExit => self.emit_op_trace_exit(op),
            molt_ir::OpKind::FrameLocalsSet => self.emit_op_frame_locals_set(op),
            molt_ir::OpKind::FrameHomeStore
            | molt_ir::OpKind::FrameHomeCell
            | molt_ir::OpKind::FrameHomePrivateCell => self.emit_op_frame_home_store(op),
            molt_ir::OpKind::FrameHomeLoad => self.emit_op_frame_home_load(op),
            molt_ir::OpKind::FrameHomeTake => self.emit_op_frame_home_take(op),
            molt_ir::OpKind::FrameHomeClear => self.emit_op_frame_home_clear(op),
            molt_ir::OpKind::BuiltinFunc => self.emit_op_builtin_func(op),
            molt_ir::OpKind::Print | molt_ir::OpKind::BuiltinPrint => self.emit_op_print(op),
            molt_ir::OpKind::Len | molt_ir::OpKind::BuiltinLen => self.emit_op_len(op),
            molt_ir::OpKind::Float | molt_ir::OpKind::CastFloat | molt_ir::OpKind::BuiltinFloat => {
                self.emit_op_float(op)
            }
            molt_ir::OpKind::FloatFromObj => self.emit_op_float_from_obj(op),
            molt_ir::OpKind::Str | molt_ir::OpKind::CastStr | molt_ir::OpKind::BuiltinStr => {
                self.emit_op_str(op)
            }
            molt_ir::OpKind::Bool | molt_ir::OpKind::CastBool | molt_ir::OpKind::BuiltinBool => {
                self.emit_op_bool(op)
            }
            molt_ir::OpKind::Chr => self.emit_op_chr(op),
            molt_ir::OpKind::Ord => self.emit_op_ord(op),
            molt_ir::OpKind::OrdAt => self.emit_op_ord_at(op),
            molt_ir::OpKind::Abs | molt_ir::OpKind::BuiltinAbs => self.emit_op_abs(op),
            molt_ir::OpKind::BuildList | molt_ir::OpKind::ListNew | molt_ir::OpKind::Alloc => {
                self.emit_op_build_list(op)
            }
            molt_ir::OpKind::BuildDict | molt_ir::OpKind::DictNew => self.emit_op_build_dict(op),
            molt_ir::OpKind::ListAppend => self.emit_op_list_append(op),
            molt_ir::OpKind::GetItem | molt_ir::OpKind::Subscript | molt_ir::OpKind::Index => {
                self.emit_op_get_item(op)
            }
            molt_ir::OpKind::DictGet => self.emit_op_dict_get(op),
            molt_ir::OpKind::SetItem
            | molt_ir::OpKind::StoreSubscript
            | molt_ir::OpKind::StoreIndex => self.emit_op_set_item(op),
            molt_ir::OpKind::DictSet => self.emit_op_dict_set(op),
            molt_ir::OpKind::GetAttr | molt_ir::OpKind::LoadAttr => self.emit_op_get_attr(op),
            molt_ir::OpKind::LoadMethod => self.emit_op_load_method(op),
            molt_ir::OpKind::LoadSuperMethod => self.emit_unsupported_op(
                op,
                "zero-argument super() needs the CPython super object protocol",
            ),
            molt_ir::OpKind::CallLoadedMethod => self.emit_op_call_loaded_method(op),
            molt_ir::OpKind::GetAttrName => self.emit_op_get_attr_name(op),
            molt_ir::OpKind::GetAttrNameDefault => self.emit_op_get_attr_name_default(op),
            molt_ir::OpKind::SetAttr
            | molt_ir::OpKind::StoreAttr
            | molt_ir::OpKind::SetAttrGenericObj
            | molt_ir::OpKind::SetAttrGenericPtr => self.emit_op_set_attr(op),
            molt_ir::OpKind::Zip => self.emit_op_zip(op),
            molt_ir::OpKind::Sorted | molt_ir::OpKind::BuiltinSorted => self.emit_op_sorted(op),
            molt_ir::OpKind::Reversed | molt_ir::OpKind::BuiltinReversed => {
                self.emit_op_reversed(op)
            }
            molt_ir::OpKind::Any | molt_ir::OpKind::BuiltinAny => self.emit_op_any(op),
            molt_ir::OpKind::All | molt_ir::OpKind::BuiltinAll => self.emit_op_all(op),
            molt_ir::OpKind::ModuleNew => self.emit_op_module_new(op),
            molt_ir::OpKind::BoundMethodNew => self.emit_op_bound_method_new(op),
            molt_ir::OpKind::ModuleCacheGet | molt_ir::OpKind::ModuleLoadCached => {
                self.emit_op_module_cache_get(op)
            }
            molt_ir::OpKind::ModuleCacheSet => self.emit_op_module_cache_set(op),
            molt_ir::OpKind::ModuleCacheDel => self.emit_op_module_cache_del(op),
            molt_ir::OpKind::ModuleGetAttr | molt_ir::OpKind::ModuleGetName => {
                self.emit_op_module_get_attr(op)
            }
            molt_ir::OpKind::ModuleSetAttr => self.emit_op_module_set_attr(op),
            molt_ir::OpKind::Nop
            | molt_ir::OpKind::Line
            | molt_ir::OpKind::LoopIndexEnd
            | molt_ir::OpKind::DropInserted
            | molt_ir::OpKind::ExceptionRegionDropsInserted => self.emit_op_nop(op),
            molt_ir::OpKind::WarnStderr => self.emit_op_warn_stderr(op),
            molt_ir::OpKind::StrFromObj
            | molt_ir::OpKind::ReprFromObj
            | molt_ir::OpKind::AsciiFromObj => self.emit_op_runtime_value_call(op),
            molt_ir::OpKind::UnpackSequence => self.emit_op_unpack_sequence(op),
            molt_ir::OpKind::Aiter
            | molt_ir::OpKind::AllocClass
            | molt_ir::OpKind::AllocClassStatic
            | molt_ir::OpKind::AllocClassTrusted
            | molt_ir::OpKind::AllocInstance
            | molt_ir::OpKind::AllocTask
            | molt_ir::OpKind::Anext
            | molt_ir::OpKind::AsyncForEnd
            | molt_ir::OpKind::AsyncForStart
            | molt_ir::OpKind::AsyncWorkPoll
            | molt_ir::OpKind::AsyncgenNew
            | molt_ir::OpKind::Binop
            | molt_ir::OpKind::BitNot
            | molt_ir::OpKind::BoolConst
            | molt_ir::OpKind::Borrow
            | molt_ir::OpKind::BrIf
            | molt_ir::OpKind::Branch
            | molt_ir::OpKind::BranchFalse
            | molt_ir::OpKind::BranchTrue
            | molt_ir::OpKind::BuildSet
            | molt_ir::OpKind::BuildSlice
            | molt_ir::OpKind::BuildTuple
            | molt_ir::OpKind::BuiltinInt
            | molt_ir::OpKind::BuiltinRange
            | molt_ir::OpKind::BuiltinSum
            | molt_ir::OpKind::BuiltinType
            | molt_ir::OpKind::BytearrayCount
            | molt_ir::OpKind::BytearrayCountSlice
            | molt_ir::OpKind::BytearrayEndswith
            | molt_ir::OpKind::BytearrayEndswithSlice
            | molt_ir::OpKind::BytearrayFillRange
            | molt_ir::OpKind::BytearrayFind
            | molt_ir::OpKind::BytearrayFindSlice
            | molt_ir::OpKind::BytearrayFromObj
            | molt_ir::OpKind::BytearrayFromStr
            | molt_ir::OpKind::BytearrayReplace
            | molt_ir::OpKind::BytearraySplit
            | molt_ir::OpKind::BytearraySplitMax
            | molt_ir::OpKind::BytearrayStartswith
            | molt_ir::OpKind::BytearrayStartswithSlice
            | molt_ir::OpKind::BytesCount
            | molt_ir::OpKind::BytesCountSlice
            | molt_ir::OpKind::BytesEndswith
            | molt_ir::OpKind::BytesEndswithSlice
            | molt_ir::OpKind::BytesFind
            | molt_ir::OpKind::BytesFindSlice
            | molt_ir::OpKind::BytesFromObj
            | molt_ir::OpKind::BytesFromStr
            | molt_ir::OpKind::BytesReplace
            | molt_ir::OpKind::BytesSplit
            | molt_ir::OpKind::BytesSplitMax
            | molt_ir::OpKind::BytesStartswith
            | molt_ir::OpKind::BytesStartswithSlice
            | molt_ir::OpKind::CallAsync
            | molt_ir::OpKind::CallBuiltin
            | molt_ir::OpKind::CallFunction
            | molt_ir::OpKind::CallargsExpandKwstar
            | molt_ir::OpKind::CallargsPushKw
            | molt_ir::OpKind::CallargsPushMethodReceiver
            | molt_ir::OpKind::Cast
            | molt_ir::OpKind::CastInt
            | molt_ir::OpKind::CheckException
            | molt_ir::OpKind::CheckedAdd
            | molt_ir::OpKind::CheckedMul
            | molt_ir::OpKind::ClassDef
            | molt_ir::OpKind::ClassLayoutFieldCount
            | molt_ir::OpKind::ClassLayoutSlotCount
            | molt_ir::OpKind::ClassSetDefinition
            | molt_ir::OpKind::ClassmethodNew
            | molt_ir::OpKind::Compare
            | molt_ir::OpKind::ComplexFromObj
            | molt_ir::OpKind::Const
            | molt_ir::OpKind::ConstBigint
            | molt_ir::OpKind::ConstBool
            | molt_ir::OpKind::ConstBytes
            | molt_ir::OpKind::ConstFloat
            | molt_ir::OpKind::ConstInt
            | molt_ir::OpKind::ConstNone
            | molt_ir::OpKind::ConstStr
            | molt_ir::OpKind::ContextEnter
            | molt_ir::OpKind::ContextExit
            | molt_ir::OpKind::DataclassGet
            | molt_ir::OpKind::DataclassNew
            | molt_ir::OpKind::DataclassNewValues
            | molt_ir::OpKind::DataclassSet
            | molt_ir::OpKind::DataclassSetClass
            | molt_ir::OpKind::DecRef
            | molt_ir::OpKind::DelAttr
            | molt_ir::OpKind::DelAttrGenericObj
            | molt_ir::OpKind::DelAttrGenericPtr
            | molt_ir::OpKind::DelAttrName
            | molt_ir::OpKind::DelBoundary
            | molt_ir::OpKind::DelIndex
            | molt_ir::OpKind::DelItem
            | molt_ir::OpKind::DeleteVar
            | molt_ir::OpKind::DictClear
            | molt_ir::OpKind::DictCopy
            | molt_ir::OpKind::DictFromObj
            | molt_ir::OpKind::DictItems
            | molt_ir::OpKind::DictKeys
            | molt_ir::OpKind::DictPop
            | molt_ir::OpKind::DictPopitem
            | molt_ir::OpKind::DictSetdefault
            | molt_ir::OpKind::DictSetdefaultEmptyList
            | molt_ir::OpKind::DictStrIntInc
            | molt_ir::OpKind::DictUnpack
            | molt_ir::OpKind::DictUpdate
            | molt_ir::OpKind::DictUpdateKwstar
            | molt_ir::OpKind::DictUpdateMissing
            | molt_ir::OpKind::DictValues
            | molt_ir::OpKind::Enumerate
            | molt_ir::OpKind::ExceptEnd
            | molt_ir::OpKind::ExceptStart
            | molt_ir::OpKind::ExceptionClass
            | molt_ir::OpKind::ExceptionContextSet
            | molt_ir::OpKind::ExceptionCurrent
            | molt_ir::OpKind::ExceptionMatchBuiltin
            | molt_ir::OpKind::ExceptionNew
            | molt_ir::OpKind::ExceptionNewBuiltin
            | molt_ir::OpKind::ExceptionNewBuiltinEmpty
            | molt_ir::OpKind::ExceptionNewBuiltinOne
            | molt_ir::OpKind::ExceptionNewFromClass
            | molt_ir::OpKind::ExceptionPending
            | molt_ir::OpKind::ExceptionPop
            | molt_ir::OpKind::ExceptionPush
            | molt_ir::OpKind::ExceptiongroupCombine
            | molt_ir::OpKind::ExceptiongroupMatch
            | molt_ir::OpKind::FinallyEnd
            | molt_ir::OpKind::FinallyStart
            | molt_ir::OpKind::ForIterEnd
            | molt_ir::OpKind::ForIterStart
            | molt_ir::OpKind::FormatString
            | molt_ir::OpKind::FrameContextSet
            | molt_ir::OpKind::FrameLocals
            | molt_ir::OpKind::Free
            | molt_ir::OpKind::FrozensetAdd
            | molt_ir::OpKind::FrozensetNew
            | molt_ir::OpKind::FunctionClosureBits
            | molt_ir::OpKind::FunctionDefaultsVersion
            | molt_ir::OpKind::GenClose
            | molt_ir::OpKind::GenSend
            | molt_ir::OpKind::GenThrow
            | molt_ir::OpKind::GetAttrGenericObj
            | molt_ir::OpKind::GetAttrGenericPtr
            | molt_ir::OpKind::GetAttrSpecialObj
            | molt_ir::OpKind::GetIter
            | molt_ir::OpKind::Getframe
            | molt_ir::OpKind::Goto
            | molt_ir::OpKind::GpuBarrier
            | molt_ir::OpKind::GpuBlockDim
            | molt_ir::OpKind::GpuBlockId
            | molt_ir::OpKind::GpuGridDim
            | molt_ir::OpKind::GpuThreadId
            | molt_ir::OpKind::GuardBool
            | molt_ir::OpKind::GuardFloat
            | molt_ir::OpKind::GuardInt
            | molt_ir::OpKind::GuardLayout
            | molt_ir::OpKind::GuardNone
            | molt_ir::OpKind::GuardStr
            | molt_ir::OpKind::GuardTag
            | molt_ir::OpKind::GuardType
            | molt_ir::OpKind::GuardedFieldGet
            | molt_ir::OpKind::GuardedFieldSet
            | molt_ir::OpKind::HasAttrName
            | molt_ir::OpKind::Id
            | molt_ir::OpKind::Import
            | molt_ir::OpKind::ImportFrom
            | molt_ir::OpKind::ImportName
            | molt_ir::OpKind::IncRef
            | molt_ir::OpKind::IndexSet
            | molt_ir::OpKind::InitInstance
            | molt_ir::OpKind::InplaceBitAnd
            | molt_ir::OpKind::InplaceBitOr
            | molt_ir::OpKind::InplaceBitXor
            | molt_ir::OpKind::InplaceDiv
            | molt_ir::OpKind::InplaceFloordiv
            | molt_ir::OpKind::InplaceLshift
            | molt_ir::OpKind::InplaceMatmul
            | molt_ir::OpKind::InplaceMod
            | molt_ir::OpKind::InplacePow
            | molt_ir::OpKind::InplaceRshift
            | molt_ir::OpKind::InstanceGetField
            | molt_ir::OpKind::InstanceHasField
            | molt_ir::OpKind::InstanceSetField
            | molt_ir::OpKind::Int
            | molt_ir::OpKind::IntFromObj
            | molt_ir::OpKind::IntFromStrOfObj
            | molt_ir::OpKind::Invert
            | molt_ir::OpKind::InvokeFfi
            | molt_ir::OpKind::IsBoundMethod
            | molt_ir::OpKind::IsCallable
            | molt_ir::OpKind::IsGenerator
            | molt_ir::OpKind::IsNativeAwaitable
            | molt_ir::OpKind::IsPending
            | molt_ir::OpKind::Isinstance
            | molt_ir::OpKind::Issubclass
            | molt_ir::OpKind::IterNextUnboxed
            | molt_ir::OpKind::Jump
            | molt_ir::OpKind::Label
            | molt_ir::OpKind::ListClear
            | molt_ir::OpKind::ListCopy
            | molt_ir::OpKind::ListCount
            | molt_ir::OpKind::ListExtend
            | molt_ir::OpKind::ListFillNew
            | molt_ir::OpKind::ListFromRange
            | molt_ir::OpKind::ListIndex
            | molt_ir::OpKind::ListIndexRange
            | molt_ir::OpKind::ListInsert
            | molt_ir::OpKind::ListIntNew
            | molt_ir::OpKind::ListPop
            | molt_ir::OpKind::ListRemove
            | molt_ir::OpKind::ListReverse
            | molt_ir::OpKind::LoadConst
            | molt_ir::OpKind::Matmul
            | molt_ir::OpKind::Mod_
            | molt_ir::OpKind::ModuleDelGlobal
            | molt_ir::OpKind::ModuleDelGlobalIfPresent
            | molt_ir::OpKind::ModuleGetGlobal
            | molt_ir::OpKind::ModuleImport
            | molt_ir::OpKind::ModuleImportFrom
            | molt_ir::OpKind::ModuleImportStar
            | molt_ir::OpKind::NoneConst
            | molt_ir::OpKind::ObjectNew
            | molt_ir::OpKind::ObjectNewBound
            | molt_ir::OpKind::OperatorIndex
            | molt_ir::OpKind::PcallWrapBegin
            | molt_ir::OpKind::PcallWrapEnd
            | molt_ir::OpKind::PowMod
            | molt_ir::OpKind::PrintNewline
            | molt_ir::OpKind::PropertyNew
            | molt_ir::OpKind::Raise
            | molt_ir::OpKind::RaiseFrom
            | molt_ir::OpKind::Range
            | molt_ir::OpKind::Release
            | molt_ir::OpKind::Reraise
            | molt_ir::OpKind::Round
            | molt_ir::OpKind::SetAdd
            | molt_ir::OpKind::SetAddProbe
            | molt_ir::OpKind::SetAttrName
            | molt_ir::OpKind::SetClear
            | molt_ir::OpKind::SetDifferenceUpdate
            | molt_ir::OpKind::SetDiscard
            | molt_ir::OpKind::SetIntersectionUpdate
            | molt_ir::OpKind::SetNew
            | molt_ir::OpKind::SetPop
            | molt_ir::OpKind::SetRemove
            | molt_ir::OpKind::SetSymdiffUpdate
            | molt_ir::OpKind::SetUpdate
            | molt_ir::OpKind::Slice
            | molt_ir::OpKind::SliceNew
            | molt_ir::OpKind::StackAlloc
            | molt_ir::OpKind::StateBlockEnd
            | molt_ir::OpKind::StateBlockStart
            | molt_ir::OpKind::StateLabel
            | molt_ir::OpKind::StateSet
            | molt_ir::OpKind::StateSwitch
            | molt_ir::OpKind::StateTransition
            | molt_ir::OpKind::StateYield
            | molt_ir::OpKind::StatefulLocalsRegister
            | molt_ir::OpKind::StaticmethodNew
            | molt_ir::OpKind::StoreFast
            | molt_ir::OpKind::StringCapitalize
            | molt_ir::OpKind::StringConcat
            | molt_ir::OpKind::StringConst
            | molt_ir::OpKind::StringCount
            | molt_ir::OpKind::StringCountSlice
            | molt_ir::OpKind::StringEndswith
            | molt_ir::OpKind::StringEndswithSlice
            | molt_ir::OpKind::StringEq
            | molt_ir::OpKind::StringFind
            | molt_ir::OpKind::StringFindSlice
            | molt_ir::OpKind::StringFormat
            | molt_ir::OpKind::StringIndex
            | molt_ir::OpKind::StringIndexSlice
            | molt_ir::OpKind::StringJoin
            | molt_ir::OpKind::StringLower
            | molt_ir::OpKind::StringLstrip
            | molt_ir::OpKind::StringPartition
            | molt_ir::OpKind::StringRepeat
            | molt_ir::OpKind::StringReplace
            | molt_ir::OpKind::StringRfind
            | molt_ir::OpKind::StringRfindSlice
            | molt_ir::OpKind::StringRindex
            | molt_ir::OpKind::StringRindexSlice
            | molt_ir::OpKind::StringRpartition
            | molt_ir::OpKind::StringRstrip
            | molt_ir::OpKind::StringSplit
            | molt_ir::OpKind::StringSplitField
            | molt_ir::OpKind::StringSplitFieldEnd
            | molt_ir::OpKind::StringSplitFieldEq
            | molt_ir::OpKind::StringSplitFieldIsAscii
            | molt_ir::OpKind::StringSplitFieldLen
            | molt_ir::OpKind::StringSplitFieldLenFromBounds
            | molt_ir::OpKind::StringSplitFieldOrdAtBounds
            | molt_ir::OpKind::StringSplitFieldStart
            | molt_ir::OpKind::StringSplitFieldToInt
            | molt_ir::OpKind::StringSplitMax
            | molt_ir::OpKind::StringSplitSepDictInc
            | molt_ir::OpKind::StringSplitValidate
            | molt_ir::OpKind::StringSplitWsDictInc
            | molt_ir::OpKind::StringSplitlines
            | molt_ir::OpKind::StringStartswith
            | molt_ir::OpKind::StringStartswithSlice
            | molt_ir::OpKind::StringStrip
            | molt_ir::OpKind::StringUpper
            | molt_ir::OpKind::Sum
            | molt_ir::OpKind::TaskWait
            | molt_ir::OpKind::Trunc
            | molt_ir::OpKind::TryEnd
            | molt_ir::OpKind::TryStart
            | molt_ir::OpKind::TupleCount
            | molt_ir::OpKind::TupleFromList
            | molt_ir::OpKind::TupleIndex
            | molt_ir::OpKind::TupleNew
            | molt_ir::OpKind::TypeOf
            | molt_ir::OpKind::UnaryInvert
            | molt_ir::OpKind::UnaryOp
            | molt_ir::OpKind::VecMax
            | molt_ir::OpKind::VecMin
            | molt_ir::OpKind::VecProd
            | molt_ir::OpKind::VecSum
            | molt_ir::OpKind::Widen
            | molt_ir::OpKind::Yield
            | molt_ir::OpKind::YieldFrom => self.emit_op_other(op),
        }
    }
}
