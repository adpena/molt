//! The walker's view of one function being lowered (design:
//! `docs/design/foundation/codegen_walker.md`, W2, "the walker context").
//!
//! `FunctionCx` owns the mutable value table for the duration of an op's
//! lowering, so an op family binds its results here, once, instead of in each
//! backend. A backend lends it beside its builder: the two borrow disjoint
//! parts of the backend's function state.

use std::cell::{Cell, RefCell};

use molt_ir::runtime_callable_symbols::RuntimeFnSet;

use molt_codegen_abi as nanbox;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::CodegenBuilder;
use super::custody::{CallTarget, Operands, RuntimeArg};
use super::scalar;
use super::value_table::{HeldCarrier, ValueTable};
use crate::representation_plan::ProducerCarrierPlan;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// The boxed word of `None`.
const NONE_WORD: u64 = nanbox::QNAN | nanbox::TAG_NONE;

/// Custody of a runtime call's return word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultCustody {
    /// The canonical boxed return (a generated boxed-ABI row or the manifest
    /// return contract): an owned or poll word is bound or released, a
    /// borrowed word is retained only when bound, and a void callee binds None.
    Boxed(RuntimeBoxedReturn),
    /// An immediate or immortal word (None, a boolean) with no owner: bound
    /// as-is when requested and never released.
    Unowned,
    /// The call reports only through the exception state: its word is not
    /// adopted and a requested result is None.
    SideEffect,
}

impl ResultCustody {
    /// The generated boxed-ABI return custody of a dedicated call whose entry
    /// has a canonical row; a missing row is generator drift, not a fallback.
    pub(crate) fn canonical(function: RuntimeFn, arity: usize) -> Self {
        let abi = function
            .boxed_abi()
            .filter(|abi| abi.arity() == arity)
            .unwrap_or_else(|| {
                panic!(
                    "{}/{arity} must have a generated boxed ABI row",
                    function.symbol()
                )
            });
        Self::Boxed(abi.result)
    }
}

/// One function being lowered: its TIR, carrier plan, lowered values and
/// diagnostics, the runtime entries the selected runtime provides, and the
/// count of inline-cache call sites numbered so far.
pub(crate) struct FunctionCx<'a, B: CodegenBuilder> {
    pub(crate) func: &'a TirFunction,
    pub(crate) plan: &'a ProducerCarrierPlan,
    pub(crate) values: &'a mut ValueTable<B::Value>,
    pub(crate) diagnostics: &'a RefCell<Vec<String>>,
    pub(crate) runtime: &'a RuntimeFnSet,
    pub(crate) call_sites: &'a Cell<usize>,
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Custody's read-only view of this function.
    pub(crate) fn operands(&self) -> Operands<'_, B> {
        Operands {
            function: &self.func.name,
            values: self.values,
            plan: self.plan,
            diagnostics: self.diagnostics,
        }
    }

    /// Record a fatal lowering diagnostic for this function.
    pub(crate) fn fatal(&self, message: impl Into<String>) {
        self.diagnostics
            .borrow_mut()
            .push(format!("{}: {}", self.func.name, message.into()));
    }

    /// The boxed inline-int id of the next inline-cache call site of `lane`
    /// in this function (`molt_codegen_abi::stable_ic_site_id`). Site ids fit
    /// the inline window by construction, so the word owns nothing.
    pub(crate) fn next_call_site(&self, b: &mut B, lane: &str) -> B::Word {
        let index = self.call_sites.get();
        self.call_sites.set(index + 1);
        let site = molt_codegen_abi::stable_ic_site_id(self.func.name.as_str(), index, lane);
        assert!(
            (nanbox::INT_MIN_INLINE..=nanbox::INT_MAX_INLINE).contains(&site),
            "compile-time integer word {site} does not fit the inline payload"
        );
        b.constant(nanbox::box_int_bits(site) as u64)
    }

    /// Record `id` as lowered to `value`, held as `carrier`.
    pub(crate) fn bind(&mut self, id: ValueId, value: B::Value, carrier: HeldCarrier) {
        self.values.set_value(id, value);
        self.values.set_repr(id, carrier);
    }

    /// Bind an owned boxed word as `op`'s result, or release it when the op
    /// has no result.
    pub(crate) fn bind_owned(&mut self, b: &mut B, op: &TirOp, value: B::Value) {
        match op.results.first() {
            Some(&result) => self.bind(result, value, HeldCarrier::Boxed),
            None => {
                let release = b.refcount_site(RefcountOperation::Release);
                let word = b.expect_word(value);
                b.refcount(release, word);
            }
        }
    }

    /// One borrowed call whose word binds or retires by `custody`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn runtime_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        target: CallTarget<B::Callee>,
        args: &[RuntimeArg<B::Value>],
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let bound = !op.results.is_empty();
        let retain = bound && custody == ResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue);
        let word = self
            .operands()
            .call(b, target, args, retain, label, call_name);
        match custody {
            ResultCustody::Boxed(
                RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue,
            ) => {
                let value = b.word_value(word);
                self.bind_owned(b, op, value)
            }
            ResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue) | ResultCustody::Unowned => {
                if bound {
                    let value = b.word_value(word);
                    self.bind_owned(b, op, value);
                }
            }
            ResultCustody::Boxed(RuntimeBoxedReturn::Void) | ResultCustody::SideEffect => {
                let none = b.constant(NONE_WORD);
                let none = b.word_value(none);
                for &result in &op.results {
                    self.bind(result, none, HeldCarrier::Boxed);
                }
            }
        }
    }

    /// `runtime_call` over every operand of `op`, in order.
    pub(crate) fn positional_runtime_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        callee: B::Callee,
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let args: Vec<RuntimeArg<B::Value>> = op
            .operands
            .iter()
            .map(|&operand| RuntimeArg::Operand(operand))
            .collect();
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

    /// `IncRef`/`DecRef`. Reference counting consumes the held carrier,
    /// never an annotation or a freshly boxed view: a raw full-width integer's
    /// payload bits must not reach the object reference-count ABI.
    pub(crate) fn refcount_op(&mut self, b: &mut B, op: &TirOp, operation: RefcountOperation) {
        let operand = op.operands[0];
        let operands = self.operands();
        let value = operands.resolve(b, operand);
        let held = operands.held(operand);
        if held == HeldCarrier::Boxed && !self.plan.carrier_refcount_is_noop(operand) {
            let site = b.refcount_site(operation);
            let word = scalar::word_of(b, value);
            b.refcount(site, word);
        }
        if let Some(&result) = op.results.first() {
            if operation == RefcountOperation::Retain {
                self.bind(result, value, held);
            } else {
                let none = b.constant(NONE_WORD);
                let none = b.word_value(none);
                self.bind(result, none, HeldCarrier::of(&TirType::None));
            }
        }
    }

    /// `BoxVal`. Its borrowed operand never donates its owner: scalar boxing
    /// creates the result's owner, and a boxed carrier is retained.
    pub(crate) fn box_op(&mut self, b: &mut B, op: &TirOp) {
        let operand = op.operands[0];
        let Some(&result) = op.results.first() else {
            let operands = self.operands();
            if operands.mints_owner(operand) {
                // Boxing can allocate even without a result binding. Keep that
                // failure, then retire only the newly minted owner.
                let boxed = operands.boxed_word(b, operand);
                let release = b.refcount_site(RefcountOperation::Release);
                b.refcount(release, boxed);
            }
            return;
        };
        let word = self.operands().owned_word(b, operand);
        let value = b.word_value(word);
        self.bind(result, value, HeldCarrier::Boxed);
    }

    /// `UnboxVal`: the boxed operand decoded to the result's carrier. A
    /// result held boxed takes its own reference.
    pub(crate) fn unbox_op(&mut self, b: &mut B, op: &TirOp) {
        let Some(&result) = op.results.first() else {
            return;
        };
        let operand = op.operands[0];
        let value = self.operands().resolve(b, operand);
        let target = match op.attrs.get("type") {
            Some(AttrValue::Str(name)) => match name.as_str() {
                "i64" => TirType::I64,
                "f64" => TirType::F64,
                "bool" => TirType::Bool,
                _ => TirType::DynBox,
            },
            // The boxed operand's inner type is a TIR fact; read it from TIR,
            // where a boxed transport cannot have erased it.
            _ => self
                .values
                .repr(result)
                .map(HeldCarrier::machine_type)
                .or_else(|| match self.func.value_types.get(&operand) {
                    Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
                    _ => None,
                })
                .unwrap_or(TirType::I64),
        };
        let target = self.plan.effective_value_type(result, &target);
        let unboxed = scalar::unbox_value(b, value, &target);
        let carrier = HeldCarrier::of(&target);
        if carrier == HeldCarrier::Boxed && !self.plan.carrier_refcount_is_noop(result) {
            let retain = b.refcount_site(RefcountOperation::Retain);
            let word = scalar::word_of(b, unboxed);
            b.refcount(retain, word);
        }
        self.bind(result, unboxed, carrier);
    }
}

#[cfg(test)]
mod tests {
    //! Result binding on the interpreting builder and as Cranelift machine
    //! code. The oracle is the runtime return contract: an owned word binds
    //! or is released once, a borrowed word binds with its own reference, a
    //! void call binds None; reference counting never touches a raw carrier.

    use super::super::differential::{both, kernel};
    use super::super::scalar::tests::{HEAP_INT, Value};
    use super::*;
    use crate::tir::ops::{AttrDict, OpCode};
    use molt_ir::repr::Repr;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;
    /// %0: a boxed object; %1: a raw int proven inline-safe.
    const INPUTS: [Value; 2] = [Value::Word(OBJECT), Value::Word(7)];

    fn func() -> TirFunction {
        TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Value,
        )
    }

    fn plan() -> ProducerCarrierPlan {
        let mut plan = ProducerCarrierPlan::default();
        plan.repr_by_value.insert(ValueId(1), Repr::RawI64Safe);
        plan
    }

    /// The fixture's values from the inputs; %2 and %3 are results.
    fn table<V: Copy>(inputs: &[V]) -> ValueTable<V> {
        let mut values = ValueTable::with_capacity(4);
        values.set_value(ValueId(0), inputs[0]);
        values.set_repr(ValueId(0), HeldCarrier::Boxed);
        values.set_value(ValueId(1), inputs[1]);
        values.set_repr(ValueId(1), HeldCarrier::RawI64);
        values
    }

    fn op(operands: &[u32], results: &[u32]) -> TirOp {
        TirOp {
            dialect: crate::tir::ops::Dialect::Molt,
            opcode: OpCode::Copy,
            operands: operands.iter().map(|&id| ValueId(id)).collect(),
            results: results.iter().map(|&id| ValueId(id)).collect(),
            attrs: AttrDict::default(),
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        }
    }

    #[test]
    fn a_runtime_result_binds_or_retires_by_its_custody() {
        // The model's `molt_int_from_i64` returns the stand-in heap object,
        // so each result is an object the runtime counts.
        let calls = kernel!(func: TirFunction = func(), plan: ProducerCarrierPlan = plan(); |b, inputs| -> () {
            let mut values = table(inputs);
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func,
                plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &super::super::differential::every_runtime_entry(),
                call_sites: &std::cell::Cell::new(0),
            };
            let args = [RuntimeArg::Operand(ValueId(1))];
            for (results, custody) in [
                (&[][..], ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue)),
                (&[2][..], ResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue)),
                (&[3][..], ResultCustody::SideEffect),
            ] {
                let callee = b.runtime_callee(RuntimeFn::IntFromI64);
                let callee = CallTarget::Callee(callee);
                cx.runtime_call(b, &op(&[], results), callee, &args, custody, "t", "c");
            }
            assert!(diagnostics.borrow().is_empty());
            let results = vec![
                cx.values.value(ValueId(2)).unwrap(),
                cx.values.value(ValueId(3)).unwrap(),
            ];
            ((), results)
        });
        let run = both(&calls, &INPUTS);
        assert_eq!(
            run.refcounts,
            [
                // A discarded owned result is released once.
                (RefcountOperation::Release, HEAP_INT),
                // A kept borrowed result takes its own reference.
                (RefcountOperation::Retain, HEAP_INT),
            ],
            "a side-effect result owns nothing"
        );
        assert_eq!(run.results, [Value::Word(HEAP_INT), Value::Word(NONE_WORD)]);
        assert_eq!(run.runtime_calls, [RuntimeFn::IntFromI64; 3]);
    }

    #[test]
    fn reference_counting_touches_only_a_boxed_carrier() {
        let counts = kernel!(func: TirFunction = func(), plan: ProducerCarrierPlan = plan(); |b, inputs| -> Option<HeldCarrier> {
            let mut values = table(inputs);
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func,
                plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &super::super::differential::every_runtime_entry(),
                call_sites: &std::cell::Cell::new(0),
            };
            cx.refcount_op(b, &op(&[1], &[2]), RefcountOperation::Retain);
            cx.refcount_op(b, &op(&[0], &[3]), RefcountOperation::Release);
            let results = vec![
                cx.values.value(ValueId(2)).unwrap(),
                cx.values.value(ValueId(3)).unwrap(),
            ];
            (cx.values.repr(ValueId(2)), results)
        });
        let run = both(&counts, &INPUTS);
        assert_eq!(run.fact, Some(HeldCarrier::RawI64), "a raw int stays raw");
        assert_eq!(
            run.refcounts,
            [(RefcountOperation::Release, OBJECT)],
            "a raw int has no reference count"
        );
        assert_eq!(run.results, [Value::Word(7), Value::Word(NONE_WORD)]);
    }
}
