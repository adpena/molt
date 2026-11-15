//! The generators run on an interpreting builder over byte-addressed memory
//! holding real object headers. The oracle is the runtime's field rule
//! written directly (molt-runtime `object_field_load_ptr_raw` and
//! `object_field_set_ptr_raw`): a dictionary-backed, empty or non-object
//! receiver is the runtime's; otherwise the inline word is the field.

use std::collections::HashMap;
use std::vec::Vec;

use super::*;
use crate::frame_record::FrameBuilder;
use crate::word::{WordBuilder, WordComparison};
use crate::{HEADER_AUX_KIND_SIDECAR, TAG_INT, TAG_PTR, TYPE_ID_LIST, TYPE_ID_STRING};

const OBJECT: u64 = 0x1_0000;
const VALUE: u64 = 0x2_0000;
const CLASS: u64 = 0x3_0000;
const OFFSET: i32 = 16;
const COLD: u64 = 0xC01D;

fn boxed(address: u64) -> u64 {
    QNAN | TAG_PTR | address
}

fn int(value: u64) -> u64 {
    QNAN | TAG_INT | value
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Retain(u64),
    Release(u64),
    Flag(u32),
    Cold(u64),
}

#[derive(Default)]
struct Machine {
    bytes: HashMap<u64, u8>,
    locals: Vec<u64>,
    events: Vec<Event>,
    bailed: bool,
}

impl Machine {
    /// An object of `type_id` at `address` with `flags`, an inline class word
    /// `class` and `field` at `OFFSET`.
    fn object(&mut self, address: u64, type_id: u32, flags: u32, class: u64, field: u64) {
        self.write(address, HEADER_TYPE_ID_OFFSET, type_id.into(), 4);
        self.write(address, HEADER_FLAGS_OFFSET, flags.into(), 4);
        self.write(
            address,
            HEADER_AUX_KIND_OFFSET - 2,
            u64::from(HEADER_AUX_KIND_CLASS_INLINE) << 16,
            4,
        );
        self.write(address, HEADER_AUX_OFFSET, class, 8);
        self.write(address, OFFSET, field, 8);
    }

    /// A receiver of the proven class holding `field`.
    fn receiver(flags: u32, field: u64) -> Self {
        let mut m = Self::default();
        m.object(OBJECT, TYPE_ID_OBJECT, flags, boxed(CLASS), field);
        m
    }

    fn write(&mut self, address: u64, offset: i32, value: u64, width: usize) {
        let at = address.wrapping_add_signed(offset.into());
        for byte in 0..width {
            self.bytes
                .insert(at + byte as u64, (value >> (8 * byte)) as u8);
        }
    }

    fn read(&self, address: u64, offset: i32, width: usize) -> u64 {
        let at = address.wrapping_add_signed(offset.into());
        (0..width).fold(0, |value, byte| {
            let bits = *self
                .bytes
                .get(&(at + byte as u64))
                .expect("generated code read memory nothing wrote");
            value | (u64::from(bits) << (8 * byte))
        })
    }

    fn field(&self) -> u64 {
        self.read(OBJECT, OFFSET, 8)
    }

    fn flags(&self) -> u32 {
        self.read(OBJECT, HEADER_FLAGS_OFFSET, 4) as u32
    }

    fn get_on(&mut self, object: u64, locking: ListLocking) -> u64 {
        generate_field_slot_get(self, object, OFFSET, locking, |m, pointer| {
            m.events.push(Event::Cold(pointer));
            COLD
        })
    }

    fn get(&mut self) -> u64 {
        self.get_on(boxed(OBJECT), ListLocking::Gil)
    }

    fn set_on(&mut self, object: u64, value: u64, locking: ListLocking) {
        generate_field_slot_set(self, object, OFFSET, value, locking, |m, pointer| {
            m.events.push(Event::Cold(pointer));
        });
    }

    fn set(&mut self, value: u64) {
        self.set_on(boxed(OBJECT), value, ListLocking::Gil);
    }
}

impl WordBuilder for Machine {
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

impl crate::thread_record::RecordWordBuilder for Machine {
    fn load_word(&mut self, address: u64, offset: i32) -> u64 {
        if self.bailed {
            return 0;
        }
        self.read(address, offset, 8)
    }
    fn store_word(&mut self, address: u64, offset: i32, value: u64) {
        if !self.bailed {
            self.write(address, offset, value, 8);
        }
    }
    fn load_shared_word(&mut self, address: u64, offset: i32) -> u64 {
        self.load_word(address, offset)
    }
}

impl FrameBuilder for Machine {
    type Local = usize;

    fn local(&mut self) -> usize {
        self.locals.push(0);
        self.locals.len() - 1
    }
    fn set(&mut self, local: usize, value: u64) {
        if !self.bailed {
            self.locals[local] = value;
        }
    }
    fn get(&mut self, local: usize) -> u64 {
        self.locals[local]
    }
    fn load_header_u32(&mut self, pointer: u64, offset: i32) -> u64 {
        if self.bailed {
            return 0;
        }
        self.read(pointer, offset, 4)
    }
    fn add(&mut self, left: u64, right: u64) -> u64 {
        left.wrapping_add(right)
    }
    fn retain(&mut self, bits: u64) {
        if !self.bailed {
            self.events.push(Event::Retain(bits));
        }
    }
    fn release(&mut self, bits: u64) {
        if !self.bailed {
            self.events.push(Event::Release(bits));
        }
    }
    fn enter_slow(&mut self, _code_slot: u64) -> u64 {
        unreachable!("a field site pushes no frame")
    }
    fn exit_slow(&mut self, _base: u64) {
        unreachable!("a field site pops no frame")
    }
    fn attempt(&mut self, fast: impl FnOnce(&mut Self), slow: impl FnOnce(&mut Self)) {
        assert!(!self.bailed, "an attempt nested in a failed fast path");
        fast(self);
        if std::mem::take(&mut self.bailed) {
            slow(self);
        }
    }
    fn require(&mut self, condition: bool) {
        if !condition {
            self.bailed = true;
        }
    }
    fn if_then(&mut self, condition: bool, then: impl FnOnce(&mut Self)) {
        if condition && !self.bailed {
            then(self);
        }
    }
    fn counted_loop(&mut self, _count: u64, _descending: bool, _body: impl FnMut(&mut Self, u64)) {
        unreachable!("a field site has no loop")
    }
}

impl FieldSlotBuilder for Machine {
    fn or_header_flag(&mut self, pointer: u64, flag: u32) {
        if self.bailed {
            return;
        }
        let flags = self.read(pointer, HEADER_FLAGS_OFFSET, 4) as u32;
        self.write(pointer, HEADER_FLAGS_OFFSET, u64::from(flags | flag), 4);
        self.events.push(Event::Flag(flag));
    }
}

#[test]
fn an_inline_field_reads_as_an_owned_word() {
    let mut m = Machine::receiver(0, int(7));
    assert_eq!(m.get(), int(7));
    assert_eq!(m.events, [Event::Retain(int(7))]);

    // A string, and an instance of a class, are field values.
    for (type_id, class) in [(TYPE_ID_STRING, 0), (TYPE_ID_OBJECT, boxed(CLASS))] {
        let mut m = Machine::receiver(HEADER_FLAG_HAS_PTRS, boxed(VALUE));
        m.object(VALUE, type_id, 0, class, 0);
        assert_eq!(m.get(), boxed(VALUE));
        assert_eq!(m.events, [Event::Retain(boxed(VALUE))]);
    }
}

/// The runtime owns an empty field (class fallback or AttributeError), a
/// dictionary-backed one, and a receiver whose field base may move.
#[test]
fn every_word_the_inline_slot_does_not_own_takes_the_runtime() {
    type Refusal = (&'static str, fn(&mut Machine));
    let refusals: [Refusal; 4] = [
        ("instance dictionary", |m| {
            m.write(
                OBJECT,
                HEADER_FLAGS_OFFSET,
                HEADER_FLAG_HAS_INSTANCE_DICT.into(),
                4,
            )
        }),
        ("builtin-payload subclass", |m| {
            m.write(OBJECT, HEADER_TYPE_ID_OFFSET, TYPE_ID_LIST.into(), 4)
        }),
        ("missing singleton", |m| {
            m.write(OBJECT, OFFSET, boxed(VALUE), 8);
            m.object(VALUE, TYPE_ID_OBJECT, crate::HEADER_FLAG_IMMORTAL, 0, 0);
        }),
        ("classless object behind a sidecar", |m| {
            m.write(OBJECT, OFFSET, boxed(VALUE), 8);
            m.object(VALUE, TYPE_ID_OBJECT, 0, 0x4_0000, 0);
            m.write(
                VALUE,
                HEADER_AUX_KIND_OFFSET,
                HEADER_AUX_KIND_SIDECAR.into(),
                2,
            );
        }),
    ];
    for (name, refuse) in refusals {
        let mut m = Machine::receiver(0, int(3));
        refuse(&mut m);
        assert_eq!(m.get(), COLD, "{name}");
        assert_eq!(m.events, [Event::Cold(OBJECT)], "{name}");
    }
    for (name, refuse) in &refusals[..2] {
        let mut m = Machine::receiver(0, int(3));
        refuse(&mut m);
        m.set(int(4));
        assert_eq!(m.field(), int(3), "{name}");
        assert_eq!(m.events, [Event::Cold(OBJECT)], "{name}");
    }
}

/// A failed allocation leaves a non-pointer sentinel with its exception
/// pending: the access must neither touch memory nor raise again.
#[test]
fn a_non_pointer_receiver_reads_none_and_stores_nothing() {
    let mut m = Machine::default();
    assert_eq!(m.get_on(int(0), ListLocking::Gil), QNAN | TAG_NONE);
    m.set_on(int(0), int(1), ListLocking::Gil);
    assert!(m.events.is_empty());
}

#[test]
fn a_store_retains_the_new_word_before_it_releases_the_old_one() {
    let mut m = Machine::receiver(0, int(1));
    m.set(boxed(VALUE));
    assert_eq!(m.field(), boxed(VALUE));
    assert_eq!(m.flags(), HEADER_FLAG_HAS_PTRS);
    // Before the first pointer the old word owns nothing: no release.
    assert_eq!(
        m.events,
        [
            Event::Flag(HEADER_FLAG_HAS_PTRS),
            Event::Retain(boxed(VALUE))
        ]
    );
    // HAS_PTRS is already set: no second write of the header.
    m.events.clear();
    m.set(int(2));
    m.set(boxed(VALUE));
    assert_eq!(
        m.events,
        [
            Event::Retain(int(2)),
            Event::Release(boxed(VALUE)),
            Event::Retain(boxed(VALUE)),
            Event::Release(int(2)),
        ]
    );
}

#[test]
fn an_immediate_store_leaves_the_header_alone() {
    let mut m = Machine::receiver(0, int(1));
    m.set(int(9));
    assert_eq!((m.field(), m.flags()), (int(9), 0));
    assert_eq!(m.events, [Event::Retain(int(9))]);
    // Once the object holds pointers, the old word is released.
    let mut m = Machine::receiver(HEADER_FLAG_HAS_PTRS, int(1));
    m.set(int(9));
    assert_eq!(m.events, [Event::Retain(int(9)), Event::Release(int(1))]);
}

#[test]
fn a_free_threaded_build_takes_the_runtime() {
    let mut m = Machine::receiver(0, int(1));
    assert_eq!(m.get_on(boxed(OBJECT), ListLocking::FreeThreaded), COLD);
    m.set_on(boxed(OBJECT), int(2), ListLocking::FreeThreaded);
    assert_eq!(m.field(), int(1));
    assert_eq!(m.events, [Event::Cold(OBJECT), Event::Cold(OBJECT)]);
}
