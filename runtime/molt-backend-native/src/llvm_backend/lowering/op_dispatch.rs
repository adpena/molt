use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        match op.opcode {
            // -- Constants --
            OpCode::ConstInt
            | OpCode::ModuleSlot
            | OpCode::ConstFloat
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstStr
            | OpCode::ConstBigInt
            | OpCode::ConstBytes => {
                let (mut cx, mut b) = self.walker();
                crate::codegen_ssa::constants::lower(&mut cx, &mut b, op);
            }

            // -- Operators: arithmetic, comparison, bitwise, unary, identity,
            // membership, selection, truth and the late-TIR word primitives.
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
            | OpCode::BoxInlineInt => {
                let (mut cx, mut b) = self.walker();
                cx.operator_op(&mut b, op);
            }
            OpCode::CodeIs => self.walker_op(|cx, b| cx.code_is_op(b, op)),
            OpCode::DirectCall => self.walker_op(|cx, b| cx.guarded_direct_call_op(b, op)),
            OpCode::RecursionAdmits | OpCode::RecursionCount | OpCode::RecursionRelease => {
                self.emit_recursion_charge(op)
            }
            OpCode::CallRuntime => {
                let runtime = op
                    .runtime
                    .expect("verified: CallRuntime carries its runtime entry");
                match runtime.boxed_abi() {
                    Some(abi) => self.walker_op(|cx, b| cx.boxed_runtime_call(b, op, abi)),
                    None => self.walker_op(|cx, b| cx.raw_runtime_call(b, op, runtime)),
                }
            }
            OpCode::NativeSymbolAddr => self.emit_native_symbol_addr(op),
            OpCode::CallNative => self.emit_call_native(op),

            // -- Box/Unbox --
            OpCode::BoxVal => self.emit_box(op),
            OpCode::UnboxVal => self.emit_unbox(op),

            // -- Refcount --
            OpCode::IncRef => self.emit_refcount(
                op,
                molt_codegen_abi::inline_refcount::RefcountOperation::Retain,
            ),
            // Surviving Python lifetime boundaries share terminal release
            // with inserted drops. Raw carriers have no runtime heap owner.
            OpCode::DelBoundary | OpCode::DecRef => self.emit_refcount(
                op,
                molt_codegen_abi::inline_refcount::RefcountOperation::Release,
            ),

            // -- Memory / Attribute / Index --
            OpCode::LoadAttr
            | OpCode::StoreAttr
            | OpCode::DelAttr
            | OpCode::Index
            | OpCode::StoreIndex
            | OpCode::DelIndex => {
                let (mut cx, mut b) = self.walker();
                cx.object_op(&mut b, op);
            }

            // -- Call --
            OpCode::Call => self.walker_op(|cx, b| cx.call_op(b, op)),

            // -- DeleteVar local-slot transition --
            // DeleteVar defines the local's new SSA value as the missing sentinel
            // operand. Ownership of the previous slot occupant is modeled by the
            // drop fact plane (DecRef / explicit consumed operands), not by LLVM
            // lowering.
            OpCode::DeleteVar => self.walker_op(|cx, b| cx.delete_var_op(b, op)),

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

            OpCode::Alloc => self.walker_op(|cx, b| cx.alloc_op(b, op)),

            // -- Calls --
            OpCode::CallMethod => self.walker_op(|cx, b| cx.call_method_op(b, op)),

            OpCode::LoadMethod | OpCode::LoadSuperMethod => {
                self.walker_op(|cx, b| cx.load_method_op(b, op))
            }

            OpCode::CallLoadedMethod => self.walker_op(|cx, b| cx.call_loaded_method_op(b, op)),

            OpCode::CallargsPushMethodReceiver => {
                self.walker_op(|cx, b| cx.push_method_receiver_op(b, op))
            }

            OpCode::CallBuiltin => self.walker_op(|cx, b| cx.call_builtin_op(b, op)),

            // -- OrdAt: fused ord(container[index]) --
            OpCode::OrdAt => self.walker_op(|cx, b| cx.ord_at_op(b, op)),

            // Raw boxed frame allocation is not implemented. In particular,
            // a naked i64 alloca cannot stand in for a tagged object payload.
            OpCode::StackAlloc => {
                panic!(
                    "{}",
                    crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED
                );
            }

            // -- Free: stack-allocated slots are freed automatically — no-op --
            OpCode::Free => {
                // Stack memory is reclaimed by the function epilogue; nothing to emit.
            }

            // -- BuildList: [item0, item1, ...] --
            // Borrow the fixed operand range for one constructor transaction.
            OpCode::BuildList => self.walker_op(|cx, b| cx.container_op(b, op)),

            // -- BuildDict: {k0: v0, k1: v1, ...} --
            // operands: [k0, v0, k1, v1, ...]  (pairs)
            OpCode::BuildDict => self.walker_op(|cx, b| cx.container_op(b, op)),

            // -- BuildTuple: (item0, item1, ...) --
            OpCode::BuildTuple => self.walker_op(|cx, b| cx.container_op(b, op)),

            // -- BuildSet: {item0, item1, ...} --
            OpCode::BuildSet => self.walker_op(|cx, b| {
                cx.set_aggregate_op(b, op, RuntimeFn::SetNew, RuntimeFn::SetAdd)
            }),

            // -- BuildSlice: slice(start, stop, step) --
            // operands: [start, stop, step]   (already declared as molt_slice_new)
            OpCode::BuildSlice => self.walker_op(|cx, b| cx.container_op(b, op)),

            // -- GetIter: iter(obj) --
            OpCode::GetIter => self.emit_get_iter(op),

            // -- IterNext: next(iter) -> value (or StopIteration sentinel) --
            OpCode::IterNext => self.emit_iter_next(op),

            // -- ForIter: advance iterator, returning next value or exhaustion sentinel --
            OpCode::ForIter => self.emit_for_iter(op),

            // -- Async / generator / channel state machine --
            OpCode::AllocTask => self.emit_alloc_task(op),
            OpCode::StateSwitch => self.emit_state_switch(),
            OpCode::ClosureLoad => self.emit_closure_load(op),
            OpCode::ClosureStore => self.emit_closure_store(op),
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
            OpCode::Raise => self.walker_op(|cx, b| cx.raise_op(b, op)),

            OpCode::WarnStderr => self.walker_op(|cx, b| cx.warn_stderr_op(b, op)),

            // -- ExceptionPending: read the runtime exception-pending flag as
            //    an exact i1 boolean (`molt_exception_pending() != 0`). Produced
            //    by `loop_break_if_exception` and consumed as the condition of
            //    the loop-exit CondBranch that breaks an iterator-consumer loop
            //    on a mid-iteration raise.  Non-foldable: it observes mutable
            //    runtime state, so the value (and the break) always survive.
            OpCode::ExceptionPending => {
                let raw = self.emit_exception_pending_word("exc_pending");
                if let Some(&result_id) = op.results.first() {
                    let pending = self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            raw,
                            self.backend.context.i64_type().const_zero(),
                            "exc_pending_bool",
                        )
                        .unwrap();
                    self.values.set_value(result_id, pending.into());
                    self.values.set_repr(
                        result_id,
                        crate::codegen_ssa::HeldCarrier::of(&TirType::Bool),
                    );
                }
            }

            // -- AsyncWorkPending: test the thread's pending-work word. It
            //    services nothing; a promoted module loop branches on it to
            //    the unpromoted loop, whose poll runs the work.
            OpCode::AsyncWorkPending => {
                let pending = self.emit_async_work_pending();
                if let Some(&result_id) = op.results.first() {
                    self.values.set_value(result_id, pending.into());
                    self.values.set_repr(
                        result_id,
                        crate::codegen_ssa::HeldCarrier::of(&TirType::Bool),
                    );
                }
            }

            // -- FunctionDefaultsVersion: read a function object's
            //    __defaults__/__kwdefaults__ mutation version stamp as a boxed
            //    inline int (`molt_function_defaults_version(func)`).  Produced
            //    by the compile-time defaults-devirt deopt guard and consumed by
            //    its `== 0` compare (baked literal vs live read).  Non-foldable:
            //    it observes mutable runtime state, so the read always survives.
            OpCode::FunctionDefaultsVersion => {
                self.walker_op(|cx, b| cx.function_defaults_version_op(b, op))
            }

            // -- CheckException: transfer to its explicit exceptional successor --
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

            // -- ModuleImportClosed: a Fixed row's import by id --
            OpCode::ModuleImportClosed => self.walker_op(|cx, b| cx.global_op(b, op)),

            // -- ImportFrom: from module import name --
            // operands: [module, attr_name]
            OpCode::ImportFrom => self.emit_module_runtime_call(op, RuntimeFn::ModuleGetAttr, 2),

            // -- ModuleCacheGet: module-cache lookup by name --
            // operands: [module_name]
            OpCode::ModuleCacheGet => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleCacheGet, 1)
            }

            // -- ModuleCacheSet: module-cache mutation by name --
            // operands: [module_name, module]
            OpCode::ModuleCacheSet => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleCacheSet, 2)
            }

            // -- ModuleCacheDel: module-cache deletion by name --
            // operands: [module_name]
            OpCode::ModuleCacheDel => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleCacheDel, 1)
            }

            // -- ModuleGetAttr: module attribute read --
            // operands: [module, attr_name]
            OpCode::ModuleGetAttr => self.emit_module_runtime_call(op, RuntimeFn::ModuleGetAttr, 2),

            // -- ModuleImportFrom: `from M import name` binding --
            // operands: [module, attr_name]. CPython IMPORT_FROM semantics:
            // ImportError (not AttributeError) on miss, with a sys.modules
            // submodule fallback (see molt_module_import_from).
            OpCode::ModuleImportFrom => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleImportFrom, 2)
            }

            // -- ModuleGetGlobal: CPython-style module global lookup --
            // operands: [module, global_name]
            OpCode::ModuleGetGlobal => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleGetGlobal, 2)
            }
            // -- Module global slot sites (call_anatomy.md 4.5) --
            OpCode::GlobalSlotBase => self.walker_op(|cx, b| cx.global_op(b, op)),
            OpCode::GlobalSlotGet => self.walker_op(|cx, b| cx.global_op(b, op)),
            OpCode::StaticCellBase => self.walker_op(|cx, b| cx.global_op(b, op)),
            OpCode::StaticCellLoad => self.walker_op(|cx, b| cx.global_op(b, op)),
            OpCode::StaticCellSet => self.walker_op(|cx, b| cx.global_op(b, op)),

            // -- ModuleGetName: module name/attribute lookup helper --
            // operands: [module, attr_name]
            OpCode::ModuleGetName => self.emit_module_runtime_call(op, RuntimeFn::ModuleGetName, 2),

            // -- ModuleSetAttr: module attribute mutation --
            // operands: [module, attr_name, value]; the module dict retains the
            // value, so a box minted for a raw value is released after the store.
            OpCode::ModuleSetAttr => self.emit_module_runtime_call(op, RuntimeFn::ModuleSetAttr, 3),

            // -- ModuleDelGlobal: CPython-style module global deletion --
            // operands: [module, global_name]
            OpCode::ModuleDelGlobal => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleDelGlobal, 2)
            }
            OpCode::ModuleDelGlobalIfPresent => {
                self.emit_module_runtime_call(op, RuntimeFn::ModuleDelGlobalIfPresent, 2)
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

            // Path-local region metadata does not mutate runtime state. The
            // preserved exception_push/pop and stack operations own that state;
            // explicit exception edges own transfer, independent of block order.
            OpCode::TryStart | OpCode::TryEnd => {}
            OpCode::IterNextUnboxed => self.emit_iter_next_unboxed(op),
            OpCode::IterNextUnpack2 => self.emit_iter_next_unpack2(op),
            OpCode::UnpackSequence => {
                assert!(
                    self.lower_preserved_container_op(op, OpKind::UnpackSequence),
                    "verified UnpackSequence must lower through the canonical runtime ABI"
                );
            }
            OpCode::ObjectNewBound => {
                let Some(&class_id) = op.operands.first() else {
                    panic!("{:?} requires class operand", op.opcode);
                };
                let new_fn = self.ensure_runtime_import(RuntimeFn::ObjectNewBound);
                self.emit_borrowed_runtime_call(
                    op,
                    new_fn,
                    &[RuntimeArg::Operand(class_id)],
                    ResultCustody::canonical(RuntimeFn::ObjectNewBound, 1),
                    "object_new_bound",
                    "object_new_bound",
                );
            }
            OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }

    /// Module and import operations are positional boxed runtime calls with
    /// canonical rows: every operand is borrowed through one custody and the
    /// owned result is bound or released.
    fn emit_module_runtime_call(&mut self, op: &TirOp, function: RuntimeFn, arity: usize) {
        let callee = self.ensure_runtime_import(function);
        let args: Vec<RuntimeArg<'ctx>> = op.operands[..arity]
            .iter()
            .map(|&operand| RuntimeArg::Operand(operand))
            .collect();
        self.emit_borrowed_runtime_call(
            op,
            callee,
            &args,
            ResultCustody::canonical(function, arity),
            "module_call",
            function.symbol(),
        );
    }
}
