//! Operand custody and runtime calls (design:
//! `docs/design/foundation/codegen_walker.md`, W2, "operand custody and
//! runtime calls").
//!
//! A lowered operation that borrows boxed operands opens one custody before
//! its first failure edge. Only a raw integer held without an inline-safe
//! proof can mint a heap owner when it is boxed, so only such an operand gets
//! an owner slot: a static entry slot, reset to None when custody opens, so a
//! failure taken before the operand is requested releases nothing and a loop
//! never releases a previous iteration's owner. Operands are boxed at their
//! first request; a later request of the same value reuses its word, so one
//! operation transports one identity per value. A failed mint and every
//! consumer step routed through [`Operands::continue_if`] join one failure
//! block, which skips all later work and keeps the first exception pending.
//! Custody creates blocks only when something can fail, so an operation with
//! nothing that can fail leaves the control flow unchanged.

use std::cell::RefCell;
use std::collections::BTreeMap;

use molt_codegen_abi as nanbox;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::{WordBuilder, WordComparison};
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, MachineKind, SiteBody};
use super::scalar;
use super::value_table::{HeldCarrier, ValueTable};
use crate::representation_plan::ProducerCarrierPlan;
use crate::tir::values::ValueId;

/// The boxed word of `None`.
const NONE_WORD: u64 = nanbox::QNAN | nanbox::TAG_NONE;

/// What a borrowed call enters: a callee, or a site body the backend places
/// (`CodegenBuilder::site`).
#[derive(Clone, Copy)]
pub(crate) enum CallTarget<C> {
    Callee(C),
    Site(SiteBody),
}

/// A runtime-call argument: a borrowed operand of the lowered operation, or a
/// machine word the runtime ABI fixes (an immediate, address or length).
#[derive(Clone, Copy)]
pub(crate) enum RuntimeArg<V> {
    Operand(ValueId),
    Word(V),
}

/// One operation's custody of borrowed boxed operands (see the module
/// documentation), over a backend's words, entry slots and blocks.
pub(crate) struct BorrowedOperands<Word, Slot, Block> {
    /// The operation's debug name; blocks and values custody makes carry it.
    label: String,
    /// Operand requests so far: the position of the next one.
    requests: usize,
    owners: BTreeMap<ValueId, Slot>,
    words: BTreeMap<ValueId, Word>,
    adopted_objects: Vec<Word>,
    adopted_callable: Option<Word>,
    abort: Option<Block>,
}

/// The custody a builder `B` keeps.
pub(crate) type Custody<B> = BorrowedOperands<
    <B as WordBuilder>::Word,
    <B as CodegenBuilder>::Slot,
    <B as CodegenBuilder>::Block,
>;

impl<Word, Slot, Block: Copy> BorrowedOperands<Word, Slot, Block> {
    /// Whether a failure edge joins this custody, making its result a
    /// rejoined value rather than the committed word itself.
    pub(crate) fn can_fail(&self) -> bool {
        self.abort.is_some()
    }

    /// The failure block, created at its first use.
    fn abort(&mut self, create: impl FnOnce(&str) -> Block) -> Block {
        let label = &self.label;
        *self
            .abort
            .get_or_insert_with(|| create(&format!("{label}_abort")))
    }
}

/// What custody reads: the lowered values with their held carriers, the
/// carrier plan, and the function's diagnostics.
pub(crate) struct Operands<'a, B: CodegenBuilder> {
    pub(crate) function: &'a str,
    pub(crate) values: &'a ValueTable<B::Value>,
    pub(crate) plan: &'a ProducerCarrierPlan,
    pub(crate) diagnostics: &'a RefCell<Vec<String>>,
}

impl<B: CodegenBuilder> Operands<'_, B> {
    /// The lowered value of `id`. A value used before its definition is a
    /// fatal diagnostic; a placeholder keeps the function well formed for the
    /// next one.
    pub(crate) fn resolve(&self, b: &mut B, id: ValueId) -> B::Value {
        match self.values.value(id) {
            Some(value) => value,
            None => {
                self.diagnostics.borrow_mut().push(format!(
                    "{}: ValueId %{} was used before being defined during lowering",
                    self.function, id.0
                ));
                b.placeholder(MachineKind::Word)
            }
        }
    }

    /// The carrier `id` is held in; a value with none recorded is a boxed
    /// word.
    pub(crate) fn held(&self, id: ValueId) -> HeldCarrier {
        self.values.repr(id).unwrap_or(HeldCarrier::Boxed)
    }

    /// Whether boxing `id` can allocate: a raw integer held without an
    /// inline-safe proof may mint a heap BigInt owner. Known before boxing,
    /// so custody initializes its owner slots before the first failure edge.
    pub(crate) fn mints_owner(&self, id: ValueId) -> bool {
        self.values.repr(id) == Some(HeldCarrier::RawI64) && !self.plan.is_inline_safe_int(id)
    }

    /// Representation boxing of a lowered value. A full-width integer boxed
    /// here may mint a heap owner that nobody tracks, so a borrowing
    /// consumer goes through custody ([`Self::operand`]) and a storing or
    /// returning consumer through [`Self::owned_word`].
    pub(crate) fn boxed_word(&self, b: &mut B, id: ValueId) -> B::Word {
        let value = self.resolve(b, id);
        let held = self.held(id);
        if held == HeldCarrier::RawI64 && self.plan.is_inline_safe_int(id) {
            let raw = scalar::word_of(b, value);
            return nanbox::boxed_word::generate_box_inline_int(b, raw);
        }
        scalar::box_value(b, value, &held.machine_type())
    }

    /// A +1 owned word for a value the consumer stores or returns rather
    /// than borrows. A boxed carrier is retained; a raw carrier is boxed, and
    /// a minted heap integer already is the new owner, so it is not retained
    /// again. A failed mint yields None with MemoryError pending for the
    /// enclosing operation's exception check.
    pub(crate) fn owned_word(&self, b: &mut B, id: ValueId) -> B::Word {
        let word = self.boxed_word(b, id);
        if self.held(id) == HeldCarrier::Boxed && !self.plan.boxed_refcount_is_noop(id) {
            let retain = b.refcount_site(RefcountOperation::Retain);
            b.refcount(retain, word);
        }
        word
    }

    /// Open custody over every operand the operation may borrow. It must
    /// precede the operation's first failure edge.
    pub(crate) fn begin(&self, b: &mut B, operands: &[ValueId], label: &str) -> Custody<B> {
        let mut owners = BTreeMap::new();
        for &operand in operands {
            if !owners.contains_key(&operand) && self.mints_owner(operand) {
                let slot = b.entry_slot(&format!("{label}_owner"));
                let none = b.constant(NONE_WORD);
                b.store_slot(slot, none);
                owners.insert(operand, slot);
            }
        }
        BorrowedOperands {
            label: label.to_owned(),
            requests: 0,
            owners,
            words: BTreeMap::new(),
            adopted_objects: Vec::new(),
            adopted_callable: None,
            abort: None,
        }
    }

    /// Open call custody before any boxing can fail. The call already owns
    /// one reference per adopted object position; their words are read now,
    /// so the failure cleanup dominates every later allocation failure. Raw
    /// values have no owner yet and box like borrowed operands.
    pub(crate) fn begin_call(
        &self,
        b: &mut B,
        operands: &[ValueId],
        adopted: &[ValueId],
        callable: Option<ValueId>,
        label: &str,
    ) -> Custody<B> {
        let mut adopted_objects = Vec::new();
        for &id in adopted {
            if self.held(id) == HeldCarrier::Boxed {
                let value = self.resolve(b, id);
                adopted_objects.push(scalar::word_of(b, value));
            }
        }
        let adopted_callable = match callable {
            Some(id) if self.held(id) == HeldCarrier::Boxed => {
                let value = self.resolve(b, id);
                Some(scalar::word_of(b, value))
            }
            _ => None,
        };
        let mut custody = self.begin(b, operands, label);
        custody.adopted_objects = adopted_objects;
        custody.adopted_callable = adopted_callable;
        custody
    }

    /// Eager custody for a consumer that needs every operand word up front
    /// (a fixed constructor): operands are requested in order, and
    /// `construct` receives one word per position and the name its value
    /// carries. The result is the constructed word, or None with the first
    /// exception pending when an operand could not be boxed.
    pub(crate) fn with_words(
        &self,
        b: &mut B,
        operands: &[ValueId],
        label: &str,
        construct: impl FnOnce(&mut B, &[B::Word], &str) -> B::Word,
    ) -> B::Word {
        let mut custody = self.begin(b, operands, label);
        let words: Vec<B::Word> = operands
            .iter()
            .map(|&operand| self.operand(b, &mut custody, operand))
            .collect();
        let name = if custody.can_fail() {
            format!("{label}_constructed")
        } else {
            format!("{label}_result")
        };
        let constructed = construct(b, &words, &name);
        self.finish(b, custody, constructed, &format!("{label}_result"), |_| {})
    }

    /// The operand's boxed word, boxed at its first request.
    pub(crate) fn operand(&self, b: &mut B, custody: &mut Custody<B>, operand: ValueId) -> B::Word {
        let position = custody.requests;
        custody.requests += 1;
        if let Some(&word) = custody.words.get(&operand) {
            return word;
        }
        let word = match custody.owners.get(&operand).copied() {
            Some(slot) => self.mint_integer(b, custody, operand, slot, position),
            None => {
                assert!(
                    !self.mints_owner(operand),
                    "operand %{} can mint an owner but was not declared when custody opened",
                    operand.0
                );
                self.boxed_word(b, operand)
            }
        };
        custody.words.insert(operand, word);
        word
    }

    /// Box a raw full-width integer for a borrowing consumer. Only the heap
    /// arm can allocate or fail, and `molt_int_from_i64` returns None exactly
    /// when it raised, so that arm records the owner and joins the failure
    /// block on None. The inline arm needs neither: no exception-state read
    /// runs on the hot path.
    fn mint_integer(
        &self,
        b: &mut B,
        custody: &mut Custody<B>,
        operand: ValueId,
        slot: B::Slot,
        position: usize,
    ) -> B::Word {
        let value = self.resolve(b, operand);
        let raw = scalar::word_of(b, value);
        let fits = nanbox::boxed_word::generate_inline_int_fits(b, raw);
        let inline_block = b.create_block("box_int_inline");
        let heap_block = b.create_block("box_int_bigint");
        let boxed_block = b.create_block(&format!("{}_operand{position}", custody.label));
        let boxed = b.append_block_param(boxed_block, MachineKind::Word);
        b.set_name(boxed, "boxed_int");
        b.branch_if(fits, (inline_block, &[]), (heap_block, &[]));
        b.switch_to_block(inline_block);
        let inline = nanbox::boxed_word::generate_box_inline_int(b, raw);
        let inline = b.word_value(inline);
        b.jump(boxed_block, &[inline]);
        b.switch_to_block(heap_block);
        let heap = scalar::box_int_by_runtime(b, raw);
        b.store_slot(slot, heap);
        let none = b.constant(NONE_WORD);
        let failed = b.compare(WordComparison::Equal, heap, none);
        let failed_bit = b.bit_value(failed);
        b.set_name(failed_bit, &format!("{}_box_failed", custody.label));
        let abort = custody.abort(|name| b.create_block(name));
        let heap = b.word_value(heap);
        b.branch_if(failed, (abort, &[]), (boxed_block, &[heap]));
        b.switch_to_block(boxed_block);
        b.expect_word(boxed)
    }

    /// Continue past a consumer step only when `ok` holds; otherwise join
    /// the custody's failure block.
    pub(crate) fn continue_if(
        &self,
        b: &mut B,
        custody: &mut Custody<B>,
        ok: B::Predicate,
        step: &str,
    ) {
        let abort = custody.abort(|name| b.create_block(name));
        let next = b.create_block(&format!("{}_{step}", custody.label));
        b.branch_if(ok, (next, &[]), (abort, &[]));
        b.switch_to_block(next);
    }

    /// Rejoin the committed path with the failure block, then release every
    /// initialized owner once, after the consumer has retained what it
    /// keeps. The result is `committed`, or None with the first exception
    /// pending. `on_abort` runs on the failure path only, for consumer-owned
    /// cleanup such as releasing a partly built aggregate.
    pub(crate) fn finish(
        &self,
        b: &mut B,
        custody: Custody<B>,
        committed: B::Word,
        result_name: &str,
        on_abort: impl FnOnce(&mut B),
    ) -> B::Word {
        let result = match custody.abort {
            None => committed,
            Some(abort) => {
                let merge = b.create_block(&format!("{}_merge", custody.label));
                let result = b.append_block_param(merge, MachineKind::Word);
                b.set_name(result, result_name);
                let committed = b.word_value(committed);
                b.jump(merge, &[committed]);
                b.switch_to_block(abort);
                on_abort(b);
                release_call_inputs(b, custody.adopted_callable, &custody.adopted_objects);
                let none = b.constant(NONE_WORD);
                let none = b.word_value(none);
                b.jump(merge, &[none]);
                b.switch_to_block(merge);
                b.expect_word(result)
            }
        };
        release_owners(b, &custody);
        result
    }

    /// One call whose object arguments are borrowed operands of the lowered
    /// operation and whose machine words pass unchanged. The result is the
    /// callee's word (None for a void callee), or None with the first
    /// exception pending when an argument could not be boxed. With
    /// `retain_result`, a borrowed word the caller keeps takes its own
    /// reference on the committed path, before the argument owners are
    /// released (it may alias one of them).
    pub(crate) fn call(
        &self,
        b: &mut B,
        target: CallTarget<B::Callee>,
        args: &[RuntimeArg<B::Value>],
        retain_result: bool,
        label: &str,
        call_name: &str,
    ) -> B::Word {
        let operands: Vec<ValueId> = args
            .iter()
            .filter_map(|arg| match *arg {
                RuntimeArg::Operand(operand) => Some(operand),
                RuntimeArg::Word(_) => None,
            })
            .collect();
        let mut custody = self.begin(b, &operands, label);
        let mut call_args = Vec::with_capacity(args.len());
        for arg in args {
            call_args.push(match *arg {
                RuntimeArg::Operand(operand) => {
                    let word = self.operand(b, &mut custody, operand);
                    b.word_value(word)
                }
                RuntimeArg::Word(value) => value,
            });
        }
        let result = match target {
            CallTarget::Callee(callee) => b.call(callee, &call_args),
            CallTarget::Site(body) => Some(b.site(body, &call_args)),
        };
        let value = match result {
            Some(named) => {
                let word = b.expect_word(named);
                b.set_name(named, call_name);
                if retain_result {
                    let retain = b.refcount_site(RefcountOperation::Retain);
                    b.refcount(retain, word);
                }
                word
            }
            None => b.constant(NONE_WORD),
        };
        self.finish(b, custody, value, &format!("{label}_result"), |_| {})
    }

    /// Commit one reference per adopted position while keeping one boxed
    /// identity per value. Repeated transfers need more references. If the
    /// consumer also borrows the value, custody keeps its original owner
    /// until the call returns; otherwise the first transfer takes that owner.
    /// Call only after every fallible input preparation, just before entry.
    pub(crate) fn surrender(
        &self,
        b: &mut B,
        custody: &Custody<B>,
        operands: &[ValueId],
        borrowed: &[ValueId],
    ) {
        let mut transfers = BTreeMap::<ValueId, usize>::new();
        for &operand in operands {
            if custody.owners.contains_key(&operand) {
                *transfers.entry(operand).or_default() += 1;
            }
        }
        for (operand, count) in transfers {
            let slot = custody.owners[&operand];
            let word = custody.words[&operand];
            let keep_owner = borrowed.contains(&operand);
            let retains = count - usize::from(!keep_owner);
            if retains > 0 {
                let retain = b.refcount_site(RefcountOperation::Retain);
                for _ in 0..retains {
                    b.refcount(retain, word);
                }
            }
            if !keep_owner {
                let none = b.constant(NONE_WORD);
                b.store_slot(slot, none);
            }
        }
    }
}

/// Release every initialized owner of `custody` here, in descending
/// value-id order, on the rejoined path.
fn release_owners<B: CodegenBuilder>(b: &mut B, custody: &Custody<B>) {
    if custody.owners.is_empty() {
        return;
    }
    let release = b.refcount_site(RefcountOperation::Release);
    for &slot in custody.owners.values().rev() {
        let word = b.load_slot(slot);
        let named = b.word_value(word);
        b.set_name(named, &format!("{}_owner_bits", custody.label));
        b.refcount(release, word);
    }
}

/// Spill call argument words into this operation's static entry array:
/// `(args_ptr, nargs)` words for an owned runtime entry, which copies them
/// out before any Python code can run.
pub(crate) fn spill_words<B: CodegenBuilder>(
    b: &mut B,
    words: &[B::Word],
    name: &str,
) -> (B::Word, B::Word) {
    let array = b.entry_slot_array(words.len().max(1) as u32, name);
    for (index, &word) in words.iter().enumerate() {
        b.store_slot_at(array, index as u32, word);
    }
    let pointer = b.slot_pointer(array);
    let address = b.address_to_word(pointer);
    let named = b.word_value(address);
    b.set_name(named, &format!("{name}_ptr"));
    let count = b.constant(words.len() as u64);
    (address, count)
}

/// Release a call's adopted inputs on a path that will not enter the
/// callee: the callable (or 0) and each adopted object word.
pub(crate) fn release_call_inputs<B: CodegenBuilder>(
    b: &mut B,
    callable: Option<B::Word>,
    owners: &[B::Word],
) {
    if owners.is_empty() && callable.is_none() {
        return;
    }
    let (args, count) = spill_words(b, owners, "call_retired");
    let callable = callable.unwrap_or_else(|| b.constant(0));
    b.call_runtime(RuntimeFn::CallInputsRelease, &[callable, args, count]);
}

#[cfg(test)]
mod tests {
    //! Custody on the interpreting builder. The oracle is the ownership
    //! contract, written out by hand: a value that cannot mint opens no
    //! owner slot and no block; a minted heap integer is boxed once per
    //! operation, joins the failure block only on its heap arm, and is
    //! released exactly once, after the consumer; an adopted input is
    //! released only on the failure path.

    use super::super::scalar::tests::{Edge, HEAP_INT, Interpreter, ModelCallee, Value};
    use super::*;
    use molt_ir::repr::Repr;

    /// An inline int's word: a quiet NaN, tag 1 at bit 48, the payload.
    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;
    const FULL_RANGE: i64 = 1 << 50;

    /// %0: a boxed object; %1: a raw int proven inline-safe; %2: a raw int
    /// without that proof.
    struct Fixture {
        values: ValueTable<Value>,
        plan: ProducerCarrierPlan,
        diagnostics: RefCell<Vec<String>>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut values = ValueTable::with_capacity(3);
            values.set_value(ValueId(0), Value::Word(OBJECT));
            values.set_repr(ValueId(0), HeldCarrier::Boxed);
            values.set_value(ValueId(1), Value::Word(7));
            values.set_repr(ValueId(1), HeldCarrier::RawI64);
            values.set_value(ValueId(2), Value::Word(FULL_RANGE as u64));
            values.set_repr(ValueId(2), HeldCarrier::RawI64);
            let mut plan = ProducerCarrierPlan::default();
            plan.repr_by_value.insert(ValueId(1), Repr::RawI64Safe);
            plan.repr_by_value.insert(ValueId(2), Repr::RawI64FullDeopt);
            Self {
                values,
                plan,
                diagnostics: RefCell::new(Vec::new()),
            }
        }

        fn operands(&self) -> Operands<'_, Interpreter> {
            Operands {
                function: "f",
                values: &self.values,
                plan: &self.plan,
                diagnostics: &self.diagnostics,
            }
        }
    }

    #[test]
    fn an_operand_that_cannot_mint_opens_no_slot_and_no_block() {
        let fixture = Fixture::new();
        let operands = fixture.operands();
        let mut b = Interpreter::default();
        let ids = [ValueId(0), ValueId(1)];
        let mut custody = operands.begin(&mut b, &ids, "t");
        assert_eq!(operands.operand(&mut b, &mut custody, ValueId(0)), OBJECT);
        assert_eq!(
            operands.operand(&mut b, &mut custody, ValueId(1)),
            inline(7)
        );
        assert!(!custody.can_fail());
        assert_eq!(
            operands.finish(&mut b, custody, OBJECT, "r", |_| {}),
            OBJECT
        );
        assert!(b.slots.is_empty(), "no owner slot");
        assert!(b.edges.is_empty(), "the control flow is unchanged");
        assert!(b.runtime_calls.is_empty());
        assert!(b.refcounts.is_empty());
        assert!(fixture.diagnostics.borrow().is_empty());
    }

    #[test]
    fn a_full_range_integer_mints_once_and_releases_its_owner_once() {
        let fixture = Fixture::new();
        let operands = fixture.operands();
        let mut b = Interpreter::default();
        // %2 is declared twice and requested twice: one identity.
        let mut custody = operands.begin(&mut b, &[ValueId(2), ValueId(2)], "t");
        assert_eq!(b.slots, [NONE_WORD], "one owner slot, reset to None");
        let first = operands.operand(&mut b, &mut custody, ValueId(2));
        let second = operands.operand(&mut b, &mut custody, ValueId(2));
        assert_eq!(first, second, "one operation transports one identity");
        assert_eq!(b.runtime_calls, [RuntimeFn::IntFromI64], "one mint");
        assert_eq!(b.slots, [HEAP_INT], "the heap arm records the owner");
        assert!(custody.can_fail(), "a failed mint joins the failure block");
        assert!(b.refcounts.is_empty(), "nothing is released before finish");
        operands.finish(&mut b, custody, OBJECT, "r", |_| {});
        assert_eq!(
            b.refcounts,
            [(RefcountOperation::Release, HEAP_INT)],
            "the owner is released once, after the consumer"
        );
        // inline, heap, boxed, failure, merge.
        let (boxed, failure, merge) = (2, 3, 4);
        assert_eq!(
            b.edges,
            [
                Edge::Branch(false, (0, vec![]), (1, vec![]),),
                Edge::Jump(boxed, vec![Value::Word(inline(FULL_RANGE))]),
                Edge::Branch(
                    false,
                    (failure, vec![]),
                    (boxed, vec![Value::Word(HEAP_INT)]),
                ),
                Edge::Jump(merge, vec![Value::Word(OBJECT)]),
                Edge::Jump(merge, vec![Value::Word(NONE_WORD)]),
            ]
        );
    }

    #[test]
    fn a_surrendered_owner_is_retained_per_extra_transfer_and_not_released() {
        let fixture = Fixture::new();
        let operands = fixture.operands();
        let mut b = Interpreter::default();
        let mut custody = operands.begin(&mut b, &[ValueId(2)], "t");
        // The word is the boxed block's parameter, which the interpreter
        // does not follow; the retain must take that word.
        let word = operands.operand(&mut b, &mut custody, ValueId(2));
        // Two adopted positions, no borrow: the first takes the owner, the
        // second needs one more reference.
        operands.surrender(&mut b, &custody, &[ValueId(2), ValueId(2)], &[]);
        assert_eq!(b.refcounts, [(RefcountOperation::Retain, word)]);
        assert_eq!(b.slots, [NONE_WORD], "the slot no longer owns it");
        operands.finish(&mut b, custody, OBJECT, "r", |_| {});
        assert_eq!(
            b.refcounts[1..],
            [(RefcountOperation::Release, NONE_WORD)],
            "finish releases the emptied slot: None, a no-op"
        );
    }

    #[test]
    fn a_failure_releases_the_calls_adopted_inputs_on_the_failure_path_only() {
        let fixture = Fixture::new();
        let operands = fixture.operands();
        let mut b = Interpreter::default();
        let mut custody =
            operands.begin_call(&mut b, &[ValueId(2)], &[ValueId(0), ValueId(1)], None, "t");
        operands.operand(&mut b, &mut custody, ValueId(2));
        let mut aborted = false;
        operands.finish(&mut b, custody, OBJECT, "r", |_| aborted = true);
        assert!(aborted, "the consumer's cleanup runs on the failure path");
        // Only the boxed object is adopted; the raw int has no owner yet.
        assert_eq!(
            b.runtime_calls,
            [RuntimeFn::IntFromI64, RuntimeFn::CallInputsRelease]
        );
        assert_eq!(
            b.slots,
            [HEAP_INT, OBJECT],
            "the owner slot, then the spilled input"
        );
    }

    #[test]
    fn a_runtime_call_borrows_its_operands_and_retains_a_kept_borrowed_result() {
        let fixture = Fixture::new();
        let operands = fixture.operands();
        let mut b = Interpreter::default();
        let result = operands.call(
            &mut b,
            CallTarget::Callee(ModelCallee::Runtime(RuntimeFn::IsTruthy)),
            &[RuntimeArg::Operand(ValueId(1))],
            true,
            "t",
            "c",
        );
        assert_eq!(result, 1, "the callee received the boxed operand");
        assert_eq!(b.refcounts, [(RefcountOperation::Retain, 1)]);
        assert!(b.edges.is_empty());
    }

    #[test]
    fn a_value_used_before_its_definition_is_a_fatal_diagnostic() {
        let fixture = Fixture::new();
        let operands = fixture.operands();
        let mut b = Interpreter::default();
        operands.boxed_word(&mut b, ValueId(3));
        assert_eq!(
            *fixture.diagnostics.borrow(),
            ["f: ValueId %3 was used before being defined during lowering"]
        );
    }

    // Custody as executed code. The interpreting builder runs every block as
    // it builds it, so the tests above check custody's structure on it; the
    // tests below run custody on the machines and check what each path does.

    use crate::codegen_ssa::differential::{both, kernel, machine};

    /// The fixture's values from the inputs, in its carriers.
    fn table<V: Copy>(inputs: &[V]) -> ValueTable<V> {
        let mut values = ValueTable::with_capacity(3);
        for (index, carrier) in [HeldCarrier::Boxed, HeldCarrier::RawI64, HeldCarrier::RawI64]
            .into_iter()
            .enumerate()
        {
            values.set_value(ValueId(index as u32), inputs[index]);
            values.set_repr(ValueId(index as u32), carrier);
        }
        values
    }

    fn inputs(full_range: i64) -> [Value; 3] {
        [
            Value::Word(OBJECT),
            Value::Word(7),
            Value::Word(full_range as u64),
        ]
    }

    /// One custody over `operands` (borrowed) and `adopted`, each requested
    /// in order and committed with the last word requested; returns the
    /// rejoined result. An operand word does not dominate the rejoin (the
    /// failure path skips it), so only the result is read after it.
    fn custody_kernel(
        operands: &'static [ValueId],
        adopted: &'static [ValueId],
    ) -> impl crate::codegen_ssa::differential::Kernel<Fact = ()> {
        kernel!(
            plan: ProducerCarrierPlan = Fixture::new().plan,
            operands: &'static [ValueId] = operands,
            adopted: &'static [ValueId] = adopted;
            |b, inputs| -> () {
                let values = table(inputs);
                let diagnostics = RefCell::new(Vec::new());
                let view = Operands::<B> {
                    function: "f",
                    values: &values,
                    plan,
                    diagnostics: &diagnostics,
                };
                let mut custody = view.begin_call(b, operands, adopted, None, "t");
                let mut committed = None;
                for &operand in operands.iter() {
                    committed = Some(view.operand(b, &mut custody, operand));
                }
                let committed = committed.expect("one operand at least");
                let result = view.finish(b, custody, committed, "r", |_| {});
                assert!(diagnostics.borrow().is_empty());
                ((), vec![b.word_value(result)])
            }
        )
    }

    #[test]
    fn custody_that_cannot_fail_runs_alike_on_both_machines() {
        let run = both(
            &custody_kernel(&[ValueId(0), ValueId(1)], &[]),
            &inputs(FULL_RANGE),
        );
        assert_eq!(run.results, [Value::Word(inline(7))]);
        assert!(run.runtime_calls.is_empty());
        assert!(run.refcounts.is_empty());
    }

    #[test]
    fn a_minted_integer_is_released_once_on_the_committed_path() {
        let operands = &[ValueId(2), ValueId(2)];
        let run = machine(&custody_kernel(operands, &[]), &inputs(FULL_RANGE), false);
        assert_eq!(run.results, [Value::Word(HEAP_INT)]);
        assert_eq!(
            run.runtime_calls,
            [RuntimeFn::IntFromI64],
            "one mint: the second request reuses the first word"
        );
        assert_eq!(run.refcounts, [(RefcountOperation::Release, HEAP_INT)]);
        // A value that fits boxes inline: no mint, nothing to release.
        let run = machine(&custody_kernel(operands, &[]), &inputs(-3), false);
        assert_eq!(run.results, [Value::Word(inline(-3))]);
        assert!(run.runtime_calls.is_empty());
        assert!(run.refcounts.is_empty());
    }

    #[test]
    fn a_failed_mint_yields_none_and_releases_the_adopted_inputs() {
        let kernel = custody_kernel(&[ValueId(2)], &[ValueId(0), ValueId(1)]);
        let run = machine(&kernel, &inputs(FULL_RANGE), true);
        assert_eq!(run.results[0], Value::Word(NONE_WORD), "the failure result");
        assert_eq!(
            run.runtime_calls,
            [RuntimeFn::IntFromI64, RuntimeFn::CallInputsRelease]
        );
        assert_eq!(
            run.released,
            [vec![OBJECT]],
            "only the boxed object was adopted"
        );
        assert!(run.refcounts.is_empty(), "the empty owner slot is None");
        // The committed path releases nothing it adopted.
        let run = machine(&kernel, &inputs(FULL_RANGE), false);
        assert_eq!(run.results[0], Value::Word(HEAP_INT));
        assert_eq!(run.runtime_calls, [RuntimeFn::IntFromI64]);
        assert!(run.released.is_empty());
        assert_eq!(run.refcounts, [(RefcountOperation::Release, HEAP_INT)]);
    }

    #[test]
    fn a_surrendered_owner_is_retained_per_extra_transfer_on_the_machine() {
        let surrender = kernel!(plan: ProducerCarrierPlan = Fixture::new().plan; |b, inputs| -> () {
            let values = table(inputs);
            let diagnostics = RefCell::new(Vec::new());
            let view = Operands::<B> {
                function: "f",
                values: &values,
                plan,
                diagnostics: &diagnostics,
            };
            let mut custody = view.begin(b, &[ValueId(2)], "t");
            view.operand(b, &mut custody, ValueId(2));
            view.surrender(b, &custody, &[ValueId(2), ValueId(2)], &[]);
            let object = b.constant(OBJECT);
            let result = view.finish(b, custody, object, "r", |_| {});
            ((), vec![b.word_value(result)])
        });
        let run = machine(&surrender, &inputs(FULL_RANGE), false);
        assert_eq!(run.results, [Value::Word(OBJECT)]);
        assert_eq!(
            run.refcounts,
            [(RefcountOperation::Retain, HEAP_INT)],
            "one extra reference; the owner moved to the callee and is not released"
        );
    }

    #[test]
    fn a_runtime_call_retains_its_kept_result_on_both_machines() {
        let call = kernel!(plan: ProducerCarrierPlan = Fixture::new().plan; |b, inputs| -> () {
            let values = table(inputs);
            let diagnostics = RefCell::new(Vec::new());
            let view = Operands::<B> {
                function: "f",
                values: &values,
                plan,
                diagnostics: &diagnostics,
            };
            let callee = b.runtime_callee(RuntimeFn::IntFromI64);
            let callee = CallTarget::Callee(callee);
            let result = view.call(b, callee, &[RuntimeArg::Operand(ValueId(1))], true, "t", "c");
            ((), vec![b.word_value(result)])
        });
        let run = both(&call, &inputs(FULL_RANGE));
        assert_eq!(run.results, [Value::Word(HEAP_INT)]);
        assert_eq!(run.runtime_calls, [RuntimeFn::IntFromI64]);
        assert_eq!(run.refcounts, [(RefcountOperation::Retain, HEAP_INT)]);
    }
}
