use super::*;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeFn;

// Handler-owned routing slice. `code_new` and the CallArgs push/expand
// steps are exact `molt_<kind>` boxed-ABI calls and take the shared admitted
// boxed-call path instead of an arm here.
define_llvm_op_kinds!(
    OpKind::BuiltinFunc,
    OpKind::FuncNew,
    OpKind::FuncNewClosure,
    OpKind::FunctionClosureBits,
    OpKind::AsyncgenNew,
    OpKind::CodeSlotSet,
    OpKind::CodeSlotsInit,
    OpKind::TraceEnterSlot,
    OpKind::TraceExit,
    OpKind::FrameHomeStore,
    OpKind::FrameHomeCell,
    OpKind::FrameHomePrivateCell,
    OpKind::FrameHomeLoad,
    OpKind::FrameHomeTake,
    OpKind::FrameHomeClear,
    OpKind::FrameSetLocation,
    OpKind::FrameLocals,
    OpKind::FrameLocalsSet,
    OpKind::Line,
    OpKind::CallargsNew,
);

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(in crate::llvm_backend::lowering) fn lend_chunk_frame_homes(&mut self) {
        let Some(slot) = self.frame_homes else { return };
        let frame = self.owned_frame_entered;
        if frame.is_some_and(|owned| owned.slot == slot) {
            return;
        }
        let abi = require_function_linkage_abi(self.func, self.backend);
        let (mut cx, mut b) = self.walker();
        crate::codegen_ssa::frame::lend_homes(&mut b, slot, |b| {
            crate::codegen_ssa::returns::emit_return(&mut cx, b, &abi, None, frame.as_ref())
        });
    }

    pub(in crate::llvm_backend::lowering) fn store_frame_location(&mut self, location: i64) {
        let homes = self
            .frame_homes
            .expect("location publication requires a frame");
        crate::codegen_ssa::frame::store_location(&mut self.codegen(), homes, location);
    }

    pub(super) fn lower_preserved_callable_op(&mut self, op: &TirOp, kind: OpKind) -> bool {
        if matches!(
            kind,
            OpKind::FunctionClosureBits
                | OpKind::AsyncgenNew
                | OpKind::CodeSlotSet
                | OpKind::CodeSlotsInit
                | OpKind::CallargsNew
                | OpKind::Line
        ) {
            let mut lowered = false;
            self.walker_op(|cx, b| lowered = cx.callable_runtime_op(b, op, kind));
            return lowered;
        }
        match kind {
            OpKind::BuiltinFunc | OpKind::FuncNew | OpKind::FuncNewClosure => {
                // A task constructor's target and task trampoline are the
                // switch lane's (`ensure_callable_target`); the walker builds
                // everything else.
                let task_entry = if op.attrs.contains_key("task_kind")
                    || op.attrs.contains_key("task_closure_size")
                {
                    let Some(symbol) = self.callee_symbol(op) else {
                        return false;
                    };
                    let arity = match op.attrs.get("value") {
                        Some(AttrValue::Int(value)) => usize::try_from(*value).unwrap_or(0),
                        _ => 0,
                    };
                    let has_closure = kind == OpKind::FuncNewClosure;
                    Some(self.ensure_callable_target(op, &symbol, arity, has_closure))
                } else {
                    None
                };
                let mut lowered = false;
                self.walker_op(|cx, b| {
                    lowered = cx.function_construction_op(b, op, kind, task_entry);
                });
                lowered
            }
            OpKind::TraceEnterSlot => {
                let frame = self
                    .owned_frame_entered
                    .expect("trace_enter_slot requires local execution-context ownership");
                let code_id = self.program_slot_id(molt_ir::code_slots::ProgramSlotKind::Code, op);
                let base = self.emit_frame_record_push(code_id, frame.kind);
                self.backend.builder.build_store(frame.slot, base).unwrap();
                true
            }
            OpKind::TraceExit => {
                // The invocation's guarded return owns the pop, including
                // generated suspension and early-error returns.
                true
            }
            OpKind::FrameHomeStore
            | OpKind::FrameHomeCell
            | OpKind::FrameHomePrivateCell
            | OpKind::FrameHomeClear
            | OpKind::FrameHomeTake
            | OpKind::FrameHomeLoad
            | OpKind::FrameSetLocation => {
                let (mut cx, mut b) = self.walker();
                cx.frame_home_op(&mut b, op)
            }
            OpKind::FrameLocals => {
                // `locals()`: the runtime's one authority over the executing
                // frame.
                let locals_fn = self.ensure_runtime_import(RuntimeFn::LocalsBuiltin);
                self.emit_borrowed_runtime_call(
                    op,
                    locals_fn,
                    &[],
                    ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind.as_str(),
                    "frame_locals",
                );
                true
            }
            OpKind::FrameLocalsSet => {
                let Some(&dict_id) = op.operands.first() else {
                    return false;
                };
                let frame_locals_fn = self.ensure_runtime_import(RuntimeFn::FrameLocalsSet);
                self.emit_borrowed_runtime_call(
                    op,
                    frame_locals_fn,
                    &[RuntimeArg::Operand(dict_id)],
                    ResultCustody::SideEffect,
                    kind.as_str(),
                    "frame_locals_set",
                );
                true
            }
            // A statement marker: it emits no code. Tracebacks read the
            // location that `frame_set_location` publishes before each
            // raising op.
            _ => unreachable!("preserved callable kind predicate accepted unknown kind {kind}"),
        }
    }
}
