//! An interpreting `FrameBuilder` over byte-addressed memory, shared by the
//! generator model tests. A generator runs on it exactly as a backend spells
//! it: a failed `require` abandons the fast path's effects and runs the slow
//! path, and every retain, release and cold call is recorded in order.

use std::collections::HashMap;
use std::vec;
use std::vec::Vec;

use crate::frame_record::FrameBuilder;
use crate::word::{WordBuilder, WordComparison};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Retain(u64),
    Release(u64),
    Cold,
    /// A header flag ORed in (`field_slot::FieldSlotBuilder`).
    Flag(u32),
    /// A task-construction runtime call (`task_site::TaskSiteBuilder`) and
    /// its arguments.
    Call(&'static str, Vec<u64>),
}

#[derive(Default)]
pub(crate) struct Machine {
    pub(crate) bytes: HashMap<u64, u8>,
    pub(crate) locals: Vec<u64>,
    pub(crate) events: Vec<Event>,
    pub(crate) bailed: bool,
    /// What `molt_task_new` returns (`task_site::TaskSiteBuilder`).
    pub(crate) task_new_result: u64,
}

impl Machine {
    pub(crate) fn write_bytes(&mut self, address: u64, offset: i32, value: u64, width: usize) {
        let at = address.wrapping_add_signed(offset.into());
        for byte in 0..width {
            self.bytes
                .insert(at + byte as u64, (value >> (8 * byte)) as u8);
        }
    }

    pub(crate) fn read_bytes(&self, address: u64, offset: i32, width: usize) -> u64 {
        let at = address.wrapping_add_signed(offset.into());
        (0..width).fold(0, |value, byte| {
            let bits = *self
                .bytes
                .get(&(at + byte as u64))
                .expect("generated code read memory nothing wrote");
            value | (u64::from(bits) << (8 * byte))
        })
    }

    pub(crate) fn write_word(&mut self, address: u64, offset: i32, value: u64) {
        self.write_bytes(address, offset, value, 8);
    }

    pub(crate) fn write_u32(&mut self, address: u64, offset: i32, value: u32) {
        self.write_bytes(address, offset, value.into(), 4);
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
        self.read_bytes(address, offset, 8)
    }
    fn store_word(&mut self, address: u64, offset: i32, value: u64) {
        if !self.bailed {
            self.write_word(address, offset, value);
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
        unreachable!("the modeled sites push no frame")
    }
    fn exit_slow(&mut self, _base: u64) {
        unreachable!("the modeled sites pop no frame")
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
        for step in 0..count {
            body(self, if descending { count - 1 - step } else { step });
        }
    }
}

/// The cancel token and async generator words the modeled runtime returns.
pub(crate) const MODEL_CANCEL_TOKEN: u64 = 0x7ff4_0000_0000_c0de;
pub(crate) const MODEL_ASYNCGEN: u64 = 0x7ffc_0000_0000_a5a5;

impl crate::task_site::TaskSiteBuilder for Machine {
    fn task_new(&mut self, poll: u64, frame_bytes: u64, kind: u64, function: u64) -> u64 {
        self.events.push(Event::Call(
            "task_new",
            vec![poll, frame_bytes, kind, function],
        ));
        self.task_new_result
    }
    fn cancel_token_get_current(&mut self) -> u64 {
        self.events
            .push(Event::Call("cancel_token_get_current", vec![]));
        MODEL_CANCEL_TOKEN
    }
    fn task_register_execution(&mut self, task: u64, token: u64) {
        self.events
            .push(Event::Call("task_register_execution", vec![task, token]));
    }
    fn asyncgen_new(&mut self, generator: u64) -> u64 {
        self.events
            .push(Event::Call("asyncgen_new", vec![generator]));
        MODEL_ASYNCGEN
    }
    fn int_from_i64(&mut self, raw: u64) -> u64 {
        self.events.push(Event::Call("int_from_i64", vec![raw]));
        MODEL_BIG_INT
    }
}

/// The heap integer the modeled runtime mints for a raw value outside the
/// inline range.
pub(crate) const MODEL_BIG_INT: u64 = 0x7ffc_0000_0000_b161;
