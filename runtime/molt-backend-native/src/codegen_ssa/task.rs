//! A task owns one reference per payload slot. Repeated operands share a
//! single materialization; allocation failure must never mint an integer
//! owner that the task cannot adopt.

use std::collections::BTreeMap;

use molt_codegen_abi::task_site::{
    PayloadCustody, TaskConstructorLayout, TaskFrameContext, TaskPayload,
};
use molt_ir::tir::values::ValueId;

use super::builder::{CodegenBuilder, MachineKind};
use super::control::carrier_kind;
use super::custody::{CallTarget, RuntimeArg};
use super::function::FunctionCx;
use super::function::ResultCustody;
use super::scalar;
use super::value_table::HeldCarrier;
use molt_ir::{
    code_slots::TaskPoll,
    runtime_fn_generated::RuntimeFn,
    tir::ops::{AttrValue, TirOp},
};

pub(crate) struct TaskOperands<W> {
    pub(crate) context: TaskFrameContext<W>,
    pub(crate) words: Vec<(W, PayloadCustody)>,
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    pub(crate) fn task_operands(
        &self,
        b: &mut B,
        layout: TaskConstructorLayout,
        closure_size: i64,
        function: Option<ValueId>,
        operands: &[ValueId],
    ) -> TaskOperands<B::Word> {
        layout.validate_closure_size(
            closure_size,
            operands.len(),
            false,
            crate::GENERATOR_CONTROL_BYTES,
        );
        let values = self.operands();
        let mut first = BTreeMap::<ValueId, usize>::new();
        let mut words: Vec<(B::Word, PayloadCustody)> = Vec::with_capacity(operands.len());
        for (index, &id) in operands.iter().enumerate() {
            let item = match first.entry(id) {
                std::collections::btree_map::Entry::Occupied(entry) => {
                    let previous = *entry.get();
                    (words[previous].0, PayloadCustody::SameAs(previous))
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(index);
                    match values.held(id) {
                        HeldCarrier::RawI64 if !self.plan.is_inline_safe_int(id) => {
                            let value = values.resolve(b, id);
                            (scalar::word_of(b, value), PayloadCustody::RawInt)
                        }
                        carrier => {
                            let custody = if carrier == HeldCarrier::Boxed
                                && !self.plan.boxed_refcount_is_noop(id)
                            {
                                PayloadCustody::Borrowed
                            } else {
                                PayloadCustody::Unowned
                            };
                            (values.boxed_word(b, id), custody)
                        }
                    }
                }
            };
            words.push(item);
        }
        let context = match function {
            Some(id) => TaskFrameContext::Function(values.boxed_word(b, id)),
            None => TaskFrameContext::NoFrame,
        };
        TaskOperands { context, words }
    }
}

/// The task allocation generator lives in an inlined helper on LLVM and in
/// the current function on Cranelift. Its inputs have identical custody.
pub(crate) trait TaskPlacement: CodegenBuilder {
    fn poll_frame_bytes(&mut self, symbol: &str) -> Self::Word;
    fn place_task(
        &mut self,
        layout: TaskConstructorLayout,
        poll: Self::Word,
        frame_bytes: Self::Word,
        context: TaskFrameContext<Self::Word>,
        payload: TaskPayload<'_, Self::Word>,
        name: &str,
    ) -> Self::Word;
}

/// The body of a task trampoline: the new task word it answers. The
/// trampoline borrows the closure and argument array. The allocator takes
/// its context from the current thread's pending Python invocation.
pub(crate) fn task_trampoline_word<B: TaskPlacement>(
    b: &mut B,
    layout: TaskConstructorLayout,
    poll: B::Word,
    frame_bytes: B::Word,
    closure: Option<B::Word>,
    arguments: B::Word,
    count: usize,
) -> B::Word {
    let thread = b.thread_record_word();
    b.place_task(
        layout,
        poll,
        frame_bytes,
        TaskFrameContext::PendingInvocation { thread },
        TaskPayload::BorrowedArguments {
            closure,
            arguments,
            count,
        },
        "task_new",
    )
}

impl<B: TaskPlacement> FunctionCx<'_, B> {
    pub(crate) fn alloc_task(&mut self, b: &mut B, op: &TirOp) {
        let size = match op.attrs.get("value") {
            Some(AttrValue::Int(size)) => *size,
            _ => 0,
        };
        let kind = if op.attrs.contains_key("task_kind") {
            op.task_kind()
                .unwrap_or_else(|| panic!("invalid task constructor kind in {}", self.func.name))
        } else {
            molt_ir::task_kind::TaskKind::Future
        };
        let (function, words) = molt_ir::tir::ops::alloc_task_operands(&op.operands);
        self.construct_task(
            b,
            op,
            crate::task_layout(kind),
            size,
            Some(*function),
            words,
        );
    }

    pub(crate) fn call_async(&mut self, b: &mut B, op: &TirOp) -> bool {
        if op.results.is_empty() {
            return false;
        }
        if op.runtime == Some(RuntimeFn::AsyncSleep) {
            if op.operands.len() > 2 {
                return false;
            }
            let defaults = [
                0.0_f64.to_bits(),
                molt_codegen_abi::QNAN_TAG_NONE_I64 as u64,
            ];
            let args = std::array::from_fn::<_, 2, _>(|index| match op.operands.get(index) {
                Some(&id) => RuntimeArg::Operand(id),
                None => {
                    let word = b.constant(defaults[index]);
                    RuntimeArg::Word(b.word_value(word))
                }
            });
            let callee = b.runtime_callee(RuntimeFn::AsyncSleep);
            self.runtime_call(
                b,
                op,
                CallTarget::Callee(callee),
                &args,
                ResultCustody::canonical(RuntimeFn::AsyncSleep, args.len()),
                "async_sleep",
                "call_async_sleep",
            );
        } else {
            let layout = TaskConstructorLayout::for_call_async();
            let size = layout.required_closure_size(
                op.operands.len(),
                false,
                crate::GENERATOR_CONTROL_BYTES,
            );
            self.construct_task(b, op, layout, size, None, &op.operands);
        }
        true
    }

    fn construct_task(
        &mut self,
        b: &mut B,
        op: &TirOp,
        layout: TaskConstructorLayout,
        size: i64,
        function: Option<ValueId>,
        words: &[ValueId],
    ) {
        let compiled = if let Some(id) = op.callee {
            let symbol = self.functions.symbol(id);
            let Some(abi) = self.linkage.get(symbol) else {
                self.fatal(format!("poll {symbol} lacks its native linkage ABI"));
                return;
            };
            let is_word = |ty: &molt_ir::tir::types::TirType| {
                carrier_kind(HeldCarrier::of(ty)) == MachineKind::Word
            };
            if abi.source_signature.has_closure
                || !matches!(abi.param_types.as_slice(), [frame] if is_word(frame))
                || !abi.return_type.as_ref().is_some_and(is_word)
            {
                self.fatal(format!(
                    "task poll target `{symbol}` must have ABI (context, task) -> i64"
                ));
                return;
            }
            Some((symbol, abi))
        } else {
            None
        };
        let Some(target) = TaskPoll::from_targets(compiled, op.runtime) else {
            self.fatal(format!(
                "{:?} requires a compiled poll or a manifest poll key",
                op.opcode
            ));
            return;
        };
        let (address, frame_bytes) = target.operands(
            b,
            size,
            |b, bits| b.constant(bits),
            |b, (symbol, abi)| {
                let callee = b.function_callee(symbol, abi);
                (b.function_address(callee), b.poll_frame_bytes(symbol))
            },
        );
        let payload = self.task_operands(b, layout, size, function, words);
        let result = b.place_task(
            layout,
            address,
            frame_bytes,
            payload.context,
            TaskPayload::Words(&payload.words),
            "task_new",
        );
        let value = b.word_value(result);
        self.bind_owned(b, op, value);
    }
}
