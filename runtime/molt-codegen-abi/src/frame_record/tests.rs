//! The generators run on an interpreting builder over simulated memory: each
//! word operation executes at once, so a test reads the machine state the
//! generated code would leave.

use std::collections::HashMap;
use std::vec::Vec;

use super::*;
use crate::word::{WordBuilder, WordComparison};
use crate::{FRAME_HOME_CELL, FRAME_HOME_PLAIN, FRAME_HOME_RAW_INT, FRAME_RECORD_OBSERVED};

const THREAD: u64 = 0x1_0000;
const CHUNK: u64 = 0x2_0000;
const CHUNK_BYTES: u64 = 0x1000;
const FUNCTION: u64 = 0x3_0000;
const CODE: u64 = 0x4_0000;
const SLOT: u64 = 7;
const SLOW_BASE: u64 = 0x9_0000;

fn boxed(address: u64) -> u64 {
    crate::QNAN | crate::TAG_PTR | address
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Retain(u64),
    Release(u64),
    EnterSlow(u64),
    ExitSlow(u64),
}

#[derive(Default)]
struct Machine {
    words: HashMap<u64, u64>,
    header_u32: HashMap<u64, u32>,
    locals: Vec<u64>,
    events: Vec<Event>,
    /// A `require` failed in the running `attempt`: the rest of its fast path
    /// does not execute.
    bailed: bool,
}

impl Machine {
    /// A thread with an empty chunk and a caller frame; a function for
    /// `SLOT` is handed over.
    fn new() -> Self {
        let mut machine = Self::default();
        machine.write(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET, CHUNK);
        machine.write(
            THREAD,
            THREAD_RECORD_ARENA_LIMIT_OFFSET,
            CHUNK + CHUNK_BYTES,
        );
        machine.write(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET, 0xCA11);
        machine.write(
            THREAD,
            THREAD_RECORD_PENDING_INVOCATION_OFFSET,
            boxed(FUNCTION),
        );
        machine.header_u32.insert(
            FUNCTION.wrapping_add_signed(HEADER_TYPE_ID_OFFSET.into()),
            TYPE_ID_FUNCTION,
        );
        machine.write(FUNCTION, FUNCTION_CODE_OFFSET, boxed(CODE));
        machine.write(CODE, CODE_FRAME_SLOT_OFFSET, SLOT + 1);
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
            .unwrap_or(0xDEAD_BEEF)
    }

    fn push(&mut self, homes: u32) -> u64 {
        let result = generate_frame_push(self, THREAD, SLOT, HomeCount::Constant(homes));
        self.locals[result]
    }

    fn pop(&mut self, base: u64, homes: HomeCount<u64>, descending: bool) {
        generate_frame_pop(self, THREAD, base, homes, descending);
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
    fn enter_slow(&mut self, code_slot: u64) -> u64 {
        self.events.push(Event::EnterSlow(code_slot));
        SLOW_BASE
    }
    fn exit_slow(&mut self, base: u64) {
        self.events.push(Event::ExitSlow(base));
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
    fn counted_loop(&mut self, count: u64, descending: bool, mut body: impl FnMut(&mut Self, u64)) {
        if self.bailed {
            return;
        }
        if descending {
            (0..count).rev().for_each(|index| body(self, index));
        } else {
            (0..count).for_each(|index| body(self, index));
        }
    }
}

#[test]
fn an_inline_push_links_a_record_that_owns_its_function_and_code() {
    let mut machine = Machine::new();
    let base = machine.push(2);
    assert_eq!(base, CHUNK + FRAME_RECORD_HEADER_BYTES as u64);
    assert_eq!(machine.read(base, FRAME_RECORD_PREV_OFFSET), 0xCA11);
    assert_eq!(
        machine.read(base, FRAME_RECORD_FUNCTION_OFFSET),
        boxed(FUNCTION)
    );
    assert_eq!(machine.read(base, FRAME_RECORD_CODE_OFFSET), boxed(CODE));
    assert_eq!(machine.read(base, FRAME_RECORD_FLAGS_OFFSET), 0);
    assert_eq!(machine.read(base, FRAME_RECORD_OBSERVER_OFFSET), 0);
    assert_eq!(
        machine.read(base, FRAME_LOCATION_OFFSET),
        FRAME_LOCATION_ABSENT
    );
    for word in 0..4 {
        assert_eq!(
            machine.read(base, word * 8),
            0,
            "home word {word} is unbound"
        );
    }
    assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), base);
    assert_eq!(
        machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET),
        base + 2 * FRAME_HOME_BYTES as u64
    );
    // The handoff is consumed: a later direct call cannot take it again.
    assert_eq!(
        machine.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET),
        0
    );
    assert_eq!(
        machine.events,
        [Event::Retain(boxed(FUNCTION)), Event::Retain(boxed(CODE))]
    );
}

/// A named change to the machine that makes the inline push refuse.
type Refusal = (&'static str, fn(&mut Machine));

/// Every way the handoff or the arena can refuse the inline push enters the
/// runtime and leaves the thread record untouched.
#[test]
fn a_push_without_a_matching_handoff_or_room_enters_the_runtime() {
    let refusals: [Refusal; 6] = [
        ("no handoff", |m| {
            m.write(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET, 0)
        }),
        ("a task, not a function", |m| {
            m.header_u32.insert(
                FUNCTION.wrapping_add_signed(HEADER_TYPE_ID_OFFSET.into()),
                TYPE_ID_FUNCTION + 1,
            );
        }),
        ("a function without code", |m| {
            m.write(FUNCTION, FUNCTION_CODE_OFFSET, 0)
        }),
        ("another slot's function", |m| {
            m.write(CODE, CODE_FRAME_SLOT_OFFSET, SLOT + 2)
        }),
        ("an empty arena", |m| {
            m.write(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET, 0);
            m.write(THREAD, THREAD_RECORD_ARENA_LIMIT_OFFSET, 0);
        }),
        ("one byte short", |m| {
            let need = FRAME_RECORD_HEADER_BYTES as u64 + 2 * FRAME_HOME_BYTES as u64;
            m.write(THREAD, THREAD_RECORD_ARENA_LIMIT_OFFSET, CHUNK + need - 1);
        }),
    ];
    for (name, refuse) in refusals {
        let mut machine = Machine::new();
        refuse(&mut machine);
        let handoff = machine.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET);
        let top = machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET);
        assert_eq!(machine.push(2), SLOW_BASE, "{name}");
        assert_eq!(machine.events, [Event::EnterSlow(SLOT)], "{name}");
        assert_eq!(
            machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET),
            0xCA11,
            "{name}"
        );
        assert_eq!(
            machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET),
            top,
            "{name}"
        );
        assert_eq!(
            machine.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET),
            handoff,
            "{name}"
        );
    }
}

#[test]
fn a_push_that_fills_the_chunk_exactly_stays_inline() {
    let mut machine = Machine::new();
    let need = FRAME_RECORD_HEADER_BYTES as u64 + 2 * FRAME_HOME_BYTES as u64;
    machine.write(THREAD, THREAD_RECORD_ARENA_LIMIT_OFFSET, CHUNK + need);
    assert_eq!(machine.push(2), CHUNK + FRAME_RECORD_HEADER_BYTES as u64);
}

/// Three homes that hold references and one raw integer, with distinct bits.
fn bind_homes(machine: &mut Machine, base: u64) {
    for (slot, kind) in [
        FRAME_HOME_PLAIN,
        FRAME_HOME_RAW_INT,
        FRAME_HOME_CELL,
        FRAME_HOME_PLAIN,
    ]
    .into_iter()
    .enumerate()
    {
        let offset = (slot as i64 * FRAME_HOME_BYTES) as i32;
        machine.write(base, offset + FRAME_HOME_KIND_OFFSET, kind as u64);
        machine.write(base, offset + FRAME_HOME_BITS_OFFSET, 0x100 + slot as u64);
    }
}

#[test]
fn an_inline_pop_unlinks_then_releases_in_the_frame_policy_order() {
    for (descending, order) in [
        (false, [0x100, 0x102, 0x103]),
        (true, [0x103, 0x102, 0x100]),
    ] {
        for homes in [HomeCount::Constant(4), HomeCount::Dynamic(4)] {
            let mut machine = Machine::new();
            let base = machine.push(4);
            bind_homes(&mut machine, base);
            machine.events.clear();
            machine.pop(base, homes, descending);
            let mut expected: Vec<Event> = order.into_iter().map(Event::Release).collect();
            expected.extend([Event::Release(boxed(FUNCTION)), Event::Release(boxed(CODE))]);
            assert_eq!(machine.events, expected, "descending={descending}");
            assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), 0xCA11);
            assert_eq!(machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET), CHUNK);
        }
    }
}

#[test]
fn a_pop_above_the_inline_threshold_releases_through_the_loop_in_order() {
    let homes = FRAME_POP_INLINE_HOMES + 3;
    for descending in [false, true] {
        let mut machine = Machine::new();
        let base = machine.push(homes);
        for slot in 0..homes {
            let offset = (i64::from(slot) * FRAME_HOME_BYTES) as i32;
            machine.write(
                base,
                offset + FRAME_HOME_KIND_OFFSET,
                FRAME_HOME_PLAIN as u64,
            );
            machine.write(
                base,
                offset + FRAME_HOME_BITS_OFFSET,
                0x100 + u64::from(slot),
            );
        }
        machine.events.clear();
        machine.pop(base, HomeCount::Constant(homes), descending);
        let mut slots: Vec<u32> = (0..homes).collect();
        if descending {
            slots.reverse();
        }
        let released: Vec<Event> = slots
            .into_iter()
            .map(|slot| Event::Release(0x100 + u64::from(slot)))
            .collect();
        assert_eq!(machine.events[..homes as usize], released[..]);
    }
}

#[test]
fn a_pop_of_an_observed_record_exits_through_the_runtime_untouched() {
    let mut machine = Machine::new();
    let base = machine.push(4);
    bind_homes(&mut machine, base);
    machine.write(base, FRAME_RECORD_FLAGS_OFFSET, FRAME_RECORD_OBSERVED);
    let top = machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET);
    machine.events.clear();
    machine.pop(base, HomeCount::Constant(4), false);
    assert_eq!(machine.events, [Event::ExitSlow(base)]);
    // The runtime unlinks and returns the storage; generated code did neither.
    assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), base);
    assert_eq!(machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET), top);
}

#[test]
fn a_failed_entry_pops_nothing() {
    let mut machine = Machine::new();
    machine.pop(0, HomeCount::Constant(4), false);
    assert!(machine.events.is_empty());
    assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), 0xCA11);
}

#[test]
fn a_runtime_linked_frame_exits_through_the_runtime_only_when_entered() {
    let mut machine = Machine::new();
    generate_linked_frame_pop(&mut machine, 0);
    assert!(machine.events.is_empty());
    generate_linked_frame_pop(&mut machine, SLOW_BASE);
    assert_eq!(machine.events, [Event::ExitSlow(SLOW_BASE)]);
}

#[test]
fn an_entry_value_round_trips_the_frame_shape() {
    for shape in [
        FrameShape {
            homes: 0,
            descending: false,
        },
        FrameShape {
            homes: 5,
            descending: true,
        },
        FrameShape {
            homes: u32::MAX,
            descending: true,
        },
    ] {
        assert!(shape.entry_value() >= 0);
        assert_eq!(FrameShape::from_entry_value(shape.entry_value()), shape);
    }
}
