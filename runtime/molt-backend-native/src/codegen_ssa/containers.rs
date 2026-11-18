//! Containers (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "containers"): fixed list, tuple, dict and slice literals, class
//! definitions, and the iterator entries that write their words through an
//! entry array, written once. Each constructor is one runtime transaction
//! over borrowed operand ranges; the runtime copies and retains every word
//! and returns None only with an exception pending.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::CodegenBuilder;
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use crate::tir::ops::{OpCode, TirOp};
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
            Some(&id) => self.bind(id, done_value, HeldCarrier::of(&TirType::DynBox)),
            None => b.refcount(release, done),
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
    use super::super::scalar::tests::{HEAP_INT, Value, literal_word};
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
