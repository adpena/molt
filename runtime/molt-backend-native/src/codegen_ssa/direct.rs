//! The walker's direct ops (design: `docs/design/foundation/codegen_walker.md`,
//! W2): the SSA value copy and the local-slot deletion, raw allocation and
//! the fused `ord(container[index])`, written once for every native backend.

use molt_codegen_abi as nanbox;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::CodegenBuilder;
use super::custody::{CallTarget, RuntimeArg};
use super::function::{FunctionCx, ResultCustody};
use super::scalar;
use super::value_table::HeldCarrier;
use crate::tir::ops::TirOp;

/// The boxed word of `None`.
const NONE_WORD: u64 = nanbox::QNAN | nanbox::TAG_NONE;

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Bind every result of `op` to operand 0's value in its carrier, or to
    /// None when the op has no operand.
    fn pass_through(&mut self, b: &mut B, op: &TirOp) {
        if op.results.is_empty() {
            return;
        }
        let (value, carrier) = match op.operands.first() {
            Some(&operand) => {
                let operands = self.operands();
                (operands.resolve(b, operand), operands.held(operand))
            }
            None => {
                let none = b.constant(NONE_WORD);
                (b.word_value(none), HeldCarrier::Boxed)
            }
        };
        for &result in &op.results {
            self.bind(result, value, carrier);
        }
    }

    /// `DeleteVar`: the local's new SSA value is the missing sentinel
    /// operand. The previous occupant's ownership is the drop plane's
    /// (DecRef, consumed operands), not this op's.
    pub(crate) fn delete_var_op(&mut self, b: &mut B, op: &TirOp) {
        self.pass_through(b, op);
    }

    /// `Copy` with no origin: a genuine SSA value copy (`copy`, `copy_var`,
    /// `load_var`, `store_var`), so operand 0 passes through. A `Copy` whose
    /// origin no preserved-op lowering claimed is refused: passing operand 0
    /// through would drop its side effect or alias a fresh value, which the
    /// drop pass then frees twice (`alias_analysis::CopyLowering`).
    pub(crate) fn copy_op(&mut self, b: &mut B, op: &TirOp) {
        let Some(kind) = op.origin else {
            self.pass_through(b, op);
            return;
        };
        let (operands, results) = (op.operands.len(), op.results.len());
        let obligation =
            if crate::tir::passes::alias_analysis::copy_kind_reaches_no_incref_passthrough(Some(
                kind,
            )) {
                "its shared lowering must preserve the operation's effects"
            } else {
                "its shared lowering must produce a fresh owned result"
            };
        self.fatal(format!(
            "unhandled preserved SimpleIR op `{kind}` (operands={operands}, results={results}): {obligation}"
        ));
    }

    /// `Alloc`: a raw object of the operand's size, published initialized.
    pub(crate) fn alloc_op(&mut self, b: &mut B, op: &TirOp) {
        let size = self.operands().resolve(b, op.operands[0]);
        let size = scalar::word_of(b, size);
        let object = call_word(b, RuntimeFn::Alloc, &[size], "alloc");
        let object = call_word(
            b,
            RuntimeFn::ObjectPublishInitialized,
            &[object],
            "publish_initialized",
        );
        let object = b.word_value(object);
        self.bind(op.results[0], object, HeldCarrier::Boxed);
    }

    /// `OrdAt`: the fused `ord(container[index])` over borrowed operands.
    pub(crate) fn ord_at_op(&mut self, b: &mut B, op: &TirOp) {
        if op.operands.len() < 2 {
            return;
        }
        let callee = b.runtime_callee(RuntimeFn::OrdAt);
        let args = [
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
        ];
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            &args,
            ResultCustody::canonical(RuntimeFn::OrdAt, 2),
            "ord_at",
            "ord_at",
        );
    }
}

/// Call `function` with machine words, for its word.
fn call_word<B: CodegenBuilder>(
    b: &mut B,
    function: RuntimeFn,
    args: &[B::Word],
    name: &str,
) -> B::Word {
    let value = b
        .call_runtime(function, args)
        .unwrap_or_else(|| panic!("{} returns a word", function.symbol()));
    let named = b.word_value(value);
    b.set_name(named, name);
    value
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// One borrowed runtime call over `op`'s first operand, bound by
    /// `custody`.
    fn unary_runtime_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        function: RuntimeFn,
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let callee = b.runtime_callee(function);
        let args = [RuntimeArg::Operand(op.operands[0])];
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            &args,
            custody,
            label,
            call_name,
        );
    }

    /// `Raise`: `molt_raise` borrows the exception object and returns the
    /// immortal None with that exception pending.
    pub(crate) fn raise_op(&mut self, b: &mut B, op: &TirOp) {
        self.unary_runtime_call(
            b,
            op,
            RuntimeFn::Raise,
            ResultCustody::Unowned,
            "raise",
            "raise",
        );
    }

    /// `WarnStderr`: a side-effecting diagnostic.
    pub(crate) fn warn_stderr_op(&mut self, b: &mut B, op: &TirOp) {
        self.unary_runtime_call(
            b,
            op,
            RuntimeFn::WarnStderr,
            ResultCustody::Boxed(molt_ir::runtime_fn_generated::RuntimeBoxedReturn::Void),
            "warn_stderr",
            "warn_stderr",
        );
    }

    /// `FunctionDefaultsVersion`: a function object's `__defaults__` /
    /// `__kwdefaults__` mutation stamp, a boxed inline int. It reads mutable
    /// runtime state, so it never folds.
    pub(crate) fn function_defaults_version_op(&mut self, b: &mut B, op: &TirOp) {
        assert!(
            !op.operands.is_empty(),
            "FunctionDefaultsVersion requires a function operand"
        );
        self.unary_runtime_call(
            b,
            op,
            RuntimeFn::FunctionDefaultsVersion,
            ResultCustody::canonical(RuntimeFn::FunctionDefaultsVersion, 1),
            "function_defaults_version",
            "func_defaults_version",
        );
    }
}

#[cfg(test)]
mod tests {
    //! The direct ops on the interpreting builder and as Cranelift machine
    //! code. The oracle is each op's contract: a copy binds operand 0 in its
    //! own carrier (None without an operand), an unclaimed preserved copy is
    //! refused, allocation publishes the object it allocated, `ord_at` is
    //! one borrowed call.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel};
    use super::super::scalar::tests::{Value, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use molt_ir::OpKind;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    #[derive(Clone, Copy, Debug)]
    enum Family {
        Copy,
        DeleteVar,
        Alloc,
        OrdAt,
    }

    fn op(opcode: OpCode, origin: Option<OpKind>, operands: &[u32], results: &[u32]) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: operands.iter().map(|&id| ValueId(id)).collect(),
            results: results.iter().map(|&id| ValueId(id)).collect(),
            attrs: AttrDict::default(),
            source_span: None,
            origin,
            runtime: None,
            callee: None,
        }
    }

    /// Lower `op` over %0 (a boxed object) and %1 (a raw int); the fact is
    /// the diagnostics and the first result's carrier.
    fn lower(family: Family, op: TirOp) -> Run<(Vec<String>, Option<HeldCarrier>)> {
        let lowering = kernel!(family: Family = family, op: TirOp = op; |b, inputs| -> (Vec<String>, Option<HeldCarrier>) {
            let func = TirFunction::new(
                "direct".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(8);
            values.set_value(ValueId(0), inputs[0]);
            values.set_repr(ValueId(0), HeldCarrier::Boxed);
            values.set_value(ValueId(1), inputs[1]);
            values.set_repr(ValueId(1), HeldCarrier::RawI64);
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
            match *family {
                Family::Copy => cx.copy_op(b, op),
                Family::DeleteVar => cx.delete_var_op(b, op),
                Family::Alloc => cx.alloc_op(b, op),
                Family::OrdAt => cx.ord_at_op(b, op),
            }
            let result = op.results.first().copied();
            let carrier = result.and_then(|id| cx.values.repr(id));
            let results = result.and_then(|id| cx.values.value(id)).into_iter().collect();
            ((diagnostics.borrow().clone(), carrier), results)
        });
        both(&lowering, &[Value::Word(OBJECT), Value::Word(7)])
    }

    #[test]
    fn a_copy_binds_operand_zero_in_its_carrier_or_none() {
        let run = lower(Family::Copy, op(OpCode::Copy, None, &[1], &[2]));
        assert_eq!(run.fact, (vec![], Some(HeldCarrier::RawI64)));
        assert_eq!(run.results, [Value::Word(7)]);
        let run = lower(Family::DeleteVar, op(OpCode::DeleteVar, None, &[], &[2]));
        assert_eq!(run.fact, (vec![], Some(HeldCarrier::Boxed)));
        assert_eq!(run.results, [Value::Word(NONE_WORD)]);
        let run = lower(Family::Copy, op(OpCode::Copy, None, &[0], &[]));
        assert_eq!(run.fact, (vec![], None), "no result binds nothing");
    }

    #[test]
    fn a_preserved_copy_no_lowering_claimed_is_refused() {
        let run = lower(
            Family::Copy,
            op(OpCode::Copy, Some(OpKind::Abs), &[0], &[2]),
        );
        let (diagnostics, carrier) = &run.fact;
        assert!(
            diagnostics.len() == 1 && diagnostics[0].contains("`abs`"),
            "{diagnostics:?}"
        );
        assert_eq!(*carrier, None, "a refused copy binds nothing");
    }

    #[test]
    fn allocation_publishes_the_object_it_allocated_and_ord_at_borrows() {
        let run = lower(Family::Alloc, op(OpCode::Alloc, None, &[1], &[2]));
        let object = literal_word(RuntimeFn::Alloc, &7u64.to_le_bytes());
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::ObjectPublishInitialized,
                &object.to_le_bytes()
            ))]
        );
        assert_eq!(
            run.runtime_calls,
            [RuntimeFn::Alloc, RuntimeFn::ObjectPublishInitialized]
        );
        let run = lower(Family::OrdAt, op(OpCode::OrdAt, None, &[0, 0], &[2]));
        let mut key = OBJECT.to_le_bytes().to_vec();
        key.extend(OBJECT.to_le_bytes());
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::OrdAt, &key))]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::OrdAt]);
    }
}
