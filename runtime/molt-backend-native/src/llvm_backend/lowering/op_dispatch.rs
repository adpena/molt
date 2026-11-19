use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        // The walker's dispatch first (`FunctionCx::walker_owned_op`); the
        // arms below are the families not yet moved into it.
        if self.walker_owned(source_block, op) {
            return;
        }
        match op.opcode {
            // -- SSA Copy --
            // Also serves as the fallback for unknown frontend ops that were
            // mapped to Copy by the SSA converter.  Handle all combinations of
            // operand/result counts gracefully:
            //   - 0 operands, 0 results: no-op (side-effect only)
            //   - 0 operands, 1+ results: produce NaN-boxed None per result
            //   - 1+ operands, 0 results: no-op (side-effect only)
            //   - 1+ operands, 1+ results: pass-through first operand
            OpCode::FrameSetLocation => {
                assert!(self.lower_preserved_simpleir_op(op, OpKind::FrameSetLocation));
            }
            OpCode::Copy => {
                let original_kind = op.origin;
                if let Some(kind) = original_kind
                    && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
                // A preserved op no arm claimed is refused; a genuine SSA copy
                // passes operand 0 through (`FunctionCx::copy_op`).
                self.walker_op(|cx, b| cx.copy_op(b, op));
            }

            // -- Async / generator / channel state machine --
            OpCode::AllocTask => self.emit_alloc_task(op),
            OpCode::StateSwitch => self.emit_state_switch(),
            OpCode::FrameContextSet => self.emit_frame_context_set(op),
            OpCode::StateYield | OpCode::StateTransition => panic!(
                "{}: the shared terminal drop pass must expose it as explicit activation exits before LLVM emission",
                self.func.name
            ),
            OpCode::StateSet => self.emit_state_set(op),
            OpCode::IsPending => self.emit_is_pending(op),
            OpCode::TaskWait => self.emit_task_wait(op),
            OpCode::Yield => self.emit_yield(op),
            OpCode::YieldFrom => self.emit_yield_from(op),

            // -- CheckException: the walker lowers a plain check's unwind edge;
            //    this arm keeps the async-work poll and the origin edge, which
            //    need the frame's homes (the switch lane's frame family).
            OpCode::CheckException => {
                let Some(target_label) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    self.record_fatal(
                        "check_exception requires an explicit exceptional successor label",
                    );
                    return;
                };
                let result = if op.is_async_work_poll() {
                    self.emit_async_work_poll(false, op.frame_location())
                } else {
                    self.emit_exception_pending_word("check_exc").into()
                };
                if let Some(&result_id) = op.results.first() {
                    self.values.set_value(result_id, result);
                    self.values.set_repr(
                        result_id,
                        crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
                    );
                }
                let Some(target_block_id) = self.exception_target(target_label) else {
                    self.record_fatal(format!(
                        "check_exception target label {} has no unique block in the label map",
                        target_label
                    ));
                    return;
                };
                let Some(&target_bb) = self.block_map.get(&target_block_id) else {
                    self.record_fatal(format!(
                        "check_exception target block {:?} is not present in LLVM block map",
                        target_block_id
                    ));
                    return;
                };
                let continue_bb = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("check_exc_cont{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let pending = self.ensure_i64(result);
                let has_exception = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        pending,
                        self.backend.context.i64_type().const_zero(),
                        "check_exc_pending",
                    )
                    .unwrap();
                let edge = op
                    .exception_edge_kind()
                    .expect("verified exceptional edge kind");
                match edge {
                    molt_ir::tir::op_kinds_generated::ExceptionEdgeKind::Origin => {
                        let origin = self.backend.context.append_basic_block(
                            self.llvm_fn,
                            &format!("check_exc_origin{}", self.synthetic_block_counter),
                        );
                        self.synthetic_block_counter += 1;
                        self.backend
                            .builder
                            .build_conditional_branch(has_exception, origin, continue_bb)
                            .unwrap();
                        self.backend.builder.position_at_end(origin);
                        if let Some(location) = op.frame_location() {
                            // The site's own LocationId; the normal path stores nothing.
                            self.store_frame_location(location);
                        }
                        let homes = if let Some(slot) = self.frame_homes {
                            self.backend
                                .builder
                                .build_load(self.backend.context.i64_type(), slot, "origin_homes")
                                .unwrap()
                                .into_int_value()
                        } else {
                            self.backend.context.i64_type().const_zero()
                        };
                        let service = self.ensure_runtime_import(RuntimeFn::ExceptionFrameOrigin);
                        self.backend
                            .builder
                            .build_execution_call(service, &[homes.into()], "origin_capture")
                            .unwrap();
                        self.jump_with_args(
                            source_block,
                            "check-exception-origin-edge",
                            target_block_id,
                            &op.operands,
                        );
                    }
                    molt_ir::tir::op_kinds_generated::ExceptionEdgeKind::Unwind => {
                        self.branch_with_args_if(
                            has_exception,
                            source_block,
                            "check-exception-unwind-edge",
                            target_bb,
                            target_block_id,
                            &op.operands,
                            continue_bb,
                        );
                    }
                }
                self.backend.builder.position_at_end(continue_bb);
            }

            // -- SCF dialect ops --
            // These must reach LLVM as real CFG, loop, and phi structure. The
            // old runtime-delegate names were never exported by molt-runtime, so
            // keeping them as declarations converted missing lowering into link
            // failures. Fail closed here until the structured lowering pass owns
            // the full CFG form.
            OpCode::ScfIf => self.record_unlowered_structured_op(
                op,
                "lower structured if/else regions into LLVM CFG before codegen",
            ),
            OpCode::ScfFor => self.record_unlowered_structured_op(
                op,
                "lower counted or iterator loops into LLVM CFG before codegen",
            ),
            OpCode::ScfWhile => self.record_unlowered_structured_op(
                op,
                "lower while regions into LLVM CFG before codegen",
            ),
            OpCode::ScfYield => self.record_unlowered_structured_op(
                op,
                "thread SCF region results through block arguments or phi nodes before codegen",
            ),

            _ => unreachable!("the walker lowers {:?}", op.opcode),
        }
    }
}
