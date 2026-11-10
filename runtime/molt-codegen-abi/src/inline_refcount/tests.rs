use super::*;
use crate::word::{WordBuilder, WordComparison};
use crate::{
    PointerWidth, REFCOUNT_OWNER_LIMIT, REFCOUNT_RUNTIME_CUSTODY, refcount_word_owners,
    release_transition, retain_next,
};

const ADDRESS: u64 = 0x1000;
const OBJECT: u64 = crate::QNAN | crate::TAG_PTR | ADDRESS;

struct Model {
    count: u32,
    reads: usize,
    writes: usize,
    slow: Option<RefcountOperation>,
    width: PointerWidth,
    expected_bits: u64,
}

impl Model {
    fn new(count: u32) -> Self {
        Self {
            count,
            reads: 0,
            writes: 0,
            slow: None,
            width: PointerWidth::Bits64,
            expected_bits: OBJECT,
        }
    }
}

impl WordBuilder for Model {
    type Word = u64;
    type Predicate = bool;
    fn constant(&mut self, value: u64) -> u64 {
        value
    }
    fn and(&mut self, value: u64, mask: u64) -> u64 {
        value & mask
    }
    fn or(&mut self, value: u64, bits: u64) -> u64 {
        value | bits
    }
    fn shift_left(&mut self, value: u64, amount: u32) -> u64 {
        value << amount
    }
    fn shift_right_arithmetic(&mut self, value: u64, amount: u32) -> u64 {
        ((value as i64) >> amount) as u64
    }
    fn add_immediate(&mut self, value: u64, delta: i64) -> u64 {
        value.wrapping_add_signed(delta)
    }
    fn compare(&mut self, comparison: WordComparison, left: u64, right: u64) -> bool {
        match comparison {
            WordComparison::Equal => left == right,
            WordComparison::NotEqual => left != right,
            WordComparison::UnsignedLessThan => left < right,
        }
    }
    fn both(&mut self, left: bool, right: bool) -> bool {
        left && right
    }
}

impl RefcountBuilder for Model {
    fn pointer_width(&self) -> PointerWidth {
        self.width
    }
    fn load_header_u32(&mut self, pointer: u64, offset: i32) -> u64 {
        assert_eq!(
            pointer, ADDRESS,
            "a non-object must never reach a header load"
        );
        assert!(
            self.slow.is_none(),
            "the runtime owns the remainder of a slow transition"
        );
        self.reads += 1;
        assert_eq!(
            offset, HEADER_REFCOUNT_OFFSET,
            "eligibility lives in the count word; no other header field is read"
        );
        assert_eq!(self.reads, 1, "one site reads one header word");
        self.count as u64
    }
    fn store_header_u32(&mut self, pointer: u64, offset: i32, value: u64) {
        assert_eq!((pointer, offset), (ADDRESS, HEADER_REFCOUNT_OFFSET));
        assert!(self.slow.is_none());
        self.writes += 1;
        assert_eq!(
            self.writes, 1,
            "one RC op has exactly one physical transition"
        );
        self.count = u32::try_from(value).expect("checked refcount does not truncate");
    }
    fn slow_call(&mut self, operation: RefcountOperation, bits: u64) {
        assert_eq!(
            bits, self.expected_bits,
            "the slow path receives the original boxed owner"
        );
        assert_eq!(
            self.writes, 0,
            "the runtime must not apply a transition twice"
        );
        assert!(self.slow.replace(operation).is_none());
    }
    fn if_else(
        &mut self,
        condition: bool,
        yes: impl FnOnce(&mut Self),
        no: impl FnOnce(&mut Self),
    ) {
        if condition {
            yes(self);
        } else {
            no(self);
        }
    }
}

/// Words that cover the window edges, the overflow edge, the custody bit on
/// every interesting owner count, and the immortal sentinel.
fn interesting_words() -> [u32; 23] {
    let window = COMPILED_PLAIN_REFCOUNT_WINDOW as u32;
    let owners = [
        0,
        1,
        2,
        3,
        19,
        window - 1,
        window,
        window + 1,
        window + 2,
        REFCOUNT_OWNER_LIMIT - 2,
        REFCOUNT_OWNER_LIMIT - 1,
    ];
    let mut words = [IMMORTAL_REFCOUNT; 23];
    for (index, owners) in owners.into_iter().enumerate() {
        words[2 * index] = owners;
        words[2 * index + 1] = owners | REFCOUNT_RUNTIME_CUSTODY;
    }
    words
}

#[test]
fn generated_transitions_refine_the_checked_runtime_algebra() {
    for word in interesting_words() {
        for operation in [RefcountOperation::Retain, RefcountOperation::Release] {
            let mut model = Model::new(word);
            generate_refcount(
                &mut model,
                OBJECT,
                CompiledRefcountAdmission::AttachedPlain as u64,
                operation,
                RefcountStorage::Plain,
            );
            assert_eq!(model.reads, 1, "{operation:?}, {word:#x}");
            if word == IMMORTAL_REFCOUNT {
                // The runtime's immortal transition is a no-op; deciding it
                // on the cold side must not change the word.
                assert_eq!((model.count, model.writes, model.slow), (word, 0, None));
                continue;
            }
            if model.writes == 1 {
                // Every plain write is exactly the runtime's next state for a
                // mortal object without custody: refinement, not a copy.
                assert_eq!(word & REFCOUNT_RUNTIME_CUSTODY, 0, "{word:#x}");
                let owners = refcount_word_owners(word);
                let expected = match operation {
                    RefcountOperation::Retain => retain_next(owners, 1, false).ok(),
                    RefcountOperation::Release => release_transition(owners)
                        .filter(|edge| !edge.reached_zero())
                        .map(|edge| edge.next()),
                };
                assert_eq!(Some(model.count), expected, "{operation:?}, {word:#x}");
                assert_eq!(model.slow, None);
            } else {
                assert_eq!(
                    (model.count, model.writes, model.slow),
                    (word, 0, Some(operation)),
                    "{operation:?}, {word:#x}"
                );
            }
        }
    }
}

#[test]
fn runtime_custody_routes_every_owner_count_to_the_runtime() {
    for owners in [0, 1, 2, 3, 19, 1 << 20, REFCOUNT_OWNER_LIMIT - 1] {
        for operation in [RefcountOperation::Retain, RefcountOperation::Release] {
            let word = owners | REFCOUNT_RUNTIME_CUSTODY;
            let mut model = Model::new(word);
            generate_refcount(
                &mut model,
                OBJECT,
                CompiledRefcountAdmission::AttachedPlain as u64,
                operation,
                RefcountStorage::Plain,
            );
            assert_eq!(
                (model.count, model.writes, model.slow),
                (word, 0, Some(operation)),
                "{operation:?}, {word:#x}"
            );
        }
    }
}

#[test]
fn plain_window_is_exactly_the_ordinary_mortal_interval() {
    let window = COMPILED_PLAIN_REFCOUNT_WINDOW as u32;
    for (operation, first, last) in [
        (RefcountOperation::Retain, 1, window),
        (RefcountOperation::Release, 2, window + 1),
    ] {
        for word in [first - 1, first, last, last + 1] {
            let mut model = Model::new(word);
            generate_refcount(
                &mut model,
                OBJECT,
                CompiledRefcountAdmission::AttachedPlain as u64,
                operation,
                RefcountStorage::Plain,
            );
            let plain = (first..=last).contains(&word);
            assert_eq!(model.writes, usize::from(plain), "{operation:?}, {word}");
            assert_eq!(model.slow.is_some(), !plain, "{operation:?}, {word}");
        }
    }
}

#[test]
fn immediates_and_null_words_never_read_headers_or_enter_runtime() {
    for bits in [
        0,
        1_f64.to_bits(),
        (-2_f64).to_bits(),
        crate::CANONICAL_NAN_BITS,
        crate::QNAN | crate::TAG_INT | 19,
        crate::QNAN | crate::TAG_BOOL | 1,
        crate::QNAN | crate::TAG_NONE,
        crate::QNAN | crate::TAG_PENDING,
        crate::QNAN | crate::TAG_PTR,
    ] {
        for operation in [RefcountOperation::Retain, RefcountOperation::Release] {
            for storage in [
                RefcountStorage::Plain,
                RefcountStorage::Atomic,
                RefcountStorage::HardenedPlain,
            ] {
                let mut model = Model::new(1);
                generate_refcount(
                    &mut model,
                    bits,
                    CompiledRefcountAdmission::Runtime as u64,
                    operation,
                    storage,
                );
                assert_eq!(
                    (model.reads, model.writes, model.slow),
                    (0, 0, None),
                    "{bits:#x}"
                );
            }
        }
    }
}

#[test]
fn unattached_diagnostic_and_atomic_paths_cannot_touch_header_cells() {
    for (admission, storage) in [
        (CompiledRefcountAdmission::Runtime, RefcountStorage::Plain),
        (
            CompiledRefcountAdmission::AttachedPlain,
            RefcountStorage::Atomic,
        ),
        (
            CompiledRefcountAdmission::AttachedPlain,
            RefcountStorage::HardenedPlain,
        ),
    ] {
        for operation in [RefcountOperation::Retain, RefcountOperation::Release] {
            let mut model = Model::new(2);
            generate_refcount(&mut model, OBJECT, admission as u64, operation, storage);
            assert_eq!(
                (model.count, model.reads, model.writes, model.slow),
                (2, 0, 0, Some(operation))
            );
        }
    }
}

#[test]
fn wasm_rejects_wide_payloads_before_truncating_an_address() {
    let mut model = Model::new(2);
    model.width = PointerWidth::Bits32;
    model.expected_bits = OBJECT | (1_u64 << 32);
    let bits = model.expected_bits;
    generate_refcount(
        &mut model,
        bits,
        CompiledRefcountAdmission::AttachedPlain as u64,
        RefcountOperation::Retain,
        RefcountStorage::Plain,
    );
    assert_eq!(
        (model.reads, model.writes, model.slow),
        (0, 0, Some(RefcountOperation::Retain))
    );
}
