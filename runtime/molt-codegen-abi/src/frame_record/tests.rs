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
const RECURSION_LIMIT: u64 = 0x5_0000;
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

impl crate::thread_record::RecordWordBuilder for Machine {
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
fn a_frame_probe_checks_admission_without_reserving_or_retaining() {
    let mut machine = Machine::new();
    machine.write(THREAD, crate::THREAD_RECORD_EXCEPTION_PENDING_OFFSET, 0);
    machine.write(THREAD, crate::THREAD_RECORD_RECURSION_DEPTH_OFFSET, 8);
    machine.write(
        THREAD,
        crate::THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET,
        RECURSION_LIMIT,
    );
    machine.write(RECURSION_LIMIT, 0, 10);
    let code_slot = machine.read(CODE, CODE_FRAME_SLOT_OFFSET) - 1;
    let function = boxed(FUNCTION);
    let before_top = machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET);
    let before_frame = machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET);
    let before_handoff = machine.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET);
    let callable_only = FrameReservation::default();
    let one = FrameReservation::from_counts(2, 1).unwrap();
    let two = FrameReservation::from_counts(5, 2).unwrap();
    let three = FrameReservation::from_counts(13, 3).unwrap();
    assert_eq!(
        two.arena_bytes,
        (2 * FRAME_RECORD_HEADER_BYTES + 5 * FRAME_HOME_BYTES) as u32
    );
    assert!(FrameReservation::from_counts(u32::MAX, 1).is_none());
    assert!(FrameReservation::from_counts(0, u32::MAX).is_none());
    let chunk_homes =
        ((CHUNK_BYTES - FRAME_RECORD_HEADER_BYTES as u64) / FRAME_HOME_BYTES as u64) as u32;
    let exact = FrameReservation::from_counts(chunk_homes, 1).unwrap();
    let oversized = FrameReservation::from_counts(chunk_homes + 1, 1).unwrap();
    for (slot, reservation, expected) in [
        (code_slot, callable_only, 1),
        (code_slot, one, 1),
        (code_slot, two, 1),
        (code_slot, three, 0),
        (code_slot, exact, 1),
        (code_slot, oversized, 0),
        (code_slot + 1, callable_only, 0),
        (code_slot + 1, one, 0),
    ] {
        assert_eq!(
            generate_frame_entry_admits(&mut machine, THREAD, function, slot, reservation),
            expected,
            "slot={slot}, reservation={reservation:?}"
        );
    }
    for (reservation, expected) in [(exact, 1), (oversized, 0), (two, 1), (three, 0)] {
        assert_eq!(
            generate_frame_probe(
                &mut machine,
                THREAD,
                0,
                0,
                FrameProbe::Region {
                    arena_bytes: reservation.arena_bytes,
                    recursion_depth: reservation.recursion_depth,
                    virtual_exception_level: false,
                },
            ),
            expected,
        );
    }
    let admitted_arena = FrameReservation {
        arena_bytes: 0,
        ..two
    };
    for (limit, expected) in [(9, 0), (10, 1)] {
        machine.write(RECURSION_LIMIT, 0, limit);
        for callable_admitted in [false, true] {
            assert_eq!(
                generate_frame_probe(
                    &mut machine,
                    THREAD,
                    function,
                    code_slot + u64::from(callable_admitted),
                    FrameProbe::Entry(FrameReservation {
                        callable_admitted,
                        ..admitted_arena
                    }),
                ),
                expected,
            );
        }
    }
    let virtual_level = FrameReservation {
        virtual_exception_level: true,
        ..two
    };
    for (depth, baseline, virtual_depth, expected) in [
        (6, 4, 6, 1),
        (6, 6, 6, 1),
        (6, 4, crate::NO_VIRTUAL_EXCEPTION_DEPTH, 0),
        (6, 4, 7, 0),
        (6, 7, 6, 0),
        (1 << 32, 4, 1 << 32, 0),
    ] {
        machine.write(
            THREAD,
            crate::THREAD_RECORD_EXCEPTION_HANDLER_DEPTH_OFFSET,
            depth,
        );
        machine.write(
            THREAD,
            crate::THREAD_RECORD_EXCEPTION_STACK_BASELINE_OFFSET,
            baseline,
        );
        machine.write(
            THREAD,
            crate::THREAD_RECORD_VIRTUAL_EXCEPTION_DEPTH_OFFSET,
            virtual_depth,
        );
        assert_eq!(
            generate_frame_entry_admits(&mut machine, THREAD, function, code_slot, virtual_level),
            expected,
            "depth={depth}, baseline={baseline}, virtual_depth={virtual_depth}"
        );
        assert_eq!(
            generate_frame_probe(
                &mut machine,
                THREAD,
                0,
                0,
                FrameProbe::Region {
                    arena_bytes: two.arena_bytes,
                    recursion_depth: two.recursion_depth,
                    virtual_exception_level: true,
                },
            ),
            expected,
        );
    }
    machine.write(THREAD, crate::THREAD_RECORD_EXCEPTION_PENDING_OFFSET, 1);
    assert_eq!(
        generate_frame_entry_admits(&mut machine, THREAD, function, code_slot, two),
        0
    );
    assert_eq!(
        generate_frame_probe(
            &mut machine,
            THREAD,
            0,
            0,
            FrameProbe::Region {
                arena_bytes: 0,
                recursion_depth: 0,
                virtual_exception_level: false
            },
        ),
        0,
    );
    assert_eq!(
        machine.read(THREAD, THREAD_RECORD_ARENA_TOP_OFFSET),
        before_top
    );
    assert_eq!(
        machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET),
        before_frame
    );
    assert_eq!(
        machine.read(THREAD, THREAD_RECORD_PENDING_INVOCATION_OFFSET),
        before_handoff
    );
    assert_eq!(
        machine.read(THREAD, crate::THREAD_RECORD_RECURSION_DEPTH_OFFSET),
        8
    );
    assert!(
        machine.events.is_empty(),
        "the probe neither retains nor calls the runtime"
    );
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

const TASK: u64 = 0x5_0000;
const SIDECAR: u64 = 0x6_0000;

/// A task whose constructor initialized its persistent record.
fn stateful_machine() -> (Machine, u64) {
    let mut machine = Machine::new();
    machine.write(TASK, HEADER_AUX_OFFSET, SIDECAR);
    let base = SIDECAR + SIDECAR_FRAME_RECORD_BASE_OFFSET as u64;
    machine.write(base, FRAME_RECORD_FLAGS_OFFSET, FRAME_RECORD_STATEFUL);
    machine.write(base, FRAME_RECORD_PREV_OFFSET, 0x5747E);
    machine.write(base, FRAME_LOCATION_OFFSET, 42);
    (machine, base)
}

#[test]
fn a_resume_links_the_persistent_record_and_a_suspension_unlinks_it() {
    let (mut machine, base) = stateful_machine();
    for _ in 0..2 {
        let linked = generate_stateful_frame_link(&mut machine, THREAD, TASK, SLOT);
        assert_eq!(machine.locals[linked], base);
        assert_eq!(machine.read(base, FRAME_RECORD_PREV_OFFSET), 0xCA11);
        assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), base);
        generate_stateful_frame_unlink(&mut machine, THREAD, base);
        assert_eq!(machine.read(base, FRAME_RECORD_PREV_OFFSET), 0);
        assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), 0xCA11);
    }
    // Nothing is retained, released or saved: the record keeps its facts and
    // its location word across the suspension.
    assert!(machine.events.is_empty());
    assert_eq!(machine.read(base, FRAME_LOCATION_OFFSET), 42);
    assert_eq!(
        machine.read(base, FRAME_RECORD_FLAGS_OFFSET),
        FRAME_RECORD_STATEFUL
    );
}

#[test]
fn a_task_without_an_initialized_record_links_through_the_runtime() {
    let (mut machine, base) = stateful_machine();
    machine.write(base, FRAME_RECORD_FLAGS_OFFSET, 0);
    let linked = generate_stateful_frame_link(&mut machine, THREAD, TASK, SLOT);
    assert_eq!(machine.locals[linked], SLOW_BASE);
    assert_eq!(machine.events, [Event::EnterSlow(SLOT)]);
    assert_eq!(machine.read(THREAD, THREAD_RECORD_FRAME_TOP_OFFSET), 0xCA11);
    assert_eq!(machine.read(base, FRAME_RECORD_PREV_OFFSET), 0x5747E);
    // The runtime-linked record is owned: its exit is the runtime's.
    machine.write(
        SLOW_BASE,
        FRAME_RECORD_FLAGS_OFFSET,
        crate::FRAME_RECORD_OWNED,
    );
    machine.events.clear();
    generate_stateful_frame_unlink(&mut machine, THREAD, SLOW_BASE);
    assert_eq!(machine.events, [Event::ExitSlow(SLOW_BASE)]);
    machine.events.clear();
    generate_stateful_frame_unlink(&mut machine, THREAD, 0);
    assert!(machine.events.is_empty());
}
