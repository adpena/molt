use super::*;
use crate::{PointerWidth, release_transition, retain_next};

const ADDRESS: u64 = 0x1000;
const OBJECT: u64 = crate::QNAN | crate::TAG_PTR | ADDRESS;

struct Model {
    type_id: u32,
    count: u32,
    flags: u32,
    reads: usize,
    writes: usize,
    slow: Option<RefcountOperation>,
    width: PointerWidth,
    expected_bits: u64,
}

impl Model {
    fn new(count: u32, flags: u32) -> Self {
        Self {
            type_id: crate::TYPE_ID_LIST,
            count,
            flags,
            reads: 0,
            writes: 0,
            slow: None,
            width: PointerWidth::Bits64,
            expected_bits: OBJECT,
        }
    }
}

impl RefcountBuilder for Model {
    type Word = u64;
    type Predicate = bool;
    fn pointer_width(&self) -> PointerWidth {
        self.width
    }
    fn constant(&mut self, value: u64) -> u64 {
        value
    }
    fn and(&mut self, value: u64, mask: u64) -> u64 {
        value & mask
    }
    fn add(&mut self, value: u64, delta: i64) -> u64 {
        value.wrapping_add_signed(delta)
    }
    fn shift_right(&mut self, value: u64, amount: u64) -> u64 {
        assert!(amount < 64, "an LLVM shift must never evaluate poison");
        value >> amount
    }
    fn compare(&mut self, comparison: RefcountComparison, left: u64, right: u64) -> bool {
        match comparison {
            RefcountComparison::Equal => left == right,
            RefcountComparison::NotEqual => left != right,
            RefcountComparison::UnsignedLessThan => left < right,
        }
    }
    fn either(&mut self, left: bool, right: bool) -> bool {
        left || right
    }
    fn both(&mut self, left: bool, right: bool) -> bool {
        left && right
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
        match offset {
            HEADER_TYPE_ID_OFFSET => self.type_id as u64,
            HEADER_FLAGS_OFFSET => self.flags as u64,
            HEADER_REFCOUNT_OFFSET => self.count as u64,
            _ => panic!("unknown header field"),
        }
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

#[test]
fn generated_transitions_refine_the_checked_runtime_algebra() {
    let counts = [
        0,
        1,
        2,
        3,
        19,
        IMMORTAL_REFCOUNT - 3,
        IMMORTAL_REFCOUNT - 2,
        IMMORTAL_REFCOUNT - 1,
        IMMORTAL_REFCOUNT,
    ];
    for count in counts {
        for flags in [
            0,
            HEADER_FLAG_IMMORTAL,
            HEADER_FLAG_DEALLOCATING,
            HEADER_FLAG_HAS_ABI_VIEW,
            HEADER_FLAG_IMMORTAL | HEADER_FLAG_HAS_ABI_VIEW,
            HEADER_FLAG_DEALLOCATING | HEADER_FLAG_HAS_ABI_VIEW,
        ] {
            for operation in [RefcountOperation::Retain, RefcountOperation::Release] {
                let mut model = Model::new(count, flags);
                generate_refcount(
                    &mut model,
                    OBJECT,
                    CompiledRefcountAdmission::AttachedPlain as u64,
                    operation,
                    RefcountStorage::Plain,
                );
                if flags & HEADER_FLAG_IMMORTAL != 0 {
                    assert_eq!((model.count, model.writes, model.slow), (count, 0, None));
                    continue;
                }
                let expected = match operation {
                    RefcountOperation::Retain => {
                        retain_next(count, 1, flags & HEADER_FLAG_DEALLOCATING != 0).ok()
                    }
                    RefcountOperation::Release => release_transition(count)
                        .filter(|edge| !edge.reached_zero())
                        .map(|edge| edge.next()),
                };
                if let Some(expected) = expected.filter(|_| flags & HEADER_FLAG_HAS_ABI_VIEW == 0) {
                    assert_eq!(
                        (model.count, model.writes, model.slow),
                        (expected, 1, None),
                        "{operation:?}, {count}, {flags:#x}"
                    );
                } else {
                    assert_eq!(
                        (model.count, model.writes, model.slow),
                        (count, 0, Some(operation)),
                        "{operation:?}, {count}, {flags:#x}"
                    );
                }
            }
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
            for storage in [RefcountStorage::Plain, RefcountStorage::Atomic] {
                let mut model = Model::new(1, 0);
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
    ] {
        for operation in [RefcountOperation::Retain, RefcountOperation::Release] {
            let mut model = Model::new(2, 0);
            generate_refcount(&mut model, OBJECT, admission as u64, operation, storage);
            assert_eq!(
                (model.count, model.reads, model.writes, model.slow),
                (2, 0, 0, Some(operation))
            );
        }
    }
}

#[test]
fn sparse_heap_membership_preserves_retired_type_rejection() {
    for type_id in 0..=crate::MAX_HEAP_TYPE_ID + 1 {
        let mut model = Model::new(2, 0);
        model.type_id = type_id;
        generate_refcount(
            &mut model,
            OBJECT,
            CompiledRefcountAdmission::AttachedPlain as u64,
            RefcountOperation::Retain,
            RefcountStorage::Plain,
        );
        let live = crate::ALL_HEAP_TYPE_IDS.contains(&type_id);
        assert_eq!(model.writes, usize::from(live), "type id {type_id}");
        assert_eq!(model.slow.is_some(), !live, "type id {type_id}");
    }
    for retired in [205, 220, 231] {
        assert!(!crate::is_valid_heap_type_id(retired));
    }
}

#[test]
fn wasm_rejects_wide_payloads_before_truncating_an_address() {
    let mut model = Model::new(2, 0);
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
