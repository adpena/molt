use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_ir::OpKind;

// Handler-owned authority for LLVM preserved SimpleIR ops lowered directly by
// this module. The dispatcher in `preserved_ops.rs` routes through this slice;
// audit tooling compares it with the adjacent `match kind` arms so routing and
// lowering cannot drift independently. A kind whose runtime entry is exactly
// `molt_<kind>` with a generated positional boxed-ABI row has no arm here: it
// takes the shared admitted boxed-call path.
// Kinds the walker owns (`codegen_ssa::preserved`) never reach this module.
define_llvm_op_kinds!(
    OpKind::CallAsync,
    OpKind::ExceptionLastPending,
    OpKind::ExceptionFinallyPendingObserver,
    OpKind::GuardType,
    OpKind::GuardTag,
);

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    fn runtime_guard_profile_flag(&mut self) -> inkwell::values::IntValue<'ctx> {
        if let Some(flag) = self.guard_profile_flag {
            return flag;
        }
        let builder = self.backend.context.create_builder();
        let entry = self.llvm_fn.get_first_basic_block().expect("LLVM entry");
        // The thread record publishes the process's profile flag with the
        // runtime binding. One activation observes it at entry, before any
        // guard or callback runs.
        if let Some(first) = entry.get_first_instruction() {
            builder.position_before(&first);
        } else {
            builder.position_at_end(entry);
        }
        let flag = self.backend.load_thread_record_word(
            &builder,
            molt_codegen_abi::thread_record::ThreadRecordWord::ProfileEnabled,
            "runtime_profile_enabled",
        );
        self.guard_profile_flag = Some(flag);
        flag
    }

    pub(super) fn lower_preserved_direct_op(&mut self, op: &TirOp, kind: OpKind) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            OpKind::CallAsync => self.lower_preserved_call_async_op(op),
            OpKind::ExceptionLastPending | OpKind::ExceptionFinallyPendingObserver => {
                let result = if op.is_async_work_poll() {
                    self.emit_async_work_poll(true, None)
                } else {
                    let last_fn = self.ensure_runtime_import(RuntimeFn::ExceptionLastPending);
                    self.backend
                        .builder
                        .build_execution_call(last_fn, &[], "exception_last_pending")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                };
                if let Some(&result_id) = op.results.first() {
                    self.values.set_value(result_id, result);
                    self.values.set_repr(
                        result_id,
                        crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
                    );
                }
                true
            }
            // Runtime mismatch only profiles and preserves the source. A tag
            // rejected by to_i64 can raise.
            OpKind::GuardType | OpKind::GuardTag => {
                if op.operands.len() != 2 {
                    return false;
                }
                let profile_join = if self.guard_facts.is_profile_only(op) {
                    let enabled = self.runtime_guard_profile_flag();
                    let enabled = self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            enabled,
                            i64_ty.const_zero(),
                            "guard_profile_enabled",
                        )
                        .unwrap();
                    let check = self
                        .backend
                        .context
                        .append_basic_block(self.llvm_fn, "guard_profile");
                    let join = self
                        .backend
                        .context
                        .append_basic_block(self.llvm_fn, "guard_done");
                    self.backend
                        .builder
                        .build_conditional_branch(enabled, check, join)
                        .unwrap();
                    self.backend.builder.position_at_end(check);
                    Some(join)
                } else {
                    None
                };
                let guard_fn = self.ensure_runtime_import(RuntimeFn::GuardType);
                self.emit_positional_runtime_call(
                    op,
                    guard_fn,
                    ResultCustody::SideEffect,
                    kind.as_str(),
                    "guard_type",
                );
                if let Some(join) = profile_join {
                    self.backend
                        .builder
                        .build_unconditional_branch(join)
                        .unwrap();
                    self.backend.builder.position_at_end(join);
                }
                let source = op.operands[0];
                let value = self.resolve(source);
                let ty = self.repr_ty(source);
                for &result in &op.results {
                    self.values.set_value(result, value);
                    self.values
                        .set_repr(result, crate::codegen_ssa::HeldCarrier::of(&ty.clone()));
                }
                true
            }
            _ => unreachable!("llvm preserved direct-op family routed unsupported kind `{kind}`"),
        }
    }
}
