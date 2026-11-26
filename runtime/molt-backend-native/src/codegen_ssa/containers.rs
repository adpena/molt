//! Containers (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "containers"): fixed list, tuple, dict and slice literals, class
//! definitions, and the iterator entries that write their words through an
//! entry array, written once. Each constructor is one runtime transaction
//! over borrowed operand ranges; the runtime copies and retains every word
//! and returns None only with an exception pending.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::WordComparison;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::CodegenBuilder;
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;

/// The boxed word of `None`.
const NONE_WORD: u64 = nanbox::QNAN | nanbox::TAG_NONE;

/// One borrowed word range in an entry array this operation owns: its
/// address and its word count. Every execution reuses the array, so a
/// constructor in a loop never grows the stack. An empty range transports a
/// null address.
fn word_range<B: CodegenBuilder>(b: &mut B, words: &[B::Word], name: &str) -> (B::Word, B::Word) {
    let count = b.constant(words.len() as u64);
    if words.is_empty() {
        return (b.constant(0), count);
    }
    let array = b.entry_slot_array(words.len() as u32, name);
    for (index, &word) in words.iter().enumerate() {
        b.store_slot_at(array, index as u32, word);
    }
    let pointer = b.slot_pointer(array);
    let address = b.address_to_word(pointer);
    let named = b.word_value(address);
    b.set_name(named, &format!("{name}_ptr"));
    (address, count)
}

/// Call constructor `function` with machine words, for its owned word.
fn construct<B: CodegenBuilder>(
    b: &mut B,
    function: RuntimeFn,
    args: &[B::Value],
    name: &str,
) -> B::Word {
    let callee = b.runtime_callee(function);
    let value = b
        .call(callee, args)
        .unwrap_or_else(|| panic!("{} returns a word", function.symbol()));
    b.set_name(value, name);
    b.expect_word(value)
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// `BuildSet` and the preserved frozenset literal: incremental
    /// construction owns the aggregate from allocation to commit, and the
    /// mutator borrows every operand. All entries share one custody whose
    /// operands are requested lazily, per entry, only after the previous
    /// insertion succeeded: a later entry is never boxed before an earlier
    /// entry's hashing and insertion ran, a value repeated across entries
    /// keeps one identity, and a failed box, allocation or insertion releases
    /// the partial aggregate and every minted box while the first exception
    /// stays pending.
    pub(crate) fn set_aggregate_op(
        &mut self,
        b: &mut B,
        op: &TirOp,
        new: RuntimeFn,
        mutate: RuntimeFn,
    ) {
        let view = self.operands();
        let mut custody = view.begin(b, &op.operands, "aggregate");
        // Hash-aggregate constructors share one raw usize-capacity ABI.
        let capacity = b.constant(op.operands.len() as u64);
        let aggregate = b
            .call_runtime(new, &[capacity])
            .expect("an aggregate constructor answers a word");
        let named = b.word_value(aggregate);
        b.set_name(named, "aggregate");
        let none = b.constant(NONE_WORD);
        let created = b.compare(WordComparison::NotEqual, aggregate, none);
        let named = b.bit_value(created);
        b.set_name(named, "aggregate_created");
        view.continue_if(b, &mut custody, created, "ready");
        let insert = b.runtime_callee(mutate);
        let mutation_return = mutate
            .boxed_abi()
            .filter(|abi| abi.arity() == 2)
            .unwrap_or_else(|| {
                panic!(
                    "hash aggregate mutator {} has no boxed ABI",
                    mutate.symbol()
                )
            })
            .result;
        let release = b.refcount_site(RefcountOperation::Release);
        for &operand in &op.operands {
            let word = view.operand(b, &mut custody, operand);
            let args = [b.word_value(aggregate), b.word_value(word)];
            // Keep the aggregate's own owner and retire the mutator's owned
            // result independently.
            let inserted = b
                .call(insert, &args)
                .expect("a hash aggregate mutator answers a word");
            b.set_name(inserted, "aggregate_insert");
            match mutation_return {
                RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue => {
                    let inserted = b.expect_word(inserted);
                    b.refcount(release, inserted);
                }
                RuntimeBoxedReturn::BorrowedValue => {}
                RuntimeBoxedReturn::Void => {
                    panic!(
                        "hash aggregate mutator {} cannot return void",
                        mutate.symbol()
                    )
                }
            }
            // The mutator reports failure only through the exception state.
            let thread = b.thread_record_word();
            let pending = super::pending::exception_pending_word(b, thread);
            let named = b.word_value(pending);
            b.set_name(named, "aggregate_pending_word");
            let zero = b.constant(0);
            let clear = b.compare(WordComparison::Equal, pending, zero);
            let named = b.bit_value(clear);
            b.set_name(named, "aggregate_no_exception");
            view.continue_if(b, &mut custody, clear, "inserted");
        }
        let result = view.finish(b, custody, aggregate, "aggregate_result", |b| {
            let release = b.refcount_site(RefcountOperation::Release);
            b.refcount(release, aggregate);
        });
        let result = b.word_value(result);
        self.bind_owned(b, op, result);
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Lower a fixed container literal: `BuildList`, `BuildTuple`,
    /// `BuildDict` or `BuildSlice`.
    pub(crate) fn container_op(&mut self, b: &mut B, op: &TirOp) {
        match op.opcode {
            OpCode::BuildList => self.fixed_sequence(b, op, RuntimeFn::ListFromValues),
            OpCode::BuildTuple => self.fixed_sequence(b, op, RuntimeFn::TupleFromValues),
            OpCode::BuildDict => self.dict_literal(b, op),
            OpCode::BuildSlice => self.slice_literal(b, op),
            other => unreachable!("{other:?} is not a container literal"),
        }
    }

    fn bind_constructed(&mut self, b: &mut B, op: &TirOp, word: B::Word) {
        let value = b.word_value(word);
        self.bind_owned(b, op, value);
    }

    /// A fixed-arity list or tuple: one constructor over one range.
    pub(crate) fn fixed_sequence(&mut self, b: &mut B, op: &TirOp, constructor: RuntimeFn) {
        let word = self
            .operands()
            .with_words(b, &op.operands, "sequence", |b, words, name| {
                let (address, count) = word_range(b, words, "sequence_values");
                let args = [b.word_value(address), b.word_value(count)];
                construct(b, constructor, &args, name)
            });
        self.bind_constructed(b, op, word);
    }

    /// `{k: v, ...}`: the keys and the values are two ranges of one
    /// constructor transaction.
    pub(crate) fn dict_literal(&mut self, b: &mut B, op: &TirOp) {
        assert_eq!(
            op.operands.len() % 2,
            0,
            "dict literal needs complete pairs"
        );
        let word = self
            .operands()
            .with_words(b, &op.operands, "dictionary", |b, words, name| {
                let keys: Vec<B::Word> = words.iter().step_by(2).copied().collect();
                let values: Vec<B::Word> = words.iter().skip(1).step_by(2).copied().collect();
                let (keys, count) = word_range(b, &keys, "dict_keys");
                let (values, _) = word_range(b, &values, "dict_values");
                let keys = b.word_to_address(keys);
                b.set_name(keys, "dict_keys_ptr");
                let values = b.word_to_address(values);
                b.set_name(values, "dict_values_ptr");
                let args = [keys, values, b.word_value(count)];
                construct(b, RuntimeFn::DictFromPairs, &args, name)
            });
        self.bind_constructed(b, op, word);
    }

    /// `slice(start, stop, step)`: the present bounds are operands; an
    /// omitted bound is boxed None, which needs no owner or range.
    fn slice_literal(&mut self, b: &mut B, op: &TirOp) {
        if op.operands.len() > 3 {
            self.fatal(format!(
                "BuildSlice takes at most start, stop and step, got {} operands",
                op.operands.len()
            ));
            let placeholder = b.placeholder(super::MachineKind::Word);
            for &result in &op.results {
                self.bind(result, placeholder, HeldCarrier::of(&TirType::DynBox));
            }
            return;
        }
        let word = self
            .operands()
            .with_words(b, &op.operands, "slice", |b, words, name| {
                let none = b.constant(NONE_WORD);
                let args: Vec<B::Value> = (0..3)
                    .map(|index| b.word_value(words.get(index).copied().unwrap_or(none)))
                    .collect();
                construct(b, RuntimeFn::SliceNew, &args, name)
            });
        self.bind_constructed(b, op, word);
    }

    /// `class_def(name, *bases, *namespace_pairs)`: the bases and the
    /// namespace pairs are two ranges of one guarded constructor.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn class_definition(
        &mut self,
        b: &mut B,
        op: &TirOp,
        nbases: usize,
        nattrs: usize,
        layout_size: i64,
        layout_version: i64,
        flags: i64,
    ) {
        let word = self
            .operands()
            .with_words(b, &op.operands, "class", |b, words, name| {
                let (bases, base_count) = word_range(b, &words[1..1 + nbases], "class_bases");
                let (attrs, _) = word_range(b, &words[1 + nbases..], "class_attrs");
                let immediates = [
                    nattrs as u64,
                    layout_size as u64,
                    layout_version as u64,
                    flags as u64,
                ];
                let mut args = vec![
                    b.word_value(words[0]),
                    b.word_value(bases),
                    b.word_value(base_count),
                    b.word_value(attrs),
                ];
                for immediate in immediates {
                    let word = b.constant(immediate);
                    args.push(b.word_value(word));
                }
                construct(b, RuntimeFn::GuardedClassDef, &args, name)
            });
        self.bind_constructed(b, op, word);
    }

    /// An iterator entry that writes `words` owned words to an entry array
    /// as its last action and returns the done flag (`molt_iter_next_unboxed`:
    /// one word; `molt_iter_next_unpack2`: two). Binds `op`'s results to the
    /// words, then the flag; a word with no result is released. `false` when
    /// the op is not one iterator operand.
    pub(crate) fn iterator_out_words(
        &mut self,
        b: &mut B,
        op: &TirOp,
        function: RuntimeFn,
        words: u32,
        name: &str,
    ) -> bool {
        let &[iterator] = op.operands.as_slice() else {
            return false;
        };
        let array = b.entry_slot_array(words, &format!("{name}_words"));
        // The iterator is borrowed; a box minted for a raw carrier is
        // released after the call, and a failed box skips it.
        let view = self.operands();
        let mut custody = view.begin(b, &op.operands, name);
        let iterator = view.operand(b, &mut custody, iterator);
        let pointer = b.slot_pointer(array);
        let pointer = b.address_to_word(pointer);
        let named = b.word_value(pointer);
        b.set_name(named, &format!("{name}_words_ptr"));
        let entry = b.runtime_callee(function);
        let call_name = if custody.can_fail() {
            format!("{name}_call")
        } else {
            name.to_string()
        };
        let args = [b.word_value(iterator), b.word_value(pointer)];
        let done = b
            .call(entry, &args)
            .unwrap_or_else(|| panic!("{} returns its done flag", function.symbol()));
        b.set_name(done, &call_name);
        let done = b.expect_word(done);
        // An iterator that could not be boxed was never advanced: its words
        // publish None, never a previous iteration's words.
        let done = view.finish(b, custody, done, name, |b| {
            let none = b.constant(NONE_WORD);
            for index in 0..words {
                b.store_slot_at(array, index, none);
            }
        });
        let release = b.refcount_site(RefcountOperation::Release);
        for index in 0..words {
            let word = b.load_slot_at(array, index);
            let named = b.word_value(word);
            b.set_name(named, &format!("{name}_word_load"));
            match op.results.get(index as usize) {
                Some(&id) => self.bind(id, named, HeldCarrier::of(&TirType::DynBox)),
                None => b.refcount(release, word),
            }
        }
        let done_value = b.word_value(done);
        match op.results.get(words as usize) {
            Some(&id) => self.values.bind_iterator_completion(id, done_value),
            None => b.refcount(release, done),
        }
        true
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// A preserved container op: a `Copy` whose origin names a container
    /// protocol with its own lowering. Exact `molt_<kind>` boxed calls are
    /// not here: the generated runtime ABI dispatch owns their boxing,
    /// availability and result custody. Answers false for a kind this family
    /// does not own, or for a malformed op the caller then refuses.
    pub(crate) fn container_preserved_op(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
        match kind {
            OpKind::IterNextUnboxed => {
                self.iterator_out_words(b, op, RuntimeFn::IterNextUnboxed, 1, "iter_next_unboxed")
            }
            OpKind::IterNextUnpack2 => {
                self.iterator_out_words(b, op, RuntimeFn::IterNextUnpack2, 2, "iter_next_unpack2")
            }
            OpKind::Len => {
                let &[container] = op.operands.as_slice() else {
                    return false;
                };
                let function = len_entry(self.func.value_types.get(&container));
                let abi = function
                    .boxed_abi()
                    .filter(|abi| abi.arity() == 1)
                    .unwrap_or_else(|| {
                        panic!("{} must have a generated boxed ABI", function.symbol())
                    });
                self.boxed_runtime_call(b, op, abi);
                true
            }
            OpKind::ListNew => {
                self.fixed_sequence(b, op, RuntimeFn::ListFromValues);
                true
            }
            OpKind::TupleNew => {
                self.fixed_sequence(b, op, RuntimeFn::TupleFromValues);
                true
            }
            OpKind::DictNew => {
                self.dict_literal(b, op);
                true
            }
            OpKind::SetNew => {
                self.set_aggregate_op(b, op, RuntimeFn::SetNew, RuntimeFn::SetAdd);
                true
            }
            // Frozenset has no TIR opcode of its own; its raw-capacity
            // constructor and in-place mutator share the set protocol.
            OpKind::FrozensetNew => {
                self.set_aggregate_op(b, op, RuntimeFn::FrozensetNew, RuntimeFn::FrozensetAdd);
                true
            }
            OpKind::UnpackSequence => self.unpack_sequence_op(b, op),
            _ => false,
        }
    }
}

/// The `len` entry for a container of TIR type `ty`: an exact container
/// type selects its specialized entry; a boxed word carries no type fact.
fn len_entry(ty: Option<&TirType>) -> RuntimeFn {
    match ty {
        Some(TirType::List(_)) => RuntimeFn::LenList,
        Some(TirType::Str) => RuntimeFn::LenStr,
        Some(TirType::Dict(_, _)) => RuntimeFn::LenDict,
        Some(TirType::Tuple(_)) => RuntimeFn::LenTuple,
        Some(TirType::Set(_)) => RuntimeFn::LenSet,
        _ => RuntimeFn::Len,
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// `UnpackSequence` of `value` targets: the runtime writes each owned
    /// word through an entry array this op owns and answers a status the
    /// op releases. The sequence is borrowed; a sequence that could not be
    /// boxed skips the runtime, so its targets publish None. False for a
    /// malformed op the caller then refuses.
    pub(crate) fn unpack_sequence_op(&mut self, b: &mut B, op: &TirOp) -> bool {
        let &[sequence] = op.operands.as_slice() else {
            return false;
        };
        let Some(&AttrValue::Int(expected)) = op.attrs.get("value") else {
            return false;
        };
        let Ok(expected) = usize::try_from(expected) else {
            return false;
        };
        if expected != op.results.len() {
            return false;
        }
        let array = b.entry_slot_array(expected.max(1) as u32, "unpack_out");
        let pointer = b.slot_pointer(array);
        let pointer = b.address_to_word(pointer);
        let named = b.word_value(pointer);
        b.set_name(named, "unpack_out_ptr");
        let unpack = b.runtime_callee(RuntimeFn::UnpackSequence);
        let view = self.operands();
        let mut custody = view.begin(b, &op.operands, "unpack");
        let sequence = view.operand(b, &mut custody, sequence);
        let count = b.constant(expected as u64);
        let args = [b.word_value(sequence), b.word_value(count), named];
        let status = b
            .call(unpack, &args)
            .expect("molt_unpack_sequence answers a status word");
        b.set_name(status, "unpack_sequence");
        let status = b.expect_word(status);
        let release = b.refcount_site(RefcountOperation::Release);
        b.refcount(release, status);
        let none = b.constant(NONE_WORD);
        view.finish(b, custody, none, "unpack_status", |b| {
            let none = b.constant(NONE_WORD);
            for index in 0..expected as u32 {
                b.store_slot_at(array, index, none);
            }
        });
        for (index, &result) in op.results.iter().enumerate() {
            let word = b.load_slot_at(array, index as u32);
            let named = b.word_value(word);
            b.set_name(named, "unpack_elem");
            self.bind(result, named, HeldCarrier::of(&TirType::DynBox));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    //! Container literals on the interpreting builder and as Cranelift
    //! machine code. The oracle is the runtime model's constructor, which
    //! answers an object determined by exactly the words each range holds.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel, machine};
    use super::super::scalar::tests::{HEAP_INT, UNHASHABLE, Value, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::values::ValueId;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    fn key(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[derive(Clone, Copy)]
    enum Build {
        Literal,
        Class { nbases: usize },
        Iterator { words: u32 },
    }

    /// Lower `opcode` over boxed %0.. (and a raw full-range int where
    /// `raw_last`); the results are %10...
    fn build(
        opcode: OpCode,
        build: Build,
        inputs: &[u64],
        results: usize,
        raw_last: bool,
        on_machine: Option<bool>,
    ) -> Run<()> {
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..inputs.len() as u32).map(ValueId).collect(),
            results: (0..results as u32)
                .map(|index| ValueId(10 + index))
                .collect(),
            attrs: AttrDict::default(),
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        };
        let lower = kernel!(op: TirOp = op, build: Build = build, raw_last: bool = raw_last; |b, inputs| -> () {
            let func = TirFunction::new(
                "containers".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(16);
            for (index, &input) in inputs.iter().enumerate() {
                let carrier = if *raw_last && index + 1 == inputs.len() {
                    HeldCarrier::RawI64
                } else {
                    HeldCarrier::Boxed
                };
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), carrier);
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
            match *build {
                Build::Literal => cx.container_op(b, op),
                Build::Class { nbases } => {
                    let nattrs = (op.operands.len() - 1 - nbases) / 2;
                    cx.class_definition(b, op, nbases, nattrs, 32, 7, 1);
                }
                Build::Iterator { words } => {
                    let function = if words == 1 {
                        RuntimeFn::IterNextUnboxed
                    } else {
                        RuntimeFn::IterNextUnpack2
                    };
                    assert!(cx.iterator_out_words(b, op, function, words, "iter"));
                    for (position, &id) in op.results.iter().enumerate() {
                        assert_eq!(cx.values.is_boxed_bool_or_none(id), position == words as usize);
                    }
                }
            }
            assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
            let results = op.results.iter().map(|&id| values.value(id).unwrap()).collect();
            ((), results)
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        match on_machine {
            None => both(&lower, &inputs),
            Some(fail_allocation) => machine(&lower, &inputs, fail_allocation),
        }
    }

    #[test]
    fn a_literal_hands_its_constructor_exactly_its_words() {
        let words = [OBJECT, inline(1), inline(2)];
        for (opcode, constructor) in [
            (OpCode::BuildList, RuntimeFn::ListFromValues),
            (OpCode::BuildTuple, RuntimeFn::TupleFromValues),
        ] {
            let run = build(opcode, Build::Literal, &words, 1, false, None);
            assert_eq!(
                run.results,
                [Value::Word(literal_word(constructor, &key(&words)))]
            );
            let empty = build(opcode, Build::Literal, &[], 1, false, None);
            assert_eq!(empty.results, [Value::Word(literal_word(constructor, &[]))]);
        }
        let pairs = [inline(1), OBJECT, inline(2), inline(3)];
        let run = build(OpCode::BuildDict, Build::Literal, &pairs, 1, false, None);
        let mut expected = key(&[inline(1), inline(2)]);
        expected.extend(key(&[OBJECT, inline(3)]));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::DictFromPairs,
                &expected
            ))]
        );
        let none = (0x7ff8 << 48) | (3 << 48);
        let run = build(
            OpCode::BuildSlice,
            Build::Literal,
            &[inline(1)],
            1,
            false,
            None,
        );
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::SliceNew,
                &key(&[inline(1), none, none])
            ))]
        );
    }

    #[test]
    fn a_class_passes_its_bases_and_namespace_as_two_ranges() {
        let words = [inline(9), OBJECT, inline(4), inline(5)];
        let run = build(
            OpCode::Copy,
            Build::Class { nbases: 1 },
            &words,
            1,
            false,
            None,
        );
        // name, bases (one), attrs (one name and value), size, version, flags.
        let mut model = key(&[inline(9), OBJECT, inline(4), inline(5)]);
        model.extend(key(&[32, 7, 1]));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::GuardedClassDef,
                &model
            ))]
        );
    }

    #[test]
    fn a_failed_operand_box_skips_the_constructor() {
        let run = build(
            OpCode::BuildList,
            Build::Literal,
            &[OBJECT, 1 << 50],
            1,
            true,
            Some(false),
        );
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::ListFromValues,
                &key(&[OBJECT, HEAP_INT])
            ))]
        );
        assert_eq!(run.refcounts, [(RefcountOperation::Release, HEAP_INT)]);
        let run = build(
            OpCode::BuildList,
            Build::Literal,
            &[OBJECT, 1 << 50],
            1,
            true,
            Some(true),
        );
        assert_eq!(run.results, [Value::Word(NONE_WORD)]);
        assert_eq!(
            run.runtime_calls,
            [RuntimeFn::IntFromI64],
            "no constructor call"
        );
    }

    /// The preserved container op `kind` (a `Copy` with that origin) over
    /// boxed %0.. typed `operand_ty`, with results %10..; the fact is
    /// whether the family claimed it. `value` is the op's `value` attribute.
    fn preserved(
        kind: OpKind,
        inputs: &[u64],
        results: usize,
        operand_ty: TirType,
        value: Option<i64>,
        on_machine: bool,
    ) -> Run<bool> {
        let mut attrs = AttrDict::default();
        if let Some(value) = value {
            attrs.insert("value".into(), AttrValue::Int(value));
        }
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: (0..inputs.len() as u32).map(ValueId).collect(),
            results: (0..results as u32)
                .map(|index| ValueId(10 + index))
                .collect(),
            attrs,
            source_span: None,
            origin: Some(kind),
            runtime: None,
            callee: None,
        };
        let lower = kernel!(op: TirOp = op, kind: OpKind = kind, operand_ty: TirType = operand_ty; |b, inputs| -> bool {
            let mut func = TirFunction::new(
                "preserved_containers".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(16);
            for (index, &input) in inputs.iter().enumerate() {
                let id = ValueId(index as u32);
                func.value_types.insert(id, operand_ty.clone());
                values.set_value(id, input);
                values.set_repr(id, HeldCarrier::Boxed);
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
            let claimed = cx.container_preserved_op(b, op, *kind);
            assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
            let results = if claimed {
                op.results.iter().map(|&id| values.value(id).unwrap()).collect()
            } else {
                Vec::new()
            };
            (claimed, results)
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        if on_machine {
            machine(&lower, &inputs, false)
        } else {
            both(&lower, &inputs)
        }
    }

    #[test]
    fn a_preserved_literal_hands_its_constructor_exactly_its_words() {
        let words = [OBJECT, inline(1), inline(2), inline(3)];
        for (kind, constructor) in [
            (OpKind::ListNew, RuntimeFn::ListFromValues),
            (OpKind::TupleNew, RuntimeFn::TupleFromValues),
        ] {
            let run = preserved(kind, &words, 1, TirType::DynBox, None, false);
            assert!(run.fact);
            assert_eq!(
                run.results,
                [Value::Word(literal_word(constructor, &key(&words)))]
            );
            assert_eq!(run.runtime_calls, [constructor]);
        }
        let run = preserved(OpKind::DictNew, &words, 1, TirType::DynBox, None, false);
        assert!(run.fact);
        let mut expected = key(&[OBJECT, inline(2)]);
        expected.extend(key(&[inline(1), inline(3)]));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::DictFromPairs,
                &expected
            ))]
        );
    }

    #[test]
    fn a_preserved_len_calls_the_entry_its_container_type_selects() {
        for (ty, entry) in [
            (TirType::List(Box::new(TirType::DynBox)), RuntimeFn::LenList),
            (TirType::DynBox, RuntimeFn::Len),
        ] {
            let run = preserved(OpKind::Len, &[OBJECT], 1, ty, None, false);
            assert!(run.fact);
            assert_eq!(run.runtime_calls, [entry]);
            assert_eq!(
                run.results,
                [Value::Word(literal_word(entry, &key(&[OBJECT])))]
            );
        }
        // Not one container operand: refused before any code.
        let run = preserved(
            OpKind::Len,
            &[OBJECT, OBJECT],
            1,
            TirType::DynBox,
            None,
            false,
        );
        assert!(!run.fact);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_preserved_iterator_and_unpack_bind_the_words_the_runtime_wrote() {
        let not_done = (0x7ff8 << 48) | (2 << 48);
        for (kind, function, words) in [
            (OpKind::IterNextUnboxed, RuntimeFn::IterNextUnboxed, 1u8),
            (OpKind::IterNextUnpack2, RuntimeFn::IterNextUnpack2, 2),
        ] {
            let run = preserved(
                kind,
                &[OBJECT],
                words as usize + 1,
                TirType::DynBox,
                None,
                false,
            );
            assert!(run.fact);
            let mut expected: Vec<Value> = (0..words)
                .map(|index| {
                    let mut key = OBJECT.to_le_bytes().to_vec();
                    key.push(index);
                    Value::Word(literal_word(function, &key))
                })
                .collect();
            expected.push(Value::Word(not_done));
            assert_eq!(run.results, expected);
        }
        let run = preserved(
            OpKind::UnpackSequence,
            &[OBJECT],
            2,
            TirType::DynBox,
            Some(2),
            false,
        );
        assert!(run.fact);
        let expected: Vec<Value> = (0..2u8)
            .map(|index| {
                let mut key = OBJECT.to_le_bytes().to_vec();
                key.push(index);
                Value::Word(literal_word(RuntimeFn::UnpackSequence, &key))
            })
            .collect();
        assert_eq!(run.results, expected);
        // A target count that disagrees with the results is refused.
        let run = preserved(
            OpKind::UnpackSequence,
            &[OBJECT],
            2,
            TirType::DynBox,
            Some(3),
            false,
        );
        assert!(!run.fact);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_preserved_set_and_frozenset_insert_each_entry_into_their_own_aggregate() {
        for (kind, new, add) in [
            (OpKind::SetNew, RuntimeFn::SetNew, RuntimeFn::SetAdd),
            (
                OpKind::FrozensetNew,
                RuntimeFn::FrozensetNew,
                RuntimeFn::FrozensetAdd,
            ),
        ] {
            let run = preserved(kind, &[OBJECT, inline(2)], 1, TirType::DynBox, None, true);
            assert!(run.fact);
            assert_eq!(
                run.results,
                [Value::Word(literal_word(new, &2u64.to_le_bytes()))]
            );
            assert_eq!(run.inserted, [OBJECT, inline(2)]);
            assert_eq!(run.runtime_calls, [new, add, add]);
        }
    }

    #[test]
    fn a_kind_outside_the_container_family_is_left_to_its_owner() {
        for kind in [OpKind::FuncNew, OpKind::ListAppend, OpKind::Floordiv] {
            let run = preserved(kind, &[OBJECT], 1, TirType::DynBox, None, false);
            assert!(!run.fact, "{kind}");
            assert!(run.runtime_calls.is_empty());
        }
    }

    /// `BuildSet` (or the frozenset literal) over `operands`, ids into
    /// `inputs` held as their carriers, as Cranelift machine code: each
    /// insertion's exception read rejoins through a block parameter, which
    /// only a machine follows.
    fn build_set(
        inputs: &[(u64, HeldCarrier)],
        operands: &[u32],
        frozen: bool,
        fail_allocation: bool,
    ) -> Run<()> {
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildSet,
            operands: operands.iter().map(|&id| ValueId(id)).collect(),
            results: vec![ValueId(10)],
            attrs: AttrDict::default(),
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        };
        let carriers: Vec<HeldCarrier> = inputs.iter().map(|&(_, carrier)| carrier).collect();
        let lower = kernel!(op: TirOp = op, carriers: Vec<HeldCarrier> = carriers, frozen: bool = frozen; |b, inputs| -> () {
            let func = TirFunction::new(
                "containers".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(16);
            for (index, (&input, &carrier)) in inputs.iter().zip(carriers).enumerate() {
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), carrier);
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
            let (new, add) = if *frozen {
                (RuntimeFn::FrozensetNew, RuntimeFn::FrozensetAdd)
            } else {
                (RuntimeFn::SetNew, RuntimeFn::SetAdd)
            };
            cx.set_aggregate_op(b, op, new, add);
            assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
            ((), vec![values.value(ValueId(10)).unwrap()])
        });
        let inputs: Vec<Value> = inputs.iter().map(|&(word, _)| Value::Word(word)).collect();
        machine(&lower, &inputs, fail_allocation)
    }

    /// The model's hash aggregate of `capacity` entries.
    fn aggregate(frozen: bool, capacity: u64) -> u64 {
        let new = if frozen {
            RuntimeFn::FrozensetNew
        } else {
            RuntimeFn::SetNew
        };
        literal_word(new, &capacity.to_le_bytes())
    }

    #[test]
    fn a_set_boxes_each_entry_only_after_the_previous_insertion() {
        // %1 is raw and repeated: one box, minted after %0's insertion,
        // inserted twice and released once.
        let inputs = [
            (OBJECT, HeldCarrier::Boxed),
            (1 << 50, HeldCarrier::RawI64),
            (inline(2), HeldCarrier::Boxed),
        ];
        for frozen in [false, true] {
            let (new, add) = if frozen {
                (RuntimeFn::FrozensetNew, RuntimeFn::FrozensetAdd)
            } else {
                (RuntimeFn::SetNew, RuntimeFn::SetAdd)
            };
            let run = build_set(&inputs, &[0, 1, 2, 1], frozen, false);
            assert_eq!(run.results, [Value::Word(aggregate(frozen, 4))]);
            assert_eq!(run.inserted, [OBJECT, HEAP_INT, inline(2), HEAP_INT]);
            assert_eq!(
                run.runtime_calls,
                [new, add, RuntimeFn::IntFromI64, add, add, add],
                "a clear exception hint needs no confirmation"
            );
            assert_eq!(run.refcounts, [(RefcountOperation::Release, HEAP_INT)]);
        }
    }

    #[test]
    fn a_failed_insertion_releases_the_partial_set_and_boxes_no_later_entry() {
        let inputs = [
            (1 << 50, HeldCarrier::RawI64),
            (UNHASHABLE, HeldCarrier::Boxed),
            (1 << 51, HeldCarrier::RawI64),
        ];
        let run = build_set(&inputs, &[0, 1, 2], false, false);
        assert_eq!(run.results, [Value::Word(NONE_WORD)]);
        assert_eq!(run.inserted, [HEAP_INT]);
        assert_eq!(
            run.runtime_calls,
            [
                RuntimeFn::SetNew,
                RuntimeFn::IntFromI64,
                RuntimeFn::SetAdd,
                RuntimeFn::SetAdd,
                RuntimeFn::ExceptionPending,
            ],
            "the set hint is confirmed once; %2 is never boxed or inserted"
        );
        assert_eq!(
            run.refcounts,
            [
                (RefcountOperation::Release, aggregate(false, 3)),
                (RefcountOperation::Release, HEAP_INT),
            ]
        );
    }

    #[test]
    fn a_failed_set_allocation_boxes_and_inserts_nothing() {
        let inputs = [(OBJECT, HeldCarrier::Boxed), (1 << 50, HeldCarrier::RawI64)];
        let run = build_set(&inputs, &[0, 1], false, true);
        assert_eq!(run.results, [Value::Word(NONE_WORD)]);
        assert!(run.inserted.is_empty());
        assert_eq!(run.runtime_calls, [RuntimeFn::SetNew]);
        assert!(run.refcounts.is_empty());
    }

    #[test]
    fn an_iterator_binds_its_words_then_its_flag() {
        let not_done = (0x7ff8 << 48) | (2 << 48);
        for words in [1, 2] {
            let function = if words == 1 {
                RuntimeFn::IterNextUnboxed
            } else {
                RuntimeFn::IterNextUnpack2
            };
            let run = build(
                OpCode::Copy,
                Build::Iterator { words },
                &[OBJECT],
                words as usize + 1,
                false,
                None,
            );
            let mut expected: Vec<Value> = (0..words)
                .map(|index| {
                    let mut key = OBJECT.to_le_bytes().to_vec();
                    key.push(index as u8);
                    Value::Word(literal_word(function, &key))
                })
                .collect();
            expected.push(Value::Word(not_done));
            assert_eq!(run.results, expected, "{words} words");
        }
    }
}
