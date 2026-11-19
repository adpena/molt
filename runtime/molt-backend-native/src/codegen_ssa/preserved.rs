//! Preserved SimpleIR ops in the walker: a `Copy` whose `origin` names an op
//! kind with no TIR opcode of its own (design: `codegen_walker.md`, W2,
//! "preserved ops"). One table per family maps each kind the walker owns to
//! its lowering; a kind no table claims is left to the backend's own routing
//! until its family moves here.

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::IntComparison;
use super::custody::{CallTarget, RuntimeArg};
use super::frame::RecordPlacement;
use super::function::{FunctionCx, ResultCustody};
use super::object::{class_definition_slot, raw_name_pointer};
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, TirOp};

/// The preserved kinds the walker lowers before any backend routing sees
/// them, as one pattern so a backend's exhaustive family match stays total.
macro_rules! walker_preserved_kind_pattern {
    () => {
        molt_ir::OpKind::Abs
            | molt_ir::OpKind::AllocClass
            | molt_ir::OpKind::BindingAlias
            | molt_ir::OpKind::Borrow
            | molt_ir::OpKind::BuiltinType
            | molt_ir::OpKind::Cast
            | molt_ir::OpKind::ClassDef
            | molt_ir::OpKind::ClassSetDefinition
            | molt_ir::OpKind::ConstEllipsis
            | molt_ir::OpKind::ConstNotImplemented
            | molt_ir::OpKind::Contains
            | molt_ir::OpKind::CopyVar
            | molt_ir::OpKind::ExceptionActive
            | molt_ir::OpKind::ExceptionClear
            | molt_ir::OpKind::ExceptionCurrent
            | molt_ir::OpKind::ExceptionFinallyPendingObserver
            | molt_ir::OpKind::ExceptionLast
            | molt_ir::OpKind::ExceptionLastPending
            | molt_ir::OpKind::ExceptionMatchBuiltin
            | molt_ir::OpKind::ExceptionNewBuiltin
            | molt_ir::OpKind::ExceptionNewBuiltinEmpty
            | molt_ir::OpKind::ExceptionNewBuiltinOne
            | molt_ir::OpKind::ExceptionPop
            | molt_ir::OpKind::ExceptionPush
            | molt_ir::OpKind::ExceptionStackClear
            | molt_ir::OpKind::ExceptionStackDepth
            | molt_ir::OpKind::ExceptionStackEnter
            | molt_ir::OpKind::Floordiv
            | molt_ir::OpKind::GenClose
            | molt_ir::OpKind::GenSend
            | molt_ir::OpKind::GenThrow
            | molt_ir::OpKind::GetAttrNameDefault
            | molt_ir::OpKind::GetAttrSpecialObj
            | molt_ir::OpKind::GuardLayout
            | molt_ir::OpKind::GuardTag
            | molt_ir::OpKind::GuardType
            | molt_ir::OpKind::IdentityAlias
            | molt_ir::OpKind::InplaceBitAnd
            | molt_ir::OpKind::InplaceBitOr
            | molt_ir::OpKind::InplaceBitXor
            | molt_ir::OpKind::InplaceDiv
            | molt_ir::OpKind::InplaceFloordiv
            | molt_ir::OpKind::InplaceLshift
            | molt_ir::OpKind::InplaceMod
            | molt_ir::OpKind::InplacePow
            | molt_ir::OpKind::InplaceRshift
            | molt_ir::OpKind::Invert
            | molt_ir::OpKind::Missing
            | molt_ir::OpKind::ObjectNew
            | molt_ir::OpKind::Release
            | molt_ir::OpKind::StatefulLocalsRegister
            | molt_ir::OpKind::StoreVar
            | molt_ir::OpKind::StringFormat
            | molt_ir::OpKind::TypeOf
            | molt_ir::OpKind::Widen
            // The container family (`containers.rs`).
            | molt_ir::OpKind::IterNextUnboxed
            | molt_ir::OpKind::IterNextUnpack2
            | molt_ir::OpKind::Len
            | molt_ir::OpKind::ListNew
            | molt_ir::OpKind::DictNew
            | molt_ir::OpKind::TupleNew
            | molt_ir::OpKind::SetNew
            | molt_ir::OpKind::FrozensetNew
            | molt_ir::OpKind::UnpackSequence
            // The callable family (`callable.rs`).
            | molt_ir::OpKind::BuiltinFunc
            | molt_ir::OpKind::FuncNew
            | molt_ir::OpKind::FuncNewClosure
            | molt_ir::OpKind::FunctionClosureBits
            | molt_ir::OpKind::AsyncgenNew
            | molt_ir::OpKind::CodeSlotSet
            | molt_ir::OpKind::CodeSlotsInit
            | molt_ir::OpKind::CallargsNew
            | molt_ir::OpKind::FrameLocals
            | molt_ir::OpKind::FrameLocalsSet
            | molt_ir::OpKind::Line
    };
}
pub(crate) use walker_preserved_kind_pattern;

/// The runtime entry of a kind that calls it with no operand and binds its
/// word, unowned, as a boxed result. The exception-stack reads and writes
/// return a value the frame does not own; the singletons are immortal.
const fn nullary_runtime(kind: OpKind) -> Option<RuntimeFn> {
    Some(match kind {
        OpKind::ExceptionPush => RuntimeFn::ExceptionPush,
        OpKind::ExceptionStackEnter => RuntimeFn::ExceptionStackEnter,
        OpKind::ExceptionStackDepth => RuntimeFn::ExceptionStackDepth,
        OpKind::ExceptionPop => RuntimeFn::ExceptionPop,
        OpKind::ExceptionStackClear => RuntimeFn::ExceptionStackClear,
        OpKind::ExceptionLast => RuntimeFn::ExceptionLast,
        OpKind::ExceptionActive => RuntimeFn::ExceptionActive,
        OpKind::ExceptionCurrent => RuntimeFn::ExceptionCurrent,
        OpKind::ExceptionClear => RuntimeFn::ExceptionClear,
        OpKind::ConstEllipsis => RuntimeFn::Ellipsis,
        OpKind::ConstNotImplemented => RuntimeFn::NotImplemented,
        OpKind::Missing => RuntimeFn::Missing,
        OpKind::ObjectNew => RuntimeFn::ObjectNew,
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
        OpKind::BuiltinType => (RuntimeFn::BuiltinType, 1, OWNED),
        OpKind::TypeOf => (RuntimeFn::TypeOf, 1, OWNED),
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

impl<B: RecordPlacement> FunctionCx<'_, B> {
    /// Lower a preserved `Copy` when a walker family owns its kind, and
    /// answer whether it did. A malformed op (wrong arity, missing
    /// attribute) is refused, as is a kind no family owns yet.
    pub(crate) fn preserved_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        // The pattern is the one list of kinds the walker owns: a backend's
        // routing maps the same pattern away, so the two never both lower a kind.
        let Some(kind) = op
            .origin
            .filter(|&kind| matches!(kind, walker_preserved_kind_pattern!()))
        else {
            return false;
        };
        self.direct_preserved_op(b, op, kind)
            || self.container_preserved_op(b, op, kind)
            || self.callable_preserved_op(b, op, kind)
    }

    /// The direct family: operators carried as `Copy`, value forwarding,
    /// reference-count aliases, and runtime entries whose symbol or ABI is
    /// not the generic `molt_<kind>` boxed call.
    fn direct_preserved_op(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
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
            OpKind::GuardType | OpKind::GuardTag => self.guard_type_op(b, op),
            // The pending exception, owned by the runtime: an async-work poll
            // services pending work first.
            OpKind::ExceptionLastPending | OpKind::ExceptionFinallyPendingObserver => {
                let word = if op.is_async_work_poll() {
                    self.async_work_poll(b, true, None)
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

    /// `GuardType`/`GuardTag`: the runtime checks the operand's type tag and
    /// profiles a mismatch; the result is the operand itself. A guard the
    /// function proves profile-only (`GUARD_PROFILE_ONLY_ATTR`) calls the
    /// runtime only while the process collects profile counters.
    fn guard_type_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        if op.operands.len() != 2 {
            return false;
        }
        let profile_only = matches!(
            op.attrs.get(crate::tir::ops::GUARD_PROFILE_ONLY_ATTR),
            Some(AttrValue::Bool(true))
        );
        let done = profile_only.then(|| {
            let enabled = b.profile_enabled();
            let zero = b.constant(0);
            let on = b.int_compare(IntComparison::NotEqual, enabled, zero);
            let check = b.create_block("guard_profile");
            let done = b.create_block("guard_done");
            b.branch_if(on, (check, &[]), (done, &[]));
            b.switch_to_block(check);
            done
        });
        let callee = b.runtime_callee(RuntimeFn::GuardType);
        self.positional_runtime_call(
            b,
            op,
            callee,
            ResultCustody::SideEffect,
            op.origin.map_or("guard_type", OpKind::as_str),
            "guard_type",
        );
        if let Some(done) = done {
            b.jump(done, &[]);
            b.switch_to_block(done);
        }
        self.forward_operand(b, op)
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

    use super::super::differential::{Run, both, every_runtime_entry, kernel, machine};
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
            let claimed = cx.preserved_op(b, op);
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
        let run = lower(preserved(Some(OpKind::TypeOf), 1), &[OBJECT]);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::TypeOf,
                &key(&[OBJECT])
            ))]
        );
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

    fn guard(kind: OpKind, profile_only: bool) -> TirOp {
        let mut op = preserved(Some(kind), 2);
        if profile_only {
            op.attrs.insert(
                crate::tir::ops::GUARD_PROFILE_ONLY_ATTR.into(),
                AttrValue::Bool(true),
            );
        }
        op
    }

    #[test]
    fn a_guard_checks_its_tag_and_forwards_its_operand() {
        let tag = inline(5);
        for kind in [OpKind::GuardType, OpKind::GuardTag] {
            let run = lower(guard(kind, false), &[OBJECT, tag]);
            assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)), "{kind}");
            assert_eq!(run.results, [Value::Word(OBJECT)], "{kind}");
            assert_eq!(run.runtime_calls, [RuntimeFn::GuardType], "{kind}");
        }
    }

    #[test]
    #[cfg(feature = "native-backend")]
    fn a_profile_only_guard_calls_the_runtime_only_while_profiling() {
        // The model builds both arms of a branch, so only machine code shows
        // which one ran.
        let tag = inline(5);
        for kind in [OpKind::GuardType, OpKind::GuardTag] {
            let inputs = [Value::Word(OBJECT), Value::Word(tag)];
            let lowering = |op: TirOp| {
                kernel!(op: TirOp = op; |b, inputs| -> () {
                    let func = TirFunction::new(
                        "profile".into(),
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
                    assert!(cx.preserved_op(b, op));
                    ((), values.value(op.results[0]).into_iter().collect())
                })
            };
            let off = machine(&lowering(guard(kind, true)), &inputs, false);
            assert_eq!(off.results, [Value::Word(OBJECT)], "{kind}");
            assert!(
                off.runtime_calls.is_empty(),
                "{kind}: {:?}",
                off.runtime_calls
            );
            let on = crate::native_backend::codegen_builder::machine::run_with_profile(
                &lowering(guard(kind, true)),
                &inputs,
            );
            assert_eq!(on.results, [Value::Word(OBJECT)], "{kind}");
            assert_eq!(on.runtime_calls, [RuntimeFn::GuardType], "{kind}");
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
}
