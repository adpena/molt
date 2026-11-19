//! The walker's callable family (design: `docs/design/foundation/codegen_walker.md`,
//! W2): the callable ops that are runtime calls, written once for every native
//! backend. Function construction (`func_new`, `builtin_func`) waits for
//! helper bodies through the builder (typed trampolines).

use molt_ir::OpKind;
use molt_ir::code_slots::ProgramSlotKind;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::CodegenBuilder;
use super::custody::{CallTarget, RuntimeArg};
use super::function::{FunctionCx, ResultCustody};
use crate::tir::ops::{AttrValue, TirOp};

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// A preserved callable op that is a runtime call: `function_closure_bits`,
    /// `asyncgen_new`, `code_slot_set`, `code_slots_init`, `callargs_new` and
    /// the `line` marker. Answers false for any other kind, or for a
    /// malformed op the caller then refuses.
    pub(crate) fn callable_runtime_op(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
        let unary = |op: &TirOp| op.operands.len() == 1 && op.results.len() <= 1;
        match kind {
            // The function owns this edge: only a kept result gains an owner,
            // and a discarded borrowed edge is never released.
            OpKind::FunctionClosureBits if unary(op) => self.borrowed_call(
                b,
                op,
                RuntimeFn::FunctionClosureBits,
                &[RuntimeArg::Operand(op.operands[0])],
                ResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue),
                "function_closure_bits",
            ),
            // The dedicated task-wrapper ABI.
            OpKind::AsyncgenNew if unary(op) => self.borrowed_call(
                b,
                op,
                RuntimeFn::AsyncgenNew,
                &[RuntimeArg::Operand(op.operands[0])],
                ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                "asyncgen_new",
            ),
            OpKind::FunctionClosureBits | OpKind::AsyncgenNew => return false,
            OpKind::CodeSlotSet => {
                // The slot id is a raw ABI word; the code object and the
                // globals are borrowed. Only the exception state reports
                // failure.
                let owner = match op.attrs.get("s_value") {
                    Some(AttrValue::Str(owner)) => owner.as_str(),
                    _ => panic!("admitted program-slot owner"),
                };
                let code = b.program_slot(ProgramSlotKind::Code, owner);
                let code = b.word_value(code);
                self.borrowed_call(
                    b,
                    op,
                    RuntimeFn::CodeSlotSet,
                    &[
                        RuntimeArg::Word(code),
                        RuntimeArg::Operand(op.operands[0]),
                        RuntimeArg::Operand(op.operands[1]),
                    ],
                    ResultCustody::SideEffect,
                    "code_slot_set",
                );
            }
            OpKind::CodeSlotsInit => {
                let Some(&AttrValue::Int(count)) = op.attrs.get("value") else {
                    panic!("admitted code_slots_init count");
                };
                let count = b.constant(count as u64);
                b.call_runtime(RuntimeFn::CodeSlotsInit, &[count]);
            }
            OpKind::CallargsNew => {
                // The source call form picks the builder: a CALL_FUNCTION_EX
                // site's arguments are its own tuple and mapping.
                let constructor = op
                    .call_argument_form()
                    .expect("validated callargs_new call form")
                    .runtime_constructor();
                let zero = b.constant(0);
                let builder = b
                    .call_runtime(constructor, &[zero, zero])
                    .expect("a CallArgs constructor answers a word");
                let builder = b.word_value(builder);
                b.set_name(builder, "callargs_new");
                self.bind_owned(b, op, builder);
            }
            OpKind::Line => {}
            _ => return false,
        }
        true
    }

    /// One borrowed runtime call of `function` over `args`, bound by
    /// `custody`, labelled by `name`.
    fn borrowed_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        function: RuntimeFn,
        args: &[RuntimeArg<B::Value>],
        custody: ResultCustody,
        name: &str,
    ) {
        let callee = b.runtime_callee(function);
        let label = op.origin.map_or(name, |kind| kind.as_str());
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            args,
            custody,
            label,
            name,
        );
    }
}

#[cfg(test)]
mod tests {
    //! The callable family's runtime calls on the interpreting builder and as
    //! Cranelift machine code. The oracle is each call's contract: a closure
    //! read is borrowed and binds with its own reference; the line marker
    //! emits nothing.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel};
    use super::super::scalar::tests::Value;
    use super::super::value_table::{HeldCarrier, ValueTable};
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use molt_codegen_abi::inline_refcount::RefcountOperation;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    fn lower(kind: OpKind) -> Run<(bool, Option<HeldCarrier>)> {
        let lowering = kernel!(kind: OpKind = kind; |b, inputs| -> (bool, Option<HeldCarrier>) {
            let func = TirFunction::new(
                "callable".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(4);
            values.set_value(ValueId(0), inputs[0]);
            values.set_repr(ValueId(0), HeldCarrier::Boxed);
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &every_runtime_entry(),
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &Cell::new(0),
            };
            let op = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![ValueId(0)],
                results: vec![ValueId(1)],
                attrs: AttrDict::default(),
                source_span: None,
                origin: Some(*kind),
                runtime: None,
                callee: None,
            };
            let lowered = cx.callable_runtime_op(b, &op, *kind);
            let results = cx.values.value(ValueId(1)).into_iter().collect();
            ((lowered, cx.values.repr(ValueId(1))), results)
        });
        both(&lowering, &[Value::Word(OBJECT)])
    }

    #[test]
    fn a_closure_read_is_borrowed_and_its_kept_result_retained() {
        let run = lower(OpKind::FunctionClosureBits);
        assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)));
        assert_eq!(run.runtime_calls, [RuntimeFn::FunctionClosureBits]);
        let Value::Word(closure) = run.results[0] else {
            panic!("a closure is a word")
        };
        assert_eq!(run.refcounts, [(RefcountOperation::Retain, closure)]);
    }

    #[test]
    fn the_line_marker_emits_nothing_and_other_kinds_are_left() {
        let run = lower(OpKind::Line);
        assert_eq!(run.fact, (true, None));
        assert!(run.runtime_calls.is_empty());
        let run = lower(OpKind::FuncNew);
        assert_eq!(run.fact, (false, None));
    }
}
