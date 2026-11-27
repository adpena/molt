//! Preserved SimpleIR ops in the walker: a `Copy` whose `origin` names an op
//! kind with no TIR opcode of its own (design: `codegen_walker.md`, W2,
//! "preserved ops"). One table per family maps each kind the walker owns to
//! its lowering; a kind no table claims is left to the backend's own routing
//! until its family moves here.

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::CodegenBuilder;
use super::custody::{CallTarget, RuntimeArg};
use super::function::{FunctionCx, ResultCustody};
use super::object::{class_definition_slot, raw_name_pointer};
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, TirOp};

/// The runtime entry of a kind that calls it with no operand and binds its
/// word, unowned, as a boxed result. The singletons are immortal.
const fn nullary_runtime(kind: OpKind) -> Option<RuntimeFn> {
    Some(match kind {
        OpKind::ExceptionPush => RuntimeFn::ExceptionPush,
        OpKind::ExceptionPop => RuntimeFn::ExceptionPop,
        OpKind::ExceptionStackClear => RuntimeFn::ExceptionStackClear,
        OpKind::ExceptionLast => RuntimeFn::ExceptionLast,
        OpKind::ExceptionActive => RuntimeFn::ExceptionActive,
        OpKind::ExceptionCurrent => RuntimeFn::ExceptionCurrent,
        OpKind::ExceptionClear => RuntimeFn::ExceptionClear,
        OpKind::ConstEllipsis => RuntimeFn::Ellipsis,
        OpKind::ConstNotImplemented => RuntimeFn::NotImplemented,
        OpKind::Missing => RuntimeFn::Missing,
        _ => return None,
    })
}

/// A preserved kind whose runtime symbol differs from `molt_<kind>`, called
/// over every operand at a fixed arity, with the result's custody.
fn positional_runtime(kind: OpKind) -> Option<(RuntimeFn, usize, ResultCustody)> {
    const OWNED: ResultCustody = ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue);
    Some(match kind {
        OpKind::GenSend => (RuntimeFn::GeneratorSend, 2, OWNED),
        OpKind::GenThrow => (RuntimeFn::GeneratorThrow, 2, OWNED),
        OpKind::GenClose => (RuntimeFn::GeneratorClose, 1, OWNED),
        OpKind::GetAttrNameDefault => (RuntimeFn::GetAttrNameDefault, 3, OWNED),
        OpKind::Abs => (
            RuntimeFn::AbsBuiltin,
            1,
            ResultCustody::canonical(RuntimeFn::AbsBuiltin, 1),
        ),
        OpKind::StringFormat => (
            RuntimeFn::FormatBuiltin,
            2,
            ResultCustody::canonical(RuntimeFn::FormatBuiltin, 2),
        ),
        _ => return None,
    })
}

/// The op's integer `value` attribute.
fn int_value(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(value)) => Some(*value),
        _ => None,
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Lower a preserved `Copy` when a walker family owns its kind, and
    /// answer whether it did. A malformed op (wrong arity, missing
    /// attribute) is refused, as is a kind no family owns yet.
    pub(crate) fn preserved_op(&mut self, b: &mut B, op: &TirOp, record: Option<B::Slot>) -> bool {
        let Some(kind) = op.origin else {
            return false;
        };
        self.direct_preserved_op(b, op, kind, record)
            || self.container_preserved_op(b, op, kind)
            || self.callable_preserved_op(b, op, kind)
    }

    /// The direct family: operators carried as `Copy`, value forwarding,
    /// reference-count aliases, and runtime entries whose symbol or ABI is
    /// not the generic `molt_<kind>` boxed call.
    fn direct_preserved_op(
        &mut self,
        b: &mut B,
        op: &TirOp,
        kind: OpKind,
        record: Option<B::Slot>,
    ) -> bool {
        if let Some(function) = nullary_runtime(kind) {
            let word = b
                .call_runtime(function, &[])
                .expect("a nullary preserved entry returns a word");
            if let Some(&result) = op.results.first() {
                let word = b.word_value(word);
                self.bind(result, word, HeldCarrier::Boxed);
            }
            return true;
        }
        if let Some((function, arity, custody)) = positional_runtime(kind) {
            if op.operands.len() != arity {
                return false;
            }
            let callee = b.runtime_callee(function);
            self.positional_runtime_call(b, op, callee, custody, kind.as_str(), kind.as_str());
            return true;
        }
        match kind {
            // `object()`: the shared lifecycle allocation of the root class.
            OpKind::ObjectNew => {
                self.object_alloc(b, op, None);
                true
            }
            // `type(value)` and a builtin class reference: header and thread
            // record loads, the runtime on a miss.
            OpKind::TypeOf | OpKind::BuiltinType => {
                let [operand] = op.operands[..] else {
                    return false;
                };
                self.class_of(b, op, operand, kind == OpKind::TypeOf);
                true
            }
            // Representation-identity ops: operand 0 itself, no new value.
            OpKind::Cast | OpKind::Widen | OpKind::StoreVar | OpKind::CopyVar => {
                self.forward_operand(b, op)
            }
            // `borrow` and the owned-alias kinds mint a reference; the
            // transparent aliases forward the bits.
            OpKind::Borrow | OpKind::IdentityAlias | OpKind::BindingAlias => {
                if op.operands.is_empty() {
                    return false;
                }
                if kind == OpKind::Borrow
                    || crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(kind)
                {
                    self.refcount_op(b, op, RefcountOperation::Retain);
                    return true;
                }
                debug_assert!(
                    crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(kind),
                    "alias `{kind}` lacks generated ownership classification"
                );
                self.forward_operand(b, op)
            }
            // `release` decrements the source and never aliases it: the source
            // may be freed.
            OpKind::Release => {
                if op.operands.is_empty() {
                    return false;
                }
                self.refcount_op(b, op, RefcountOperation::Release);
                true
            }
            OpKind::ClassDef => self.class_def_op(b, op),
            // The pending exception, owned by the runtime: an async-work poll
            // services pending work first.
            OpKind::ExceptionLastPending | OpKind::ExceptionFinallyPendingObserver => {
                let word = if op.is_async_work_poll() {
                    self.async_work_poll(b, true, record, op.frame_location(), None)
                } else {
                    b.call_runtime(RuntimeFn::ExceptionLastPending, &[])
                        .expect("molt_exception_last_pending returns a word")
                };
                if let Some(&result) = op.results.first() {
                    let word = b.word_value(word);
                    self.bind(result, word, HeldCarrier::Boxed);
                }
                true
            }
            OpKind::AllocClass => self.alloc_class_op(b, op),
            OpKind::StatefulLocalsRegister => self.stateful_locals_register_op(b, op),
            // The builtin exception's tag is a raw ABI word; the argument is
            // borrowed.
            OpKind::ExceptionNewBuiltin | OpKind::ExceptionNewBuiltinOne => {
                let (Some(&argument), Some(tag)) = (op.operands.first(), int_value(op)) else {
                    return false;
                };
                let function = if kind == OpKind::ExceptionNewBuiltin {
                    RuntimeFn::ExceptionNewBuiltin
                } else {
                    RuntimeFn::ExceptionNewBuiltinOne
                };
                let tag = b.constant(tag as u64);
                let tag = b.word_value(tag);
                let callee = b.runtime_callee(function);
                self.runtime_call(
                    b,
                    op,
                    CallTarget::Callee(callee),
                    &[RuntimeArg::Word(tag), RuntimeArg::Operand(argument)],
                    ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind.as_str(),
                    kind.as_str(),
                );
                true
            }
            OpKind::ExceptionNewBuiltinEmpty => {
                let Some(tag) = int_value(op) else {
                    return false;
                };
                let tag = b.constant(tag as u64);
                let word = b
                    .call_runtime(RuntimeFn::ExceptionNewBuiltinEmpty, &[tag])
                    .expect("an exception constructor returns a word");
                if let Some(&result) = op.results.first() {
                    let word = b.word_value(word);
                    self.bind(result, word, HeldCarrier::Boxed);
                }
                true
            }
            // The match answer is an unowned boolean.
            OpKind::ExceptionMatchBuiltin => {
                let (Some(&exception), Some(tag)) = (op.operands.first(), int_value(op)) else {
                    return false;
                };
                let tag = b.constant(tag as u64);
                let tag = b.word_value(tag);
                let callee = b.runtime_callee(RuntimeFn::ExceptionMatchBuiltin);
                self.runtime_call(
                    b,
                    op,
                    CallTarget::Callee(callee),
                    &[RuntimeArg::Operand(exception), RuntimeArg::Word(tag)],
                    ResultCustody::Unowned,
                    kind.as_str(),
                    "exception_match_builtin",
                );
                true
            }
            // The stamp names its class statement in `class_definition`
            // (HF-220).
            OpKind::ClassSetDefinition => {
                if op.operands.len() != 1 {
                    return false;
                }
                let slot = class_definition_slot(b, op);
                let slot = b.word_value(slot);
                let callee = b.runtime_callee(RuntimeFn::ClassSetDefinition);
                self.runtime_call(
                    b,
                    op,
                    CallTarget::Callee(callee),
                    &[RuntimeArg::Operand(op.operands[0]), RuntimeArg::Word(slot)],
                    ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind.as_str(),
                    "class_set_definition",
                );
                true
            }
            // A class hint is no pointer proof: the receiver stays tagged.
            OpKind::GuardLayout => {
                if op.operands.len() != 2 {
                    return false;
                }
                let slot = class_definition_slot(b, op);
                let slot = b.word_value(slot);
                let callee = b.runtime_callee(RuntimeFn::GuardLayout);
                self.runtime_call(
                    b,
                    op,
                    CallTarget::Callee(callee),
                    &[
                        RuntimeArg::Operand(op.operands[0]),
                        RuntimeArg::Word(slot),
                        RuntimeArg::Operand(op.operands[1]),
                    ],
                    ResultCustody::Unowned,
                    kind.as_str(),
                    "guard_layout",
                );
                true
            }
            // A special attribute (`__class__`, `__name__`) by its
            // compile-time name; one owned reference on success.
            OpKind::GetAttrSpecialObj => {
                let Some(&object) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Str(name)) = op.attrs.get("s_value") else {
                    return false;
                };
                let (pointer, len) = raw_name_pointer(b, name);
                let len = b.word_value(len);
                let callee = b.runtime_callee(RuntimeFn::GetAttrSpecial);
                self.runtime_call(
                    b,
                    op,
                    CallTarget::Callee(callee),
                    &[
                        RuntimeArg::Operand(object),
                        RuntimeArg::Word(pointer),
                        RuntimeArg::Word(len),
                    ],
                    ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind.as_str(),
                    "get_attr_special_obj",
                );
                true
            }
            // Operators the TIR lift carries as `Copy`, through the same
            // lowering as their own opcode; an in-place kind's boxed slow path
            // reads `origin` and calls `molt_inplace_<op>`.
            OpKind::Floordiv | OpKind::InplaceFloordiv => {
                self.binary_arith_op(b, op, OpKind::Floordiv);
                true
            }
            OpKind::InplaceDiv => {
                self.binary_arith_op(b, op, OpKind::Div);
                true
            }
            OpKind::InplaceMod => {
                self.binary_arith_op(b, op, OpKind::Mod);
                true
            }
            OpKind::InplacePow => {
                self.binary_arith_op(b, op, OpKind::Pow);
                true
            }
            OpKind::Invert => {
                self.unary_op(b, op, OpKind::Invert);
                true
            }
            // A `Copy`-carried `contains` is the affirmative membership test.
            OpKind::Contains => {
                self.containment_op(b, op);
                true
            }
            OpKind::InplaceBitAnd => {
                self.bitwise_op(b, op, OpKind::BitAnd);
                true
            }
            OpKind::InplaceBitOr => {
                self.bitwise_op(b, op, OpKind::BitOr);
                true
            }
            OpKind::InplaceBitXor => {
                self.bitwise_op(b, op, OpKind::BitXor);
                true
            }
            OpKind::InplaceLshift => {
                self.bitwise_op(b, op, OpKind::Lshift);
                true
            }
            OpKind::InplaceRshift => {
                self.bitwise_op(b, op, OpKind::Rshift);
                true
            }
            _ => false,
        }
    }

    /// Bind each result to operand 0's value and carrier, or refuse an op
    /// with no operand.
    fn forward_operand(&mut self, b: &mut B, op: &TirOp) -> bool {
        let Some(&source) = op.operands.first() else {
            return false;
        };
        let operands = self.operands();
        let (value, carrier) = (operands.resolve(b, source), operands.held(source));
        for &result in &op.results {
            self.bind(result, value, carrier);
        }
        true
    }

    /// `AllocClass`: an unpublished instance of the borrowed class, of the
    /// op's `value` size, published initialized before it is exposed or
    /// retired; the result is owned.
    fn alloc_class_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        let Some(size) = int_value(op) else {
            return false;
        };
        if op.operands.len() != 1 || op.results.len() > 1 {
            return false;
        }
        let size = b.constant(size as u64);
        let operands = self.operands();
        let mut custody = operands.begin(b, &op.operands, OpKind::AllocClass.as_str());
        let class = operands.operand(b, &mut custody, op.operands[0]);
        let unpublished = b
            .call_runtime(RuntimeFn::AllocClass, &[size, class])
            .expect("molt_alloc_class returns a word");
        let named = b.word_value(unpublished);
        b.set_name(named, "alloc_class");
        let initialized = b
            .call_runtime(RuntimeFn::ObjectPublishInitialized, &[unpublished])
            .expect("molt_object_publish_initialized returns a word");
        let named = b.word_value(initialized);
        b.set_name(named, "class_initialized");
        let result = operands.finish(b, custody, initialized, "alloc_class_result", |_| {});
        let result = b.word_value(result);
        self.bind_owned(b, op, result);
        true
    }

    /// `StatefulLocalsRegister`: a stateful function's raw code address and
    /// its two borrowed layout tuples. The function's frozen linkage row owns
    /// its parameters; the op's `value`, a Python-level arity claim a task
    /// poll entry omits, must agree with the row where present.
    fn stateful_locals_register_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        if op.operands.len() != 2 || op.results.len() > 1 {
            return false;
        }
        let symbol = match (op.callee, op.attrs.get("s_value")) {
            (Some(callee), _) => self.functions.symbol(callee).to_owned(),
            (None, Some(AttrValue::Str(symbol))) if !symbol.is_empty() => symbol.clone(),
            _ => return false,
        };
        let Some(abi) = self.linkage.get(&symbol).cloned() else {
            self.fatal(format!(
                "stateful function `{symbol}` has no frozen linkage row"
            ));
            return true;
        };
        if let Some(arity) = int_value(op).and_then(|arity| usize::try_from(arity).ok())
            && arity != abi.param_types.len()
        {
            self.fatal(format!(
                "stateful function `{symbol}` claims {arity} parameters; its row has {}",
                abi.param_types.len()
            ));
            return true;
        }
        let callee = b.function_callee(&symbol, &abi);
        let address = b.function_address(callee);
        let address = b.word_value(address);
        let register = b.runtime_callee(RuntimeFn::StatefulLocalsRegister);
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(register),
            &[
                RuntimeArg::Word(address),
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Operand(op.operands[1]),
            ],
            ResultCustody::SideEffect,
            OpKind::StatefulLocalsRegister.as_str(),
            OpKind::StatefulLocalsRegister.as_str(),
        );
        true
    }

    /// `ClassDef`: `s_value` is "bases,attrs,layout_size,layout_version,flags",
    /// and the operands are the name, the bases and the attribute pairs.
    fn class_def_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        let Some(AttrValue::Str(meta)) = op.attrs.get("s_value") else {
            return false;
        };
        let mut parts = meta.split(',');
        let mut next = || parts.next().and_then(|part| part.parse::<i64>().ok());
        let (Some(nbases), Some(nattrs), Some(layout_size), Some(layout_version), Some(flags)) =
            (next(), next(), next(), next(), next())
        else {
            return false;
        };
        let (Ok(nbases), Ok(nattrs)) = (usize::try_from(nbases), usize::try_from(nattrs)) else {
            return false;
        };
        if op.operands.len() != 1 + nbases + nattrs * 2 {
            return false;
        }
        self.class_definition(b, op, nbases, nattrs, layout_size, layout_version, flags);
        true
    }
}

#[cfg(test)]
mod tests {
    //! The direct family on the interpreting builder and as Cranelift machine
    //! code: a nullary entry binds its word unowned, a positional entry
    //! borrows every operand at its arity and refuses another, a forwarding
    //! kind aliases operand 0, and a kind no family owns is left to the
    //! backend.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel};
    use super::super::scalar::tests::{NONE_WORD, Value, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    fn key(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    fn preserved(kind: Option<OpKind>, operands: u32) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: (0..operands).map(ValueId).collect(),
            results: vec![ValueId(operands)],
            attrs: AttrDict::default(),
            source_span: None,
            origin: kind,
            runtime: None,
            callee: None,
        }
    }

    /// Lower `op` over boxed inputs %0..: whether a family claimed it, and
    /// its result's word and carrier when bound.
    fn lower(op: TirOp, inputs: &[u64]) -> Run<(bool, Option<HeldCarrier>)> {
        let lowering = kernel!(op: TirOp = op; |b, inputs| -> (bool, Option<HeldCarrier>) {
            let func = TirFunction::new(
                "preserved".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(8);
            for (index, &input) in inputs.iter().enumerate() {
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), HeldCarrier::Boxed);
            }
            let diagnostics = RefCell::new(Vec::new());
            let runtime = every_runtime_entry();
            let sites = Cell::new(0);
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &runtime,
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &sites,
            };
            let claimed = cx.preserved_op(b, op, None);
            assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
            let result = op.results[0];
            let carrier = values.repr(result);
            ((claimed, carrier), values.value(result).into_iter().collect())
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        both(&lowering, &inputs)
    }

    #[test]
    fn a_nullary_entry_binds_its_word_unowned() {
        for (kind, function) in [
            (OpKind::ExceptionLast, RuntimeFn::ExceptionLast),
            (OpKind::ConstEllipsis, RuntimeFn::Ellipsis),
        ] {
            let run = lower(preserved(Some(kind), 0), &[]);
            assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)), "{kind}");
            assert_eq!(run.results, [Value::Word(literal_word(function, &[]))]);
            assert_eq!(run.runtime_calls, [function]);
            assert!(run.refcounts.is_empty(), "{kind}: the frame owns nothing");
        }
    }

    #[test]
    fn a_positional_entry_borrows_its_operands_at_its_arity_only() {
        let inputs = [OBJECT, inline(5)];
        let run = lower(preserved(Some(OpKind::GenSend), 2), &inputs);
        assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::GeneratorSend,
                &key(&inputs)
            ))]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::GeneratorSend]);
        assert!(
            run.refcounts.is_empty(),
            "borrowed operands: {:?}",
            run.refcounts
        );
        // `type(value)` of an immediate takes its site's miss, the runtime
        // entry, and borrows its operand.
        let run = lower(preserved(Some(OpKind::TypeOf), 1), &[inline(5)]);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::TypeOf,
                &key(&[inline(5)])
            ))]
        );
        assert!(run.refcounts.is_empty(), "{:?}", run.refcounts);
        // `send` takes two operands: one is refused before any call.
        let run = lower(preserved(Some(OpKind::GenSend), 1), &[OBJECT]);
        assert_eq!(run.fact, (false, None));
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_forwarding_kind_aliases_operand_zero() {
        for kind in [
            OpKind::Cast,
            OpKind::Widen,
            OpKind::StoreVar,
            OpKind::CopyVar,
        ] {
            let run = lower(preserved(Some(kind), 1), &[OBJECT]);
            assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)), "{kind}");
            assert_eq!(run.results, [Value::Word(OBJECT)], "{kind}");
            assert!(run.runtime_calls.is_empty() && run.refcounts.is_empty());
        }
    }

    #[test]
    fn the_pending_reader_binds_the_pending_exception_unowned() {
        for kind in [
            OpKind::ExceptionLastPending,
            OpKind::ExceptionFinallyPendingObserver,
        ] {
            let run = lower(preserved(Some(kind), 0), &[]);
            assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)), "{kind}");
            assert_eq!(
                run.results,
                [Value::Word(NONE_WORD)],
                "{kind}: nothing pending answers None"
            );
            assert_eq!(run.runtime_calls, [RuntimeFn::ExceptionLastPending]);
            assert!(run.refcounts.is_empty());
        }
    }

    #[test]
    fn a_kind_no_family_owns_and_a_plain_copy_are_the_backends() {
        for op in [preserved(Some(OpKind::CallAsync), 2), preserved(None, 1)] {
            let run = lower(op.clone(), &[OBJECT, OBJECT][..op.operands.len()]);
            assert_eq!(run.fact, (false, None), "{:?}", op.origin);
            assert!(run.runtime_calls.is_empty());
        }
    }

    #[test]
    fn fixed_boxed_container_calls_have_exact_arity() {
        for (kind, arity) in [
            (OpKind::ListFromRange, 3),
            (OpKind::ListFillNew, 2),
            (OpKind::ListAppend, 2),
            (OpKind::ListExtend, 2),
            (OpKind::TupleFromList, 1),
            (OpKind::SetAdd, 2),
            (OpKind::FrozensetAdd, 2),
            (OpKind::DictSet, 3),
            (OpKind::DictSetdefault, 3),
            (OpKind::DictSetdefaultEmptyList, 2),
            (OpKind::DictGet, 3),
            (OpKind::DictUnpack, 2),
            (OpKind::DictUpdate, 2),
            (OpKind::DictUpdateMissing, 3),
            (OpKind::DictUpdateKwstar, 2),
            (OpKind::DictClear, 1),
            (OpKind::DictCopy, 1),
            (OpKind::DictPopitem, 1),
            (OpKind::Slice, 3),
            (OpKind::SliceNew, 3),
            (OpKind::DictKeys, 1),
            (OpKind::DictValues, 1),
            (OpKind::DictItems, 1),
            (OpKind::Enumerate, 3),
            (OpKind::DictFromObj, 1),
        ] {
            assert!(
                molt_ir::runtime_fn_generated::runtime_boxed_abi(&format!("molt_{kind}"), arity,)
                    .is_some(),
                "deleted handler `{kind}` must have an exact generated boxed ABI",
            );
        }
    }

    #[test]
    fn vector_reductions_use_the_shared_owned_boxed_abi() {
        use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, runtime_boxed_abi};

        for kind in [
            OpKind::VecSum,
            OpKind::VecProd,
            OpKind::VecMin,
            OpKind::VecMax,
        ] {
            let symbol = format!("molt_{kind}");
            let abi = runtime_boxed_abi(&symbol, 3)
                .unwrap_or_else(|| panic!("{kind} must have a three-object boxed ABI"));
            assert_eq!(abi.result, RuntimeBoxedReturn::OwnedValue);
            for arity in [0, 2, 4] {
                assert!(
                    runtime_boxed_abi(&symbol, arity).is_none(),
                    "{kind} must reject arity {arity}",
                );
            }
        }
    }

    #[test]
    fn preserved_runtime_calls_have_exact_arity() {
        for (kind, arity) in [
            (OpKind::Aiter, 1),
            (OpKind::ContextExit, 2),
            (OpKind::ModuleNew, 1),
            (OpKind::ModuleCacheGet, 1),
            (OpKind::ModuleCacheSet, 2),
            (OpKind::ModuleCacheDel, 1),
            (OpKind::ModuleGetAttr, 2),
            (OpKind::ModuleImportFrom, 2),
            (OpKind::ModuleGetGlobal, 2),
            (OpKind::ModuleSetAttr, 3),
            (OpKind::ModuleDelGlobal, 2),
            (OpKind::ModuleDelGlobalIfPresent, 2),
            (OpKind::ExceptionClass, 1),
            (OpKind::ExceptionNew, 2),
            (OpKind::ExceptionStackSetDepth, 1),
            (OpKind::ExceptionStackExit, 1),
            (OpKind::ExceptionSetLast, 1),
            (OpKind::ExceptionContextSet, 1),
            (OpKind::StrFromObj, 1),
            (OpKind::ReprFromObj, 1),
            (OpKind::IntFromObj, 3),
            (OpKind::FloatFromObj, 1),
            (OpKind::AsciiFromObj, 1),
            (OpKind::ComplexFromObj, 3),
            (OpKind::IntFromStrOfObj, 3),
            (OpKind::Ord, 1),
            (OpKind::OrdAt, 2),
            (OpKind::StringJoin, 2),
            (OpKind::Isinstance, 2),
            (OpKind::Issubclass, 2),
            (OpKind::HasAttrName, 2),
            (OpKind::IsCallable, 1),
            (OpKind::CodeNew, 9),
            (OpKind::CallargsPushPos, 2),
            (OpKind::CallargsPushKw, 3),
            (OpKind::CallargsExpandStar, 2),
            (OpKind::CallargsExpandKwstar, 2),
        ] {
            assert!(
                molt_ir::runtime_fn_generated::runtime_boxed_abi(&format!("molt_{kind}"), arity,)
                    .is_some(),
                "retired handler `{kind}` must have an exact generated boxed ABI",
            );
        }
    }
}
