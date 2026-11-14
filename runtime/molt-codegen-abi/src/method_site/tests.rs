//! The generators run on an interpreting builder over simulated memory. The
//! oracle is CPython's specialization contract for `LOAD_ATTR_METHOD_*` and
//! `CALL_PY_*`, restated as the expected machine state per case.

use std::collections::HashMap;
use std::vec;
use std::vec::Vec;

use super::*;
use crate::frame_record::FrameBuilder;
use crate::word::{WordBuilder, WordComparison};

const THREAD: u64 = 0x1_0000;
const RECORD: u64 = 0x2_0000;
const OBJECT: u64 = 0x3_0000;
const CLASS: u64 = 0x4_0000;
const FUNCTION: u64 = 0x5_0000;
const SELF_OUT: u64 = 0x6_0000;
const ENTRY: u64 = 0x7_0000;
const SIZE_CLASS: u16 = 9;
const VERSION: u64 = 41;
const DICT_AT: u64 = 16;
const OUTER: u64 = 0xA0A0;
const COLD: u64 = 0xC01D;
const RESULT: u64 = 0x5E5;

fn boxed(address: u64) -> u64 {
    crate::QNAN | crate::TAG_PTR | address
}

fn int(value: i64) -> u64 {
    crate::box_int_bits(value) as u64
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Retain(u64),
    Release(u64),
    Cold,
    Enter,
    Exit,
    /// The entry, its arguments, and the handoff word during the call.
    Call(u64, Vec<u64>, u64),
}

struct Machine {
    words: HashMap<u64, u64>,
    header_u32: HashMap<u64, u32>,
    locals: Vec<u64>,
    events: Vec<Event>,
    bailed: bool,
    admit_recursion: bool,
}

impl Machine {
    /// An object of `CLASS` with a dictionary slot holding None, and a
    /// record filled for it: entry arity 2, two usable defaults.
    fn new() -> Self {
        let mut machine = Self {
            words: HashMap::new(),
            header_u32: HashMap::new(),
            locals: Vec::new(),
            events: Vec::new(),
            bailed: false,
            admit_recursion: true,
        };
        machine.header_u32.insert(
            OBJECT.wrapping_add_signed(METHOD_SITE_HEADER_SHAPE_OFFSET.into()),
            method_site_header_shape(SIZE_CLASS) as u32,
        );
        // The class word carries an ownership tag in its low bits.
        machine.write(
            OBJECT,
            crate::HEADER_AUX_OFFSET,
            boxed(CLASS) | crate::HEADER_CLASS_WORD_BORROWED,
        );
        machine.write(OBJECT, DICT_AT as i32, crate::box_none_bits() as u64);
        machine.write(CLASS, CLASS_LAYOUT_VERSION_OFFSET, VERSION);
        machine.write(RECORD, METHOD_SITE_CLASS_OFFSET, boxed(CLASS));
        machine.write(
            RECORD,
            METHOD_SITE_SHAPE_OFFSET,
            method_site_header_shape(SIZE_CLASS),
        );
        machine.write(RECORD, METHOD_SITE_VERSION_OFFSET, VERSION);
        machine.write(RECORD, METHOD_SITE_DICT_OFFSET, DICT_AT);
        machine.write(RECORD, METHOD_SITE_FUNCTION_OFFSET, boxed(FUNCTION));
        machine.write(RECORD, METHOD_SITE_ENTRY_OFFSET, ENTRY);
        machine.write(RECORD, METHOD_SITE_ARITY_OFFSET, 2);
        machine.write(RECORD, METHOD_SITE_DEFAULT_COUNT_OFFSET, 2);
        machine.write(RECORD, METHOD_SITE_DEFAULTS_OFFSET, int(10));
        machine.write(RECORD, METHOD_SITE_DEFAULTS_OFFSET + 8, int(20));
        machine.write(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET, OUTER);
        machine
    }

    fn write(&mut self, address: u64, offset: i32, value: u64) {
        self.words
            .insert(address.wrapping_add_signed(offset.into()), value);
    }

    fn read(&self, address: u64, offset: i32) -> u64 {
        self.words
            .get(&address.wrapping_add_signed(offset.into()))
            .copied()
            .unwrap_or_else(|| panic!("generated code read unmapped 0x{address:x}{offset:+}"))
    }

    fn load(&mut self, receiver: u64) -> u64 {
        self.write(SELF_OUT, 0, 0);
        generate_method_site_load(self, RECORD, receiver, SELF_OUT, |b| {
            b.events.push(Event::Cold);
            b.write(SELF_OUT, 0, crate::loaded_method_null_bits() as u64);
            COLD
        })
    }

    fn call(&mut self, callable: u64, self_word: u64, args: &[u64]) -> u64 {
        generate_method_site_call(self, THREAD, RECORD, callable, self_word, args, |b| {
            b.events.push(Event::Cold);
            COLD
        })
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
            0
        } else {
            self.read(address, offset)
        }
    }
    fn store_word(&mut self, address: u64, offset: i32, value: u64) {
        if !self.bailed {
            self.write(address, offset, value);
        }
    }
    fn load_header_u32(&mut self, pointer: u64, offset: i32) -> u64 {
        if self.bailed {
            return 0;
        }
        u64::from(
            self.header_u32
                .get(&pointer.wrapping_add_signed(offset.into()))
                .copied()
                .expect("generated code read the header of a non-object"),
        )
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
        unreachable!("a method site links no frame")
    }
    fn exit_slow(&mut self, _base: u64) {
        unreachable!("a method site links no frame")
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
        unreachable!("a method site has no loop")
    }
}

impl MethodSiteBuilder for Machine {
    fn recursion_enter(&mut self) -> u64 {
        if self.bailed {
            return 0;
        }
        if self.admit_recursion {
            self.events.push(Event::Enter);
            1
        } else {
            0
        }
    }
    fn recursion_exit(&mut self) {
        if !self.bailed {
            self.events.push(Event::Exit);
        }
    }
    fn call_entry(&mut self, entry: u64, self_word: u64, args: &[u64], defaults: &[u64]) -> u64 {
        if self.bailed {
            return 0;
        }
        let handoff = self.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET);
        let words = [&[self_word][..], args, defaults].concat();
        self.events.push(Event::Call(entry, words, handoff));
        RESULT
    }
}

#[test]
fn a_matching_receiver_loads_the_recorded_function_and_itself() {
    let mut m = Machine::new();
    let receiver = boxed(OBJECT);
    assert_eq!(m.load(receiver), boxed(FUNCTION));
    assert_eq!(m.read(SELF_OUT, 0), receiver);
    assert_eq!(
        m.events,
        [Event::Retain(boxed(FUNCTION)), Event::Retain(receiver)]
    );
}

#[test]
fn an_absent_dictionary_reads_as_zero_or_none_and_no_dict_skips_the_read() {
    let mut m = Machine::new();
    m.write(OBJECT, DICT_AT as i32, 0);
    assert_eq!(m.load(boxed(OBJECT)), boxed(FUNCTION));
    let mut m = Machine::new();
    m.write(RECORD, METHOD_SITE_DICT_OFFSET, METHOD_SITE_NO_DICT);
    m.words.remove(&(OBJECT + DICT_AT));
    assert_eq!(m.load(boxed(OBJECT)), boxed(FUNCTION));
}

/// Each guard identity the runtime bumps or rewrites sends the load to the
/// runtime, which runs no inline retain and writes the out word itself.
#[test]
fn every_changed_identity_takes_the_runtime_load() {
    type Mutation = fn(&mut Machine);
    let cases: [(&str, Mutation); 6] = [
        ("an instance dictionary shadows", |m| {
            m.write(OBJECT, DICT_AT as i32, boxed(0x8_0000));
        }),
        ("the class was mutated", |m| {
            m.write(CLASS, CLASS_LAYOUT_VERSION_OFFSET, VERSION + 1);
        }),
        ("__class__ was assigned", |m| {
            m.write(OBJECT, crate::HEADER_AUX_OFFSET, boxed(0x9_0000));
        }),
        ("the class moved to a sidecar", |m| {
            m.header_u32.insert(
                OBJECT.wrapping_add_signed(METHOD_SITE_HEADER_SHAPE_OFFSET.into()),
                ((u32::from(crate::HEADER_AUX_KIND_SIDECAR)) << 16) | u32::from(SIZE_CLASS),
            );
        }),
        ("the record is empty", |m| {
            m.write(RECORD, METHOD_SITE_CLASS_OFFSET, METHOD_SITE_EMPTY_CLASS);
        }),
        ("an empty record meets a classless header", |m| {
            m.write(RECORD, METHOD_SITE_CLASS_OFFSET, METHOD_SITE_EMPTY_CLASS);
            m.write(OBJECT, crate::HEADER_AUX_OFFSET, 0);
        }),
    ];
    for (case, mutate) in cases {
        let mut m = Machine::new();
        mutate(&mut m);
        assert_eq!(m.load(boxed(OBJECT)), COLD, "{case}");
        assert_eq!(m.events, [Event::Cold], "{case}");
        assert_eq!(
            m.read(SELF_OUT, 0),
            crate::loaded_method_null_bits() as u64,
            "{case}"
        );
    }
    let mut m = Machine::new();
    assert_eq!(m.load(int(3)), COLD, "an inline int is no object");
}

#[test]
fn an_exact_call_enters_the_entry_with_the_callable_handed_over() {
    let mut m = Machine::new();
    let (callable, receiver) = (boxed(FUNCTION), boxed(OBJECT));
    assert_eq!(m.call(callable, receiver, &[int(1)]), RESULT);
    assert_eq!(
        m.events,
        [
            Event::Enter,
            Event::Call(ENTRY, vec![receiver, int(1)], callable),
            Event::Exit,
            Event::Release(callable),
        ]
    );
    assert_eq!(
        m.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET),
        OUTER
    );
}

#[test]
fn a_short_call_appends_the_trailing_defaults_each_retained() {
    let (callable, receiver) = (boxed(FUNCTION), boxed(OBJECT));
    let mut m = Machine::new();
    m.write(RECORD, METHOD_SITE_ARITY_OFFSET, 3);
    assert_eq!(m.call(callable, receiver, &[int(1)]), RESULT);
    assert_eq!(
        m.events,
        [
            Event::Enter,
            Event::Retain(int(20)),
            Event::Call(ENTRY, vec![receiver, int(1), int(20)], callable),
            Event::Exit,
            Event::Release(callable),
        ]
    );
    let mut m = Machine::new();
    m.write(RECORD, METHOD_SITE_ARITY_OFFSET, 4);
    assert_eq!(m.call(callable, receiver, &[int(1)]), RESULT);
    assert_eq!(
        m.events[3],
        Event::Call(ENTRY, vec![receiver, int(1), int(10), int(20)], callable)
    );
}

#[test]
fn a_call_the_record_does_not_describe_takes_the_runtime_untouched() {
    type Mutation = fn(&mut Machine);
    let callable = boxed(FUNCTION);
    let cases: [(&str, u64, Mutation); 6] = [
        ("NULL self", crate::loaded_method_null_bits() as u64, |_| {}),
        ("another callable", boxed(OBJECT), |m| {
            m.write(RECORD, METHOD_SITE_FUNCTION_OFFSET, boxed(0xB_0000));
        }),
        ("no direct entry", boxed(OBJECT), |m| {
            m.write(RECORD, METHOD_SITE_ENTRY_OFFSET, 0);
        }),
        ("too few defaults", boxed(OBJECT), |m| {
            m.write(RECORD, METHOD_SITE_ARITY_OFFSET, 5);
        }),
        ("too many arguments", boxed(OBJECT), |m| {
            m.write(RECORD, METHOD_SITE_ARITY_OFFSET, 1);
        }),
        ("the recursion limit", boxed(OBJECT), |m| {
            m.admit_recursion = false;
        }),
    ];
    for (case, self_word, mutate) in cases {
        let mut m = Machine::new();
        mutate(&mut m);
        assert_eq!(m.call(callable, self_word, &[int(1)]), COLD, "{case}");
        assert_eq!(m.events, [Event::Cold], "{case}");
        assert_eq!(
            m.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET),
            OUTER,
            "{case}"
        );
    }
}

#[test]
fn the_record_image_is_empty_and_carries_its_name() {
    for name in [&b""[..], b"m", b"method_x", b"a_longer_name"] {
        let mut image = vec![0xAA; method_site_record_size(name.len())];
        write_method_site_record_image(name, &mut image);
        let word = |offset: i32| {
            u64::from_le_bytes(
                image[offset as usize..offset as usize + 8]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(word(METHOD_SITE_CLASS_OFFSET), METHOD_SITE_EMPTY_CLASS);
        assert_eq!(word(METHOD_SITE_NAME_LEN_OFFSET), name.len() as u64);
        let at = METHOD_SITE_NAME_BYTES_OFFSET as usize;
        assert_eq!(&image[at..at + name.len()], name);
        assert!(
            image[8..METHOD_SITE_NAME_LEN_OFFSET as usize]
                .iter()
                .all(|&b| b == 0)
        );
        assert!(image[at + name.len()..].iter().all(|&b| b == 0));
        assert_eq!(image.len() % 8, 0);
    }
}
