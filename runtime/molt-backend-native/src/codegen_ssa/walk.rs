//! The walk over a function's final TIR (design:
//! `docs/design/foundation/codegen_walker.md`, W2 and W3): the one dispatch
//! table from each opcode the walker owns to its lowering. A backend's own
//! walk asks it first and lowers only what it answers `false` for, until the
//! last family moves and both native backends walk through it alone.

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::runtime_fn_generated::RuntimeFn;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use molt_ir::deterministic_hash::HashMap;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::runtime_callable_symbols::RuntimeFnSet;

use super::builder::CodegenBuilder;
use super::builder::{IntComparison, MachineKind, RecursionCharge, SiteBody};
use super::control::{self, Edges};
use super::custody::{CallTarget, RuntimeArg};
use super::frame::RecordPlacement;
use super::function::{FunctionCx, ResultCustody};
use super::value_table::{HeldCarrier, ValueTable};
use super::{constants, pending, returns};
use crate::representation_plan::ProducerCarrierPlan;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};

/// Where in the walk an op lowers: its block and the function's blocks,
/// for the ops that end their block with an edge.
pub(crate) struct Position<'a, B: CodegenBuilder> {
    pub(crate) source: BlockId,
    pub(crate) blocks: &'a HashMap<BlockId, B::Block>,
    /// Each exceptional successor label's block, from the one shared
    /// resolver (`exception_labels_to_blocks`), which refuses an ambiguous
    /// label instead of picking one in hash order.
    pub(crate) exception_targets: &'a HashMap<i64, BlockId>,
}

impl<B: RecordPlacement> FunctionCx<'_, B> {
    /// Lower `op`, at `at`, when the walker owns its opcode, and answer
    /// whether it did. An opcode it does not own yet is left to the backend.
    pub(crate) fn walker_owned_op(
        &mut self,
        b: &mut B,
        at: &Position<'_, B>,
        op: &TirOp,
        frame: Option<returns::OwnedFrame<B::Slot>>,
    ) -> bool {
        match op.opcode {
            OpCode::ExceptionPending => self.exception_pending_op(b, op),
            OpCode::AsyncWorkPending => {
                let pending = b.async_work_pending();
                if let Some(&result) = op.results.first() {
                    let pending = b.bit_value(pending);
                    self.bind(result, pending, HeldCarrier::RawBool);
                }
            }
            OpCode::CheckException => return self.check_exception_op(b, at, op),
            OpCode::Copy if op.origin == Some(molt_ir::OpKind::CallAsync) => {
                if !self.call_async(b, op) {
                    self.fatal("call_async requires a result and a valid task constructor");
                }
            }
            OpCode::AllocTask => self.alloc_task(b, op),
            OpCode::FrameContextSet => self.frame_context_set(b, op),
            OpCode::IsPending => self.is_pending(b, op),
            OpCode::StateSet | OpCode::TaskWait => {
                let Some(frame) = super::state::poll_frame(b) else {
                    self.fatal(format!(
                        "{:?} requires exactly one word-sized poll frame parameter",
                        op.opcode
                    ));
                    return true;
                };
                if op.opcode == OpCode::StateSet {
                    self.state_set(b, op, frame);
                } else {
                    self.task_wait(b, op, frame);
                }
            }
            // Module and import operations: positional borrowed runtime calls
            // with canonical rows.
            OpCode::ImportFrom | OpCode::ModuleGetAttr => {
                self.module_call(b, op, RuntimeFn::ModuleGetAttr, 2)
            }
            OpCode::ModuleCacheGet => self.module_call(b, op, RuntimeFn::ModuleCacheGet, 1),
            OpCode::ModuleCacheSet => self.module_call(b, op, RuntimeFn::ModuleCacheSet, 2),
            OpCode::ModuleCacheDel => self.module_call(b, op, RuntimeFn::ModuleCacheDel, 1),
            OpCode::ModuleImportFrom => self.module_call(b, op, RuntimeFn::ModuleImportFrom, 2),
            OpCode::ModuleGetGlobal => self.module_call(b, op, RuntimeFn::ModuleGetGlobal, 2),
            OpCode::ModuleGetName => self.module_call(b, op, RuntimeFn::ModuleGetName, 2),
            OpCode::ModuleSetAttr => self.module_call(b, op, RuntimeFn::ModuleSetAttr, 3),
            OpCode::ModuleDelGlobal => self.module_call(b, op, RuntimeFn::ModuleDelGlobal, 2),
            OpCode::ModuleDelGlobalIfPresent => {
                self.module_call(b, op, RuntimeFn::ModuleDelGlobalIfPresent, 2)
            }
            OpCode::ClosureLoad | OpCode::ClosureStore => self.closure_slot_op(b, op),
            // A direct call's inline recursion charge.
            OpCode::RecursionAdmits | OpCode::RecursionCount | OpCode::RecursionRelease => {
                let thread = b.thread_record_word();
                let charge = match op.opcode {
                    OpCode::RecursionAdmits => RecursionCharge::Admits,
                    OpCode::RecursionCount => RecursionCharge::Count,
                    OpCode::RecursionRelease => RecursionCharge::Release,
                    other => unreachable!("{other:?} is not a recursion charge op"),
                };
                if let Some(admitted) = b.recursion_charge(charge, thread) {
                    let admitted = b.bit_value(admitted);
                    self.bind(op.results[0], admitted, HeldCarrier::RawBool);
                }
            }
            OpCode::UnpackSequence => assert!(
                self.unpack_sequence_op(b, op),
                "verified UnpackSequence must lower through the canonical runtime ABI"
            ),
            OpCode::ObjectNewBound => {
                let Some(&class) = op.operands.first() else {
                    panic!("{:?} requires class operand", op.opcode);
                };
                self.object_alloc(b, op, Some(class));
            }
            OpCode::GetIter => {
                let call = molt_ir::runtime_calls::RuntimeCall::from_opcode(op.opcode)
                    .expect("typed runtime-call contract");
                self.contract_runtime_call(b, op, call);
            }
            // `next(iterator)`: the next value, or the exhaustion sentinel.
            OpCode::IterNext | OpCode::ForIter => {
                let abi = RuntimeFn::IterNext
                    .boxed_abi()
                    .filter(|abi| abi.arity() == 1)
                    .expect("molt_iter_next must have a generated boxed ABI");
                self.boxed_runtime_call(b, op, abi);
            }
            OpCode::IterNextUnboxed => assert!(
                self.iterator_out_words(b, op, RuntimeFn::IterNextUnboxed, 1, "iter_next_unboxed"),
                "IterNextUnboxed requires exactly one iterator operand"
            ),
            OpCode::IterNextUnpack2 => assert!(
                op.results.len() == 3
                    && self.iterator_out_words(
                        b,
                        op,
                        RuntimeFn::IterNextUnpack2,
                        2,
                        "iter_next_unpack2"
                    ),
                "IterNextUnpack2 requires one iterator operand and three results"
            ),
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
            OpCode::FunctionClosure => self.function_closure_op(b, op),
            OpCode::CallRuntime => {
                let runtime = op
                    .runtime
                    .expect("verified: CallRuntime carries its runtime entry");
                match runtime {
                    RuntimeFn::CpythonAbiRunStaticExtensionInit | RuntimeFn::NativeForwardF32 => {
                        self.native_transaction(b, op, runtime);
                    }
                    _ => match runtime.boxed_abi() {
                        Some(abi) => self.boxed_runtime_call(b, op, abi),
                        None => self.raw_runtime_call(b, op, runtime),
                    },
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
            OpCode::Call if op.origin == Some(molt_ir::OpKind::InvokeFfi) => {
                self.invoke_ffi_op(b, op)
            }
            OpCode::Call => self.call_op(b, op),
            OpCode::StateYield => {
                self.fatal("terminal drop insertion must expose it as explicit activation exits")
            }
            // DeleteVar defines the local's new SSA value as the missing
            // sentinel; the drop fact plane owns the previous occupant.
            OpCode::DeleteVar => self.delete_var_op(b, op),
            OpCode::Alloc => self.alloc_op(b, op),
            OpCode::CallMethod => {
                if let Some(builtin) = op.runtime {
                    if !self.runtime.contains(builtin) {
                        self.fatal(format!(
                            "bound builtin {} is unavailable in this runtime",
                            builtin.symbol()
                        ));
                    } else {
                        let result = self.operands().bound_builtin_call(b, op, builtin);
                        let value = b.word_value(result);
                        self.bind_owned(b, op, value);
                    }
                } else {
                    self.call_method_op(b, op);
                }
            }
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
            OpCode::FrameSetLocation => return self.frame_op(b, op, frame),
            OpCode::Copy => {
                if !self.frame_op(b, op, frame)
                    && !self.preserved_op(b, op)
                    && !op
                        .origin
                        .is_some_and(|kind| self.preserved_runtime_call(b, op, kind))
                {
                    self.copy_op(b, op);
                }
            }
            OpCode::NativeSymbolAddr => self.native_symbol_addr_op(b, op),
            OpCode::CallNative => self.call_native_op(b, op),
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
            OpCode::ScfFor
            | OpCode::ScfIf
            | OpCode::ScfWhile
            | OpCode::ScfYield
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::Yield
            | OpCode::YieldFrom => return false,
        }
        true
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// `ExceptionPending`: whether an exception is pending, as an exact
    /// raw bool. It observes mutable runtime state, so it never folds.
    fn exception_pending_op(&mut self, b: &mut B, op: &TirOp) {
        let thread = b.thread_record_word();
        let word = pending::exception_pending_word(b, thread);
        let named = b.word_value(word);
        b.set_name(named, "exc_pending_word");
        if let Some(&result) = op.results.first() {
            let zero = b.constant(0);
            let pending = b.int_compare(IntComparison::NotEqual, word, zero);
            let pending = b.bit_value(pending);
            b.set_name(pending, "exc_pending_bool");
            self.bind(result, pending, HeldCarrier::RawBool);
        }
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// `CheckException`: when an exception is pending, take the op's
    /// explicit exceptional successor with its operands as arguments. An
    /// async-work poll services pending work first and answers its own
    /// pending word. An unwind edge branches straight to the successor; an
    /// origin edge first publishes the site's location and captures the
    /// frame's traceback entry (`molt_exception_frame_origin`), which reads
    /// the frame's homes, or none when the function has none.
    fn check_exception_op(&mut self, b: &mut B, at: &Position<'_, B>, op: &TirOp) -> bool {
        use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;
        let Some(&AttrValue::Int(label)) = op.attrs.get("value") else {
            self.fatal("check_exception requires an explicit exceptional successor label");
            return true;
        };
        let pending = if op.is_async_work_poll() {
            self.async_work_poll(b, false, op.frame_location())
        } else {
            let thread = b.thread_record_word();
            pending::exception_pending_word(b, thread)
        };
        let named = b.word_value(pending);
        b.set_name(named, "check_exc_word");
        if let Some(&result) = op.results.first() {
            self.bind(result, named, HeldCarrier::Boxed);
        }
        let Some(&target) = at.exception_targets.get(&label) else {
            self.fatal(format!(
                "check_exception target label {label} has no unique block in the label map"
            ));
            return true;
        };
        let Some(&target_block) = at.blocks.get(&target) else {
            self.fatal(format!(
                "check_exception target block {target:?} is not present in the block map"
            ));
            return true;
        };
        let edge = op
            .exception_edge_kind()
            .expect("verified exceptional edge kind");
        let continued = b.create_block("check_exc_cont");
        let zero = b.constant(0);
        let raised = b.int_compare(IntComparison::NotEqual, pending, zero);
        let named = b.bit_value(raised);
        b.set_name(named, "check_exc_pending");
        let mut diagnostics = Vec::new();
        match edge {
            ExceptionEdgeKind::Unwind => {
                let passed = self.edges(at).arguments(
                    b,
                    at.source,
                    "check-exception-unwind-edge",
                    target,
                    &op.operands,
                    &mut diagnostics,
                );
                b.branch_if(raised, (target_block, &passed), (continued, &[]));
            }
            ExceptionEdgeKind::Origin => {
                let origin = b.create_block("check_exc_origin");
                b.branch_if(raised, (origin, &[]), (continued, &[]));
                b.switch_to_block(origin);
                let homes = b.frame_homes();
                // The site's own location; the normal path stores nothing.
                if let Some(location) = op.frame_location() {
                    let homes = homes.expect("location publication requires a frame");
                    super::frame::store_location(b, homes, location);
                }
                let homes = match homes {
                    Some(slot) => b.load_slot(slot),
                    None => b.constant(0),
                };
                let capture = b.runtime_callee(RuntimeFn::ExceptionFrameOrigin);
                let homes = b.word_value(homes);
                b.call(capture, &[homes]);
                let passed = self.edges(at).arguments(
                    b,
                    at.source,
                    "check-exception-origin-edge",
                    target,
                    &op.operands,
                    &mut diagnostics,
                );
                b.jump(target_block, &passed);
            }
        }
        for diagnostic in diagnostics {
            self.fatal(diagnostic);
        }
        b.switch_to_block(continued);
        true
    }

    /// The edges of the function being walked, at `at`.
    fn edges<'e>(&'e self, at: &'e Position<'_, B>) -> Edges<'e, B> {
        Edges {
            func: self.func,
            plan: self.plan,
            values: self.values,
            blocks: at.blocks,
        }
    }

    /// Service pending async work, then answer the pending-exception word: a
    /// cold call when the thread's work word is set (it publishes the
    /// frame's `location` first, since serviced work is Python code that can
    /// read this frame), else the thread record's exception byte. With
    /// `payload` the answer is the pending exception itself, or None.
    pub(crate) fn async_work_poll(
        &mut self,
        b: &mut B,
        payload: bool,
        location: Option<i64>,
    ) -> B::Word {
        let has_work = b.async_work_pending();
        let thread = b.thread_record_word();
        let cold = b.create_block("async_poll_cold");
        let fast = b.create_block("async_poll_fast");
        let done = b.create_block("async_poll_done");
        let answer = b.append_block_param(done, MachineKind::Word);
        b.branch_if(has_work, (cold, &[]), (fast, &[]));
        b.switch_to_block(cold);
        if let Some(location) = location {
            let homes = b
                .frame_homes()
                .expect("location publication requires a frame");
            super::frame::store_location(b, homes, location);
        }
        let service = b.runtime_callee(if payload {
            RuntimeFn::AsyncWorkPollAndExceptionLastPending
        } else {
            RuntimeFn::AsyncWorkPollAndExceptionPending
        });
        let serviced = b
            .call_cold(service, &[])
            .expect("the async-work service answers a word");
        b.jump(done, &[serviced]);
        b.switch_to_block(fast);
        let byte = b.load_byte(
            thread,
            molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET,
        );
        let byte = b.zero_extend(byte);
        if payload {
            let capture = b.create_block("async_poll_capture_exception");
            let zero = b.constant(0);
            let raised = b.int_compare(IntComparison::NotEqual, byte, zero);
            let none = b.constant(molt_codegen_abi::QNAN_TAG_NONE_I64 as u64);
            b.branch_if(raised, (capture, &[]), (done, &[b.word_value(none)]));
            b.switch_to_block(capture);
            let observed = b
                .call_runtime(RuntimeFn::ExceptionLastPending, &[])
                .expect("the pending-exception reader answers a word");
            b.jump(done, &[b.word_value(observed)]);
        } else {
            b.jump(done, &[b.word_value(byte)]);
        }
        b.switch_to_block(done);
        b.expect_word(answer)
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// A module or import operation: a positional boxed runtime call of
    /// `function` over the first `arity` operands, each borrowed through one
    /// custody; the owned result is bound or released.
    fn module_call(&mut self, b: &mut B, op: &TirOp, function: RuntimeFn, arity: usize) {
        let callee = b.runtime_callee(function);
        let args: Vec<RuntimeArg<B::Value>> = op.operands[..arity]
            .iter()
            .map(|&operand| RuntimeArg::Operand(operand))
            .collect();
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            &args,
            ResultCustody::canonical(function, arity),
            "module_call",
            function.symbol(),
        );
    }

    /// `ClosureLoad` and `ClosureStore`: the shared closure slot bodies
    /// address the owner's payload directly, so the owner's first word is an
    /// object address (a boxed pointer unboxed; a poll frame's raw `self`
    /// unchanged), and the slot's byte offset is the op's `value`. A load is
    /// returned retained; a store retains the borrowed value and answers
    /// None.
    fn closure_slot_op(&mut self, b: &mut B, op: &TirOp) {
        let owner = self.operands().resolve(b, op.operands[0]);
        let owner = super::scalar::word_of(b, owner);
        let owner = b.and(owner, molt_codegen_abi::POINTER_MASK);
        let offset = match op.attrs.get("value") {
            Some(&AttrValue::Int(offset)) => offset as u64,
            _ => 0,
        };
        let offset = b.constant(offset);
        let admission = b.refcount_admission();
        if op.opcode == OpCode::ClosureLoad {
            let args = [
                b.word_value(owner),
                b.word_value(offset),
                b.word_value(admission),
            ];
            let loaded = b.site(SiteBody::ClosureLoad, &args);
            b.set_name(loaded, "closure_load");
            self.bind_owned(b, op, loaded);
        } else {
            let words = [owner, offset].map(|word| RuntimeArg::Word(b.word_value(word)));
            let admission = RuntimeArg::Word(b.word_value(admission));
            self.runtime_call(
                b,
                op,
                CallTarget::Site(SiteBody::ClosureStore),
                &[
                    words[0],
                    words[1],
                    RuntimeArg::Operand(op.operands[1]),
                    admission,
                ],
                ResultCustody::Unowned,
                "closure_store",
                "closure_store",
            );
        }
    }
}

/// What a backend lends the walk for one function.
pub(crate) struct FunctionInput<'a, B: CodegenBuilder> {
    pub(crate) func: &'a TirFunction,
    pub(crate) plan: &'a ProducerCarrierPlan,
    /// The function's physical parameters after the execution context, in
    /// its linkage row's order.
    pub(crate) entry: &'a [B::Value],
    /// Each compiled function's frozen linkage row, by linker symbol; the
    /// function's own row declares its parameters and result.
    pub(crate) linkage: &'a BTreeMap<String, NativeFunctionLinkageAbi>,
    pub(crate) functions: &'a molt_ir::FunctionTable,
    pub(crate) runtime: &'a RuntimeFnSet,
}

/// A daemon may serve requests with different instrumentation settings.
fn trace_stride(name: &str) -> Option<usize> {
    use molt_ir::backend_environment::{self, BackendEnv};
    let setting = backend_environment::var(BackendEnv::MOLT_TRACE_OP_PROGRESS).ok()?;
    let (filter, stride) = setting
        .trim()
        .split_once(':')
        .unwrap_or((setting.trim(), ""));
    let filter = filter.trim();
    if filter.is_empty()
        || !(filter == "1" || filter.eq_ignore_ascii_case("all") || name.contains(filter))
    {
        return None;
    }
    Some(
        stride
            .trim()
            .parse()
            .ok()
            .filter(|&stride| stride > 0)
            .unwrap_or(5000),
    )
}

/// Lower `input.func` through `b`, which stands in the function's empty
/// physical entry block. The physical entry converts each parameter from
/// its linkage carrier to the TIR entry's carrier and jumps to the TIR
/// entry, so the TIR entry may have predecessors on every backend. Blocks
/// are walked in reverse postorder; each takes its parameters before any
/// branch to it is built. An op the walker does not own refuses the
/// function with a diagnostic naming its opcode, as does any other refusal.
pub(crate) fn lower_function<B: super::frame::RecordPlacement>(
    b: &mut B,
    input: FunctionInput<'_, B>,
) -> Result<(), Vec<String>> {
    let FunctionInput {
        func,
        plan,
        entry,
        linkage,
        functions,
        runtime,
    } = input;
    // A refusal before the walk still leaves a well-formed function: its
    // physical entry ends on a path that never executes.
    let refuse = |b: &mut B, diagnostics: Vec<String>| {
        b.unreachable();
        Err(diagnostics)
    };
    if let Err(errors) = crate::tir::verify::verify_operation_shapes(func) {
        let diagnostics = errors
            .into_iter()
            .map(|error| format!("{}: {error}", func.name))
            .collect();
        return refuse(b, diagnostics);
    }
    let layout = match super::frame::FrameLayout::for_function(func) {
        Ok(layout) => layout,
        Err(message) => return refuse(b, vec![message]),
    };
    if !func.blocks.contains_key(&func.entry_block) {
        let message = format!(
            "{}: entry block {:?} is missing from TIR block map",
            func.name, func.entry_block
        );
        return refuse(b, vec![message]);
    }
    let Some(own) = linkage.get(&func.name) else {
        let message = format!(
            "{}: the function has no exact native linkage ABI",
            func.name
        );
        return refuse(b, vec![message]);
    };
    // The TIR entry first, then every other block by id.
    let mut ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    ids.sort_by_key(|id| (*id != func.entry_block, id.0));
    let blocks: HashMap<BlockId, B::Block> = ids
        .iter()
        .map(|&id| (id, b.create_block(&format!("bb{}", id.0))))
        .collect();
    let rpo = crate::tir::dominators::executable_reverse_postorder(func);
    let mut values = ValueTable::with_capacity(func.next_value);
    for &id in &rpo {
        for arg in &func.blocks[&id].args {
            let held = HeldCarrier::of(&plan.effective_value_type(arg.id, &arg.ty));
            let value = b.append_block_param(blocks[&id], control::carrier_kind(held));
            values.set_value(arg.id, value);
            values.set_repr(arg.id, held);
        }
    }
    let diagnostics = RefCell::new(Vec::new());
    let fatal = |message: String| {
        diagnostics
            .borrow_mut()
            .push(format!("{}: {message}", func.name))
    };
    // The physical entry: each parameter in the TIR entry's carrier.
    let entry_block = &func.blocks[&func.entry_block];
    let resume_phis = entry_block.args.len() > entry.len()
        && func
            .blocks
            .values()
            .any(|block| block.terminator.has_successor(func.entry_block));
    let mut entry_args = Vec::with_capacity(entry_block.args.len());
    for (index, arg) in entry_block.args.iter().enumerate() {
        let effective = plan.effective_value_type(arg.id, &arg.ty);
        let kind = control::carrier_kind(HeldCarrier::of(&effective));
        let value = match (entry.get(index), own.param_types.get(index)) {
            (Some(&param), Some(ty)) => {
                control::coerce(b, plan, param, HeldCarrier::of(ty), None, &effective)
                    .unwrap_or_else(|| {
                        fatal(format!(
                            "entry parameter {index} of linkage type {ty:?} cannot hold %{} as {effective:?}",
                            arg.id.0
                        ));
                        b.placeholder(kind)
                    })
            }
            // Initial invocation has no predecessor value for these resume
            // phis. Each real backedge passes its own typed values.
            (None, None) if resume_phis => b.placeholder(kind),
            _ => {
                fatal(format!(
                    "entry block argument %{} at index {index} has no corresponding function parameter",
                    arg.id.0
                ));
                b.placeholder(kind)
            }
        };
        entry_args.push(value);
    }
    let frame = super::frame::initialize_owned_execution_frame(b, func.execution_context, || {
        layout.entry.kind()
    });
    let call_sites = Cell::new(0);
    let homes = match (layout.homes, layout.entry) {
        (None, _) => None,
        (Some(_), super::frame::FrameEntry::Absent) => {
            let slot = b.entry_slot("borrowed_frame_homes");
            let mut cx = FunctionCx {
                func,
                plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime,
                linkage,
                functions,
                call_sites: &call_sites,
            };
            super::frame::lend_homes(b, slot, |b| {
                returns::emit_return(&mut cx, b, own, None, frame.as_ref())
            });
            Some(slot)
        }
        (Some(_), _) => frame.as_ref().map(|owned| owned.slot),
    };
    b.set_frame_homes(homes);
    let trace = trace_stride(&func.name).map(|stride| {
        use molt_ir::backend_environment::{self, BackendEnv};
        let verbose = backend_environment::var(BackendEnv::MOLT_TRACE_OP_PROGRESS_STDERR)
            .as_deref()
            == Ok("1");
        let name = b.bytes_constant(func.name.as_bytes(), "__molt_trace_name_", "");
        let name = b.address_to_word(name);
        let len = b.constant(func.name.len() as u64);
        (stride, verbose, name, len)
    });
    let mut op_index = 0usize;
    b.jump(blocks[&func.entry_block], &entry_args);
    let exception_targets = crate::tir::dominators::exception_labels_to_blocks(&func.label_id_map);
    for &id in &rpo {
        b.switch_to_block(blocks[&id]);
        let block = &func.blocks[&id];
        let at = Position {
            source: id,
            blocks: &blocks,
            exception_targets: &exception_targets,
        };
        let mut cx = FunctionCx {
            func,
            plan,
            values: &mut values,
            diagnostics: &diagnostics,
            runtime,
            linkage,
            functions,
            call_sites: &call_sites,
        };
        for op in &block.ops {
            if let Some((stride, verbose, name, len)) = trace
                && op_index.is_multiple_of(stride)
            {
                if verbose {
                    eprintln!(
                        "[molt-native-op] func={} op={op_index} kind={:?} block={id:?} filled=false",
                        func.name, op.opcode
                    );
                }
                let index = b.constant(op_index as u64);
                b.call_runtime(RuntimeFn::DebugTrace, &[name, len, index]);
            }
            op_index += 1;
            if !cx.walker_owned_op(b, &at, op, frame) {
                cx.fatal(format!("the walker does not lower {:?} yet", op.opcode));
            }
        }
        let mut edge_diagnostics = Vec::new();
        let edges = Edges {
            func,
            plan,
            values: cx.values,
            blocks: &blocks,
        };
        if edges.terminator(b, id, &block.terminator, &mut edge_diagnostics) {
            diagnostics.borrow_mut().extend(edge_diagnostics);
            continue;
        }
        match &block.terminator {
            Terminator::Return { values: returned } => {
                returns::emit_return(&mut cx, b, own, returned.first().copied(), frame.as_ref());
            }
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => match super::state::poll_frame(b) {
                Some(frame) => {
                    edges.dispatch_state(
                        b,
                        id,
                        frame,
                        cases,
                        (*default, default_args),
                        &mut edge_diagnostics,
                    );
                }
                None => {
                    cx.fatal("a state dispatch needs the poll's frame parameter");
                    b.unreachable();
                }
            },
            other => unreachable!("Edges lowers {other:?}"),
        }
        diagnostics.borrow_mut().extend(edge_diagnostics);
    }
    // A block the walk never reaches still ends in a terminator.
    for id in &ids {
        if !rpo.contains(id) {
            b.switch_to_block(blocks[id]);
            b.unreachable();
        }
    }
    let diagnostics = diagnostics.into_inner();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    //! Whole functions through the walk, compiled as Cranelift machine code
    //! and called. The oracle is each function's Python meaning.

    use super::*;
    use crate::codegen_ssa::scalar::tests::Value;
    use crate::native_backend::codegen_builder::machine::{
        ThreadState, run_function, run_function_in,
    };
    use crate::tir::blocks::TirBlock;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    /// The test function's own linkage row: boxed parameters and a boxed result.
    fn row(func: &TirFunction) -> BTreeMap<String, NativeFunctionLinkageAbi> {
        let abi = NativeFunctionLinkageAbi {
            source_signature: crate::ir::ExternFunctionSignature {
                arity: func.param_types.len(),
                has_closure: false,
                returns_value: true,
                execution_context: func.execution_context,
            },
            parameter_custody: Vec::new(),
            param_types: vec![TirType::DynBox; func.param_types.len()],
            return_type: Some(TirType::DynBox),
        };
        BTreeMap::from([(func.name.clone(), abi)])
    }

    /// Walk `func` and call it on `args`; the fact is the walk's verdict.
    fn walk(
        func: &TirFunction,
        args: &[u64],
    ) -> crate::codegen_ssa::differential::Run<Result<(), Vec<String>>> {
        walk_in(func, args, ThreadState::default())
    }

    fn walk_in(
        func: &TirFunction,
        args: &[u64],
        state: ThreadState,
    ) -> crate::codegen_ssa::differential::Run<Result<(), Vec<String>>> {
        let linkage = row(func);
        let plan = ProducerCarrierPlan::default();
        let runtime = crate::codegen_ssa::differential::every_runtime_entry();
        let functions = molt_ir::FunctionTable::default();
        run_function_in(args, state, |b, params| {
            lower_function(
                b,
                FunctionInput {
                    func,
                    plan: &plan,
                    entry: params,
                    linkage: &linkage,
                    functions: &functions,
                    runtime: &runtime,
                },
            )
        })
    }

    fn op(opcode: OpCode, operands: &[u32], results: &[u32], value: Option<i64>) -> TirOp {
        let mut attrs = AttrDict::default();
        if let Some(value) = value {
            attrs.insert("value".into(), AttrValue::Int(value));
        }
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: operands.iter().map(|&id| ValueId(id)).collect(),
            results: results.iter().map(|&id| ValueId(id)).collect(),
            attrs,
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        }
    }

    #[test]
    fn a_parameter_crosses_a_block_parameter_to_the_return() {
        let mut func = TirFunction::new(
            "walk_identity".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let next = func.fresh_block();
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: next,
            args: vec![ValueId(0)],
        };
        let arg = func.fresh_value();
        func.value_types.insert(arg, TirType::DynBox);
        func.blocks.insert(
            next,
            TirBlock {
                id: next,
                args: vec![TirValue {
                    id: arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![arg] },
            },
        );
        let run = walk(&func, &[OBJECT]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(run.results, [Value::Word(OBJECT)]);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_resume_entry_takes_its_extra_phi_from_the_backedge() {
        let mut func = TirFunction::new(
            "resume_entry".into(),
            vec![TirType::DynBox; 2],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let entry = func.entry_block;
        let seed = func.fresh_block();
        let finished = func.fresh_block();
        let carried = func.fresh_value();
        let stop = func.fresh_value();
        func.value_types.insert(carried, TirType::DynBox);
        func.value_types.insert(stop, TirType::Bool);
        let block = func.blocks.get_mut(&entry).unwrap();
        block.args.push(TirValue {
            id: carried,
            ty: TirType::DynBox,
        });
        block.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: seed,
            then_args: vec![],
            else_block: finished,
            else_args: vec![],
        };
        func.blocks.insert(
            seed,
            TirBlock {
                id: seed,
                args: vec![],
                ops: vec![op(OpCode::ConstBool, &[], &[stop.0], Some(0))],
                terminator: Terminator::Branch {
                    target: entry,
                    args: vec![stop, ValueId(1), ValueId(1)],
                },
            },
        );
        func.blocks.insert(
            finished,
            TirBlock {
                id: finished,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![carried],
                },
            },
        );
        let yes = molt_codegen_abi::QNAN | molt_codegen_abi::TAG_BOOL | 1;
        let run = walk(&func, &[yes, OBJECT]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(run.results, [Value::Word(OBJECT)]);
    }

    #[test]
    fn a_local_return_before_frame_entry_skips_runtime_cleanup() {
        let mut func = TirFunction::new(
            "early_return".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        func.execution_context = crate::ir::ExecutionContextPolicy::Local;
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        let run = walk(&func, &[OBJECT]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(run.results, [Value::Word(OBJECT)]);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn an_unproven_raw_constant_is_boxed_by_the_runtime_for_the_boxed_return() {
        let mut func = TirFunction::new(
            "walk_constant".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let seven = func.fresh_value();
        func.value_types.insert(seven, TirType::I64);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::ConstInt, &[], &[seven.0], Some(7)));
        entry.terminator = Terminator::Return {
            values: vec![seven],
        };
        let run = walk(&func, &[]);
        assert_eq!(run.fact, Ok(()));
        // No plan fact proves 7 inline-safe, so the runtime boxes it.
        assert_eq!(
            run.results,
            [Value::Word(crate::codegen_ssa::scalar::tests::HEAP_INT)]
        );
        assert_eq!(
            run.runtime_calls,
            [molt_ir::runtime_fn_generated::RuntimeFn::IntFromI64]
        );
    }

    #[test]
    fn an_op_the_walker_does_not_own_refuses_the_function_by_name() {
        let mut func = TirFunction::new(
            "walk_refused".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let iterator = func.fresh_value();
        func.value_types.insert(iterator, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::ScfYield, &[0], &[iterator.0], None));
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        let linkage = row(&func);
        let plan = ProducerCarrierPlan::default();
        let runtime = crate::codegen_ssa::differential::every_runtime_entry();
        let functions = molt_ir::FunctionTable::default();
        // A refused function is never called; it must still verify.
        let run = run_function(&[OBJECT], |b, params| {
            lower_function(
                b,
                FunctionInput {
                    func: &func,
                    plan: &plan,
                    entry: params,
                    linkage: &linkage,
                    functions: &functions,
                    runtime: &runtime,
                },
            )
        });
        let Err(diagnostics) = run.fact else {
            panic!("the walk refuses ScfYield")
        };
        assert_eq!(
            diagnostics,
            ["walk_refused: the walker does not lower ScfYield yet"]
        );
    }

    const NONE: u64 = (0x7ff8 << 48) | (3 << 48);

    #[test]
    fn split_frame_publishes_location_and_returns_the_borrowed_binding() {
        let mut func = TirFunction::new(
            "chunk".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        func.execution_context = molt_ir::ir::ExecutionContextPolicy::Inherited;
        let result = func.fresh_value();
        let block = func.blocks.get_mut(&func.entry_block).unwrap();
        block
            .ops
            .push(op(OpCode::FrameSetLocation, &[], &[], Some(13)));
        let mut load = op(OpCode::Copy, &[], &[result.0], Some(0));
        load.origin = Some(molt_ir::OpKind::FrameHomeLoad);
        block.ops.push(load);
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        let mut record = [57u64, molt_codegen_abi::FRAME_HOME_PLAIN as u64, OBJECT];
        let run = walk_in(
            &func,
            &[],
            ThreadState {
                frame_top: record[1..].as_mut_ptr() as u64,
                ..Default::default()
            },
        );
        assert_eq!(run.fact, Ok(()));
        assert_eq!(run.results, [Value::Word(OBJECT)]);
        assert_eq!(
            record,
            [13, molt_codegen_abi::FRAME_HOME_PLAIN as u64, OBJECT]
        );
        assert!(run.runtime_calls.is_empty());
        let missing = walk(&func, &[]);
        assert_eq!(missing.fact, Ok(()));
        assert_eq!(missing.results, [Value::Word(NONE)]);
        assert!(missing.runtime_calls.is_empty());
    }

    /// The boxed bool `bit`.
    fn boxed_bool(bit: bool) -> u64 {
        (0x7ff8 << 48) | (2 << 48) | u64::from(bit)
    }

    /// `f()` that returns the raw bool `opcode` answers, boxed.
    fn probe(name: &str, opcode: OpCode) -> TirFunction {
        let mut func = TirFunction::new(
            name.into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let answer = func.fresh_value();
        func.value_types.insert(answer, TirType::Bool);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(op(opcode, &[], &[answer.0], None));
        entry.terminator = Terminator::Return {
            values: vec![answer],
        };
        func
    }

    #[test]
    fn exception_pending_confirms_a_set_hint_against_the_current_exception() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let func = probe("walk_exception_pending", OpCode::ExceptionPending);
        // A clear hint answers without the runtime; a stale hint confirms
        // clear; a current exception confirms set.
        for (hint, raised, pending, confirms) in [
            (0, false, false, 0),
            (0xff, false, false, 1),
            (1, true, true, 1),
        ] {
            let state = ThreadState {
                hint,
                raised,
                work: 0,
                ..ThreadState::default()
            };
            let run = walk_in(&func, &[], state);
            assert_eq!(run.fact, Ok(()));
            assert_eq!(run.results, [Value::Word(boxed_bool(pending))]);
            assert_eq!(
                run.runtime_calls,
                vec![RuntimeFn::ExceptionPending; confirms]
            );
        }
    }

    #[test]
    fn async_work_pending_reads_the_threads_pending_work_word() {
        let func = probe("walk_async_work", OpCode::AsyncWorkPending);
        for (work, pending) in [(0, false), (1, true), (0x8000_0000, true)] {
            let state = ThreadState {
                hint: 0,
                raised: false,
                work,
                ..ThreadState::default()
            };
            let run = walk_in(&func, &[], state);
            assert_eq!(run.fact, Ok(()));
            assert_eq!(run.results, [Value::Word(boxed_bool(pending))]);
            assert!(run.runtime_calls.is_empty());
        }
    }

    /// `f(x)`: an exception check whose unwind successor (label 7) takes `x`
    /// and returns None; the normal path returns `x`.
    fn checked() -> TirFunction {
        use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;
        let mut func = TirFunction::new(
            "walk_check".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let handler = func.fresh_block();
        let caught = func.fresh_value();
        let none = func.fresh_value();
        func.value_types.insert(caught, TirType::DynBox);
        func.value_types.insert(none, TirType::None);
        let mut check = op(OpCode::CheckException, &[0], &[], Some(7));
        check.set_exception_edge_kind(ExceptionEdgeKind::Unwind);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(check);
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: caught,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::ConstNone, &[], &[none.0], None)],
                terminator: Terminator::Return { values: vec![none] },
            },
        );
        func.label_id_map.insert(handler.0, 7);
        func
    }

    #[test]
    fn an_exception_check_unwinds_only_on_a_confirmed_exception() {
        let func = checked();
        for (hint, raised, answer) in [(0, false, OBJECT), (0xff, false, OBJECT), (1, true, NONE)] {
            let state = ThreadState {
                hint,
                raised,
                work: 0,
                ..ThreadState::default()
            };
            let run = walk_in(&func, &[OBJECT], state);
            assert_eq!(run.fact, Ok(()));
            assert_eq!(run.results, [Value::Word(answer)], "hint {hint:#x}");
        }
    }

    #[test]
    fn an_origin_check_captures_the_traceback_entry_only_when_it_unwinds() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;
        let mut func = checked();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops[0].set_exception_edge_kind(ExceptionEdgeKind::Origin);
        // A function with no frame homes hands the capture none.
        let run = walk(&func, &[OBJECT]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(run.results, [Value::Word(OBJECT)]);
        assert!(run.origins.is_empty());
        let raised = ThreadState {
            hint: 1,
            raised: true,
            ..ThreadState::default()
        };
        let run = walk_in(&func, &[OBJECT], raised);
        assert_eq!(run.results, [Value::Word(NONE)]);
        assert_eq!(run.origins, [0]);
        assert_eq!(
            run.runtime_calls,
            [RuntimeFn::ExceptionPending, RuntimeFn::ExceptionFrameOrigin]
        );
    }

    #[test]
    fn an_origin_check_publishes_its_location_into_the_lent_frame_first() {
        use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;
        let mut func = checked();
        func.execution_context = molt_ir::ir::ExecutionContextPolicy::Inherited;
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops[0].set_exception_edge_kind(ExceptionEdgeKind::Origin);
        entry.ops[0].set_frame_location(13);
        for (hint, raised, answer, location) in [(0, false, OBJECT, 57), (1, true, NONE, 13)] {
            let mut record = [57u64, molt_codegen_abi::FRAME_HOME_PLAIN as u64, OBJECT];
            let homes = record[1..].as_mut_ptr() as u64;
            let state = ThreadState {
                hint,
                raised,
                frame_top: homes,
                ..ThreadState::default()
            };
            let run = walk_in(&func, &[OBJECT], state);
            assert_eq!(run.fact, Ok(()));
            assert_eq!(run.results, [Value::Word(answer)], "raised {raised}");
            assert_eq!(record[0], location, "raised {raised}");
            let origins: &[u64] = if raised { &[homes] } else { &[] };
            assert_eq!(run.origins, origins);
        }
    }

    #[test]
    fn a_preserved_copy_lowers_through_its_family_table_or_refuses_by_name() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let preserved = |kind: molt_ir::OpKind| {
            let mut func = TirFunction::new(
                "walk_preserved".into(),
                vec![TirType::DynBox],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let list = func.fresh_value();
            func.value_types.insert(list, TirType::DynBox);
            let mut copy = op(OpCode::Copy, &[0], &[list.0], None);
            copy.origin = Some(kind);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(copy);
            entry.terminator = Terminator::Return { values: vec![list] };
            func
        };
        let run = walk(&preserved(molt_ir::OpKind::ListNew), &[OBJECT]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(
            run.results,
            [Value::Word(object(RuntimeFn::ListFromValues, &[OBJECT]))]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::ListFromValues]);
        // Structural phi must have become a block argument at SSA ingest.
        let run = walk(&preserved(molt_ir::OpKind::Phi), &[OBJECT]);
        let diagnostics = run
            .fact
            .expect_err("an unowned preserved kind refuses the walk");
        assert!(
            diagnostics[0].contains("unhandled preserved SimpleIR op `phi`"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn a_payload_poll_answers_the_pending_exception_or_none() {
        use crate::codegen_ssa::scalar::tests::literal_word;
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let func = TirFunction::new(
            "payload_poll".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        // (work, hint, raised): the answer and the runtime calls.
        for (work, hint, raised, answer, calls) in [
            (0, 0, false, NONE, vec![]),
            (
                0,
                1,
                true,
                literal_word(RuntimeFn::ExceptionLastPending, &[]),
                vec![RuntimeFn::ExceptionLastPending],
            ),
            (
                1,
                0,
                true,
                literal_word(RuntimeFn::AsyncWorkPollAndExceptionLastPending, &[]),
                vec![RuntimeFn::AsyncWorkPollAndExceptionLastPending],
            ),
        ] {
            let state = ThreadState {
                hint,
                raised,
                work,
                ..ThreadState::default()
            };
            let run = run_function_in(&[], state, |b, _| {
                let plan = ProducerCarrierPlan::default();
                let mut values = ValueTable::with_capacity(1);
                let diagnostics = RefCell::new(Vec::new());
                let runtime = crate::codegen_ssa::differential::every_runtime_entry();
                let mut cx = FunctionCx {
                    func: &func,
                    plan: &plan,
                    values: &mut values,
                    diagnostics: &diagnostics,
                    runtime: &runtime,
                    linkage: &Default::default(),
                    functions: &Default::default(),
                    call_sites: &Cell::new(0),
                };
                let answer = cx.async_work_poll(b, true, None);
                let answer = b.word_value(answer);
                b.return_value(Some(answer));
            });
            let case = format!("work {work} hint {hint}");
            assert_eq!(run.results, [Value::Word(answer)], "{case}");
            assert_eq!(run.runtime_calls, calls, "{case}");
        }
    }

    #[test]
    fn an_async_poll_services_pending_work_before_it_reads_the_exception_byte() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let mut func = checked();
        func.execution_context = molt_ir::ir::ExecutionContextPolicy::Inherited;
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        assert!(entry.ops[0].mark_async_work_poll());
        entry.ops[0].set_frame_location(21);
        // (work, hint, raised): the answer, the service calls, the location.
        for (work, hint, raised, answer, serviced, location) in [
            (0, 0, false, OBJECT, false, 57),
            (0, 1, true, NONE, false, 57),
            (1, 0, false, OBJECT, true, 21),
            (1, 0, true, NONE, true, 21),
        ] {
            let mut record = [57u64, molt_codegen_abi::FRAME_HOME_PLAIN as u64, OBJECT];
            let state = ThreadState {
                hint,
                raised,
                work,
                frame_top: record[1..].as_mut_ptr() as u64,
                ..ThreadState::default()
            };
            let run = walk_in(&func, &[OBJECT], state);
            let case = format!("work {work} hint {hint} raised {raised}");
            assert_eq!(run.fact, Ok(()), "{case}");
            assert_eq!(run.results, [Value::Word(answer)], "{case}");
            let calls: &[RuntimeFn] = if serviced {
                &[RuntimeFn::AsyncWorkPollAndExceptionPending]
            } else {
                &[]
            };
            assert_eq!(run.runtime_calls, calls, "{case}");
            assert_eq!(record[0], location, "{case}");
        }
    }

    /// The model's object for `function` over the words `args`.
    fn object(function: molt_ir::runtime_fn_generated::RuntimeFn, args: &[u64]) -> u64 {
        let bytes: Vec<u8> = args.iter().flat_map(|word| word.to_le_bytes()).collect();
        crate::codegen_ssa::scalar::tests::literal_word(function, &bytes)
    }

    #[test]
    fn next_of_iter_calls_each_entry_on_the_previous_answer() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        for advance in [OpCode::IterNext, OpCode::ForIter] {
            let mut func = TirFunction::new(
                "walk_iter".into(),
                vec![TirType::DynBox],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let iterator = func.fresh_value();
            let item = func.fresh_value();
            func.value_types.insert(iterator, TirType::DynBox);
            func.value_types.insert(item, TirType::DynBox);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry
                .ops
                .push(op(OpCode::GetIter, &[0], &[iterator.0], None));
            entry.ops.push(op(advance, &[iterator.0], &[item.0], None));
            entry.terminator = Terminator::Return { values: vec![item] };
            let run = walk(&func, &[OBJECT]);
            assert_eq!(run.fact, Ok(()));
            let expected = object(
                RuntimeFn::IterNext,
                &[object(RuntimeFn::IterChecked, &[OBJECT])],
            );
            assert_eq!(run.results, [Value::Word(expected)], "{advance:?}");
            assert_eq!(
                run.runtime_calls,
                [RuntimeFn::IterChecked, RuntimeFn::IterNext]
            );
        }
    }

    #[test]
    fn an_unpacking_advance_binds_its_words_then_its_flag() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let mut func = TirFunction::new(
            "walk_unpack".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let results: Vec<ValueId> = (0..3).map(|_| func.fresh_value()).collect();
        for &result in &results {
            func.value_types.insert(result, TirType::DynBox);
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(op(
            OpCode::IterNextUnpack2,
            &[0],
            &results.iter().map(|id| id.0).collect::<Vec<_>>(),
            None,
        ));
        // Return the second word: the runtime wrote it through the entry array.
        entry.terminator = Terminator::Return {
            values: vec![results[1]],
        };
        let run = walk(&func, &[OBJECT]);
        assert_eq!(run.fact, Ok(()));
        let mut key = OBJECT.to_le_bytes().to_vec();
        key.push(1);
        assert_eq!(
            run.results,
            [Value::Word(
                crate::codegen_ssa::scalar::tests::literal_word(RuntimeFn::IterNextUnpack2, &key)
            )]
        );
    }

    #[test]
    fn an_unpacked_sequence_binds_each_target_the_runtime_wrote() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let mut func = TirFunction::new(
            "walk_unpack_sequence".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let targets: Vec<ValueId> = (0..3).map(|_| func.fresh_value()).collect();
        for &target in &targets {
            func.value_types.insert(target, TirType::DynBox);
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(op(
            OpCode::UnpackSequence,
            &[0],
            &targets.iter().map(|id| id.0).collect::<Vec<_>>(),
            Some(3),
        ));
        entry.terminator = Terminator::Return {
            values: vec![targets[2]],
        };
        let run = walk(&func, &[OBJECT]);
        assert_eq!(run.fact, Ok(()));
        let mut key = OBJECT.to_le_bytes().to_vec();
        key.push(2);
        assert_eq!(
            run.results,
            [Value::Word(
                crate::codegen_ssa::scalar::tests::literal_word(RuntimeFn::UnpackSequence, &key)
            )]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::UnpackSequence]);
    }

    #[test]
    fn a_bound_object_is_allocated_from_its_class() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        // A class object with no lifecycle plan, so the shared allocation
        // reads its header and plan word and takes its miss, the runtime
        // constructor. The image lives for the test.
        let image: &'static mut [u64] = Box::leak(vec![0_u64; 3 + 20].into_boxed_slice());
        image[0] = u64::from(molt_codegen_abi::TYPE_ID_TYPE);
        let class = molt_codegen_abi::QNAN
            | molt_codegen_abi::TAG_PTR
            | (std::ptr::from_ref(&image[3]) as u64);
        let mut func = TirFunction::new(
            "walk_object_new".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let object_id = func.fresh_value();
        func.value_types.insert(object_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut new = op(OpCode::ObjectNewBound, &[0], &[object_id.0], None);
        new.attrs
            .insert("class_definition".into(), AttrValue::Str("Point".into()));
        entry.ops.push(new);
        entry.terminator = Terminator::Return {
            values: vec![object_id],
        };
        let run = walk(&func, &[class]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(
            run.results,
            [Value::Word(object(RuntimeFn::ObjectNewBound, &[class]))]
        );
    }

    /// `f()` that applies the recursion charges `charges`, then answers
    /// whether the limit admits one more frame.
    fn charged(charges: &[OpCode]) -> TirFunction {
        let mut func = TirFunction::new(
            "walk_recursion".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let admitted = func.fresh_value();
        func.value_types.insert(admitted, TirType::Bool);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for &charge in charges {
            entry.ops.push(op(charge, &[], &[], None));
        }
        entry
            .ops
            .push(op(OpCode::RecursionAdmits, &[], &[admitted.0], None));
        entry.terminator = Terminator::Return {
            values: vec![admitted],
        };
        func
    }

    #[test]
    fn the_recursion_charge_counts_against_the_runtimes_limit() {
        let state = |depth| ThreadState {
            depth,
            limit: 100,
            ..ThreadState::default()
        };
        // Below the limit admits; at it refuses.
        for (depth, admits) in [(0, true), (99, true), (100, false)] {
            let run = walk_in(&charged(&[]), &[], state(depth));
            assert_eq!(run.fact, Ok(()));
            assert_eq!(
                run.results,
                [Value::Word(boxed_bool(admits))],
                "depth {depth}"
            );
        }
        // A counted frame reaches the limit; a released one leaves it.
        let run = walk_in(&charged(&[OpCode::RecursionCount]), &[], state(99));
        assert_eq!(run.results, [Value::Word(boxed_bool(false))]);
        let run = walk_in(&charged(&[OpCode::RecursionRelease]), &[], state(100));
        assert_eq!(run.results, [Value::Word(boxed_bool(true))]);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_module_attribute_read_borrows_both_operands() {
        use molt_ir::runtime_fn_generated::RuntimeFn;
        const NAME: u64 = (0x7ff8 << 48) | (4 << 48) | 0x3000;
        let mut func = TirFunction::new(
            "walk_module_attr".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let attr = func.fresh_value();
        func.value_types.insert(attr, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::ModuleGetAttr, &[0, 1], &[attr.0], None));
        entry.terminator = Terminator::Return { values: vec![attr] };
        let run = walk(&func, &[OBJECT, NAME]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(
            run.results,
            [Value::Word(object(
                RuntimeFn::ModuleGetAttr,
                &[OBJECT, NAME]
            ))]
        );
        assert!(run.refcounts.is_empty(), "both operands are borrowed");
    }

    #[test]
    fn a_closure_slot_store_is_read_back_from_the_owners_payload() {
        // A payload of four words; the owner is its boxed address.
        let mut payload = vec![0u64; 4];
        payload[1] = (0x7ff8 << 48) | (1 << 48) | 5;
        let owner = (0x7ff8 << 48) | (4 << 48) | (payload.as_mut_ptr() as u64);
        let stored = (0x7ff8 << 48) | (1 << 48) | 9;
        // f(owner, value): read slot 1, store `value` in slot 2, read it back.
        let mut func = TirFunction::new(
            "walk_closure".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let first = func.fresh_value();
        let stored_none = func.fresh_value();
        let second = func.fresh_value();
        for id in [first, stored_none, second] {
            func.value_types.insert(id, TirType::DynBox);
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::ClosureLoad, &[0], &[first.0], Some(8)));
        entry.ops.push(op(
            OpCode::ClosureStore,
            &[0, 1],
            &[stored_none.0],
            Some(16),
        ));
        entry
            .ops
            .push(op(OpCode::ClosureLoad, &[0], &[second.0], Some(16)));
        entry.terminator = Terminator::Return {
            values: vec![second],
        };
        let run = walk(&func, &[owner, stored]);
        assert_eq!(run.fact, Ok(()));
        assert_eq!(run.results, [Value::Word(stored)]);
        assert_eq!(payload[2], stored, "the store reached the payload");
        assert_eq!(payload[1], (0x7ff8 << 48) | (1 << 48) | 5);
        assert!(run.runtime_calls.is_empty(), "inline words never miss");
    }
}
