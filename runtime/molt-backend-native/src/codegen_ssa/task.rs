//! A task owns one reference per payload slot. Repeated operands share a
//! single materialization; allocation failure must never mint an integer
//! owner that the task cannot adopt.

use std::collections::BTreeMap;

use molt_codegen_abi::task_site::{PayloadCustody, TaskConstructorLayout, TaskFrameContext};
use molt_ir::tir::values::ValueId;

use super::builder::CodegenBuilder;
use super::function::FunctionCx;
use super::scalar;
use super::value_table::HeldCarrier;

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
    fn place_task(
        &mut self,
        layout: TaskConstructorLayout,
        poll: Self::Word,
        frame_bytes: Self::Word,
        operands: TaskOperands<Self::Word>,
        name: &str,
    ) -> Self::Word;
}
