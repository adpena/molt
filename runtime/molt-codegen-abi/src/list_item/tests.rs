//! The generators run on an interpreting builder over byte-addressed memory
//! holding real object headers and `MoltVec` storage. The oracle is Python's
//! own rule written directly: `xs[i]` after the negative-index adjustment, in
//! range, on an exact list; everything else is the runtime's.

use std::collections::HashMap;
use std::vec::Vec;

use super::*;
use crate::frame_record::FrameBuilder;
use crate::word::{WordBuilder, WordComparison};
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_FLAG_HAS_PTRS, QNAN, TAG_BOOL, TAG_INT, TAG_NONE, TAG_PTR,
    TYPE_ID_DICT, TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, box_int_bits,
};

const LIST: u64 = 0x1_0000;
const STORAGE: u64 = 0x2_0000;
const DATA: u64 = 0x3_0000;
const COLD: u64 = 0xC01D;

fn boxed(address: u64) -> u64 {
    QNAN | TAG_PTR | address
}

fn int(value: i64) -> u64 {
    box_int_bits(value) as u64
}

fn boolean(value: bool) -> u64 {
    QNAN | TAG_BOOL | u64::from(value)
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Retain(u64),
    Release(u64),
    Cold,
}

#[derive(Default)]
struct Machine {
    bytes: HashMap<u64, u8>,
    locals: Vec<u64>,
    events: Vec<Event>,
    bailed: bool,
}

impl Machine {
    /// A list object of `type_id` whose storage holds `elements`: 8-byte
    /// words, or bytes for a bool list.
    fn list(type_id: u32, elements: &[u64]) -> Self {
        let mut m = Self::default();
        m.write_u32(LIST, HEADER_TYPE_ID_OFFSET, type_id);
        m.write_u32(LIST, HEADER_FLAGS_OFFSET, HEADER_FLAG_HAS_PTRS);
        // As the runtime allocates a list: an inline class word of zero.
        m.write_u32(
            LIST,
            HEADER_AUX_KIND_OFFSET - 2,
            u32::from(HEADER_AUX_KIND_CLASS_INLINE) << 16,
        );
        m.write_word(LIST, HEADER_AUX_OFFSET, 0);
        m.write_word(LIST, 0, STORAGE);
        m.write_word(STORAGE, LIST_STORAGE_DATA_OFFSET, DATA);
        m.write_word(STORAGE, LIST_STORAGE_LEN_OFFSET, elements.len() as u64);
        for (index, &element) in elements.iter().enumerate() {
            if type_id == TYPE_ID_LIST_BOOL {
                m.bytes.insert(DATA + index as u64, element as u8);
            } else {
                m.write_word(DATA, 8 * index as i32, element);
            }
        }
        m
    }

    fn write_bytes(&mut self, address: u64, offset: i32, value: u64, width: usize) {
        let at = address.wrapping_add_signed(offset.into());
        for byte in 0..width {
            self.bytes
                .insert(at + byte as u64, (value >> (8 * byte)) as u8);
        }
    }

    fn read_bytes(&self, address: u64, offset: i32, width: usize) -> u64 {
        let at = address.wrapping_add_signed(offset.into());
        (0..width).fold(0, |value, byte| {
            let bits = *self
                .bytes
                .get(&(at + byte as u64))
                .expect("generated code read memory nothing wrote");
            value | (u64::from(bits) << (8 * byte))
        })
    }

    fn write_word(&mut self, address: u64, offset: i32, value: u64) {
        self.write_bytes(address, offset, value, 8);
    }

    fn write_u32(&mut self, address: u64, offset: i32, value: u32) {
        self.write_bytes(address, offset, value.into(), 4);
    }

    fn element(&self, index: u64) -> u64 {
        self.read_bytes(DATA, 8 * index as i32, 8)
    }

    fn get(&mut self, index: u64) -> u64 {
        generate_list_item_get(self, boxed(LIST), index, ListLocking::Gil, |m| {
            m.events.push(Event::Cold);
            COLD
        })
    }

    fn set(&mut self, index: u64, value: u64) -> u64 {
        generate_list_item_set(
            self,
            boxed(LIST),
            index,
            value,
            ListLocking::Gil,
            QNAN | TAG_NONE,
            |m| {
                m.events.push(Event::Cold);
                COLD
            },
        )
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
    fn load_word(&mut self, address: u64, offset: i32) -> u64 {
        if self.bailed {
            return 0;
        }
        self.read_bytes(address, offset, 8)
    }
    fn store_word(&mut self, address: u64, offset: i32, value: u64) {
        if !self.bailed {
            self.write_word(address, offset, value);
        }
    }
    fn load_header_u32(&mut self, pointer: u64, offset: i32) -> u64 {
        if self.bailed {
            return 0;
        }
        self.read_bytes(pointer, offset, 4)
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
        unreachable!("a list item site pushes no frame")
    }
    fn exit_slow(&mut self, _base: u64) {
        unreachable!("a list item site pops no frame")
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
        unreachable!("a list item site has no loop")
    }
}

/// Python's index rule, the oracle: the element position, or None when the
/// runtime raises IndexError.
fn python_position(index: i64, len: usize) -> Option<u64> {
    let len = len as i64;
    let adjusted = if index < 0 { index + len } else { index };
    (0..len).contains(&adjusted).then_some(adjusted as u64)
}

const OBJECTS: [u64; 3] = [
    0x7ffc_0000_00a0_0000,
    0x7ffc_0000_00b0_0000,
    0x7ffc_0000_00c0_0000,
];

#[test]
fn a_word_list_read_matches_pythons_index_rule_on_every_edge() {
    for index in [-5i64, -4, -3, -1, 0, 1, 2, 3, 4, 1 << 45, -(1 << 45)] {
        let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
        let got = m.get(int(index));
        match python_position(index, OBJECTS.len()) {
            Some(position) => {
                let item = OBJECTS[position as usize];
                assert_eq!((got, &m.events[..]), (item, &[Event::Retain(item)][..]));
            }
            None => assert_eq!((got, &m.events[..]), (COLD, &[Event::Cold][..])),
        }
    }
}

#[test]
fn a_compact_int_list_boxes_its_inline_ints_without_a_retain() {
    let mut m = Machine::list(TYPE_ID_LIST_INT, &[7, (-3i64) as u64]);
    assert_eq!(m.get(int(-1)), int(-3));
    assert_eq!(m.get(int(0)), int(7));
    assert!(m.events.is_empty());
    assert_eq!(m.set(int(0), int(-9)), QNAN | TAG_NONE);
    assert_eq!(m.element(0) as i64, -9);
    // A value the compact kind cannot hold converts the storage: runtime.
    assert_eq!(m.set(int(1), OBJECTS[0]), COLD);
    assert_eq!(m.element(1) as i64, -3);
    assert_eq!(m.events, [Event::Cold]);
    // A compact bool list is the runtime's.
    let mut m = Machine::list(TYPE_ID_LIST_BOOL, &[1, 0]);
    assert_eq!(m.get(int(0)), COLD);
    assert_eq!(m.set(int(0), boolean(false)), COLD);
}

/// Each named change sends the read to the runtime, which owns subclass
/// protocols, C views, other containers and every non-int index.
#[test]
fn everything_but_an_exact_list_at_an_inline_int_takes_the_runtime() {
    type Refusal = (&'static str, fn(&mut Machine) -> u64);
    let refusals: [Refusal; 7] = [
        ("subclass class edge", |m| {
            m.write_word(LIST, HEADER_AUX_OFFSET, 0x7ffc_0000_00e0_0000 & !7);
            int(0)
        }),
        ("sidecar", |m| {
            m.write_bytes(
                LIST,
                HEADER_AUX_KIND_OFFSET,
                crate::HEADER_AUX_KIND_SIDECAR.into(),
                2,
            );
            int(0)
        }),
        ("C view", |m| {
            m.write_u32(LIST, HEADER_FLAGS_OFFSET, HEADER_FLAG_HAS_ABI_VIEW);
            int(0)
        }),
        ("dict", |m| {
            m.write_u32(LIST, HEADER_TYPE_ID_OFFSET, TYPE_ID_DICT);
            int(0)
        }),
        ("bool index", |_| boolean(false)),
        ("big int index", |_| boxed(0x9_0000)),
        ("float index", |_| 0.0f64.to_bits()),
    ];
    for (name, refuse) in refusals {
        let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
        let index = refuse(&mut m);
        assert_eq!(m.get(index), COLD, "{name}");
        assert_eq!(m.events, [Event::Cold], "{name}");
    }
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    assert_eq!(
        generate_list_item_get(&mut m, int(3), int(0), ListLocking::Gil, |_| COLD),
        COLD,
        "a non-pointer container"
    );
}

#[test]
fn a_word_store_retains_the_new_value_before_releasing_the_old() {
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    let value = 0x7ffc_0000_00d0_0000;
    assert_eq!(m.set(int(-1), value), QNAN | TAG_NONE);
    assert_eq!(m.element(2), value);
    assert_eq!(m.events, [Event::Retain(value), Event::Release(OBJECTS[2])]);
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    assert_eq!(m.set(int(3), value), COLD);
    assert_eq!(m.events, [Event::Cold]);
    assert_eq!(m.element(2), OBJECTS[2], "a refused store changes nothing");
}

#[test]
fn a_free_threaded_build_always_takes_the_runtime() {
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    let read = generate_list_item_get(
        &mut m,
        boxed(LIST),
        int(0),
        ListLocking::FreeThreaded,
        |_| COLD,
    );
    assert_eq!((read, m.events.len()), (COLD, 0));
}

#[test]
fn the_inline_int_tag_is_the_runtime_one() {
    assert_eq!(int(5) & (QNAN | crate::TAG_MASK), QNAN | TAG_INT);
}

#[test]
fn a_list_without_an_aux_word_is_exact_too() {
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    m.write_bytes(
        LIST,
        HEADER_AUX_KIND_OFFSET,
        crate::HEADER_AUX_KIND_NONE.into(),
        2,
    );
    assert_eq!(m.get(int(1)), OBJECTS[1]);
    assert_eq!(m.events, [Event::Retain(OBJECTS[1])]);
}
