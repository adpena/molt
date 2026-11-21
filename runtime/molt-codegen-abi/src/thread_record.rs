//! Words of the thread record that compiled code reads with plain loads, as
//! CPython's code reads its `PyThreadState`.
//!
//! One generator owns the address chain: the invocation's attached execution
//! context lends the thread record (`ATTACHED_CONTEXT_THREAD_RECORD_OFFSET`),
//! and each word sits at a fixed record offset that the runtime asserts
//! (molt-runtime `state/tls.rs`). A backend spells the two operations only.

use crate::attached_context_generated::ATTACHED_CONTEXT_THREAD_RECORD_OFFSET;
use crate::word::{WordBuilder, WordComparison};
use crate::{
    THREAD_RECORD_OBJECT_CLASS_OFFSET, THREAD_RECORD_PROFILE_ENABLED_OFFSET,
    THREAD_RECORD_RECURSION_DEPTH_OFFSET, THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET,
    THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET, THREAD_RECORD_TERMINAL_RELEASE_OFFSET,
};

/// A u64 word of the thread record that compiled code reads at entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadRecordWord {
    /// The `inline_refcount::CompiledRefcountAdmission` of this invocation.
    RefcountAdmission,
    /// 1 when the process collects runtime profile counters, else 0.
    ProfileEnabled,
    /// Nonzero while a terminal release runs on the thread.
    TerminalRelease,
    /// The bound runtime's builtin `object` class, boxed.
    ObjectClass,
}

impl ThreadRecordWord {
    pub const fn offset(self) -> i32 {
        match self {
            Self::RefcountAdmission => THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET,
            Self::ProfileEnabled => THREAD_RECORD_PROFILE_ENABLED_OFFSET,
            Self::TerminalRelease => THREAD_RECORD_TERMINAL_RELEASE_OFFSET,
            Self::ObjectClass => THREAD_RECORD_OBJECT_CLASS_OFFSET,
        }
    }
}

/// Backend instruction spelling for thread-record reads.
pub trait ThreadRecordBuilder {
    type Word;

    /// The invocation's attached execution context, its first argument.
    fn attached_context(&mut self) -> Self::Word;
    /// A 64-bit load of `address + offset` that cannot trap and is aligned,
    /// so a backend may delete it when nothing uses the result.
    fn load_trusted_word(&mut self, address: Self::Word, offset: i32) -> Self::Word;
}

/// The thread record the attached context lends.
pub fn generate_thread_record<B: ThreadRecordBuilder>(b: &mut B) -> B::Word {
    let context = b.attached_context();
    b.load_trusted_word(context, ATTACHED_CONTEXT_THREAD_RECORD_OFFSET)
}

/// One thread-record word, read through the attached context.
pub fn generate_thread_record_word<B: ThreadRecordBuilder>(
    b: &mut B,
    word: ThreadRecordWord,
) -> B::Word {
    let record = generate_thread_record(b);
    b.load_trusted_word(record, word.offset())
}

/// Backend spelling for plain word accesses at a fixed offset from an
/// address: the thread record, a frame record, a site record. Every
/// generator that reads or writes such a record extends this trait.
pub trait RecordWordBuilder: WordBuilder {
    fn load_word(&mut self, address: Self::Word, offset: i32) -> Self::Word;
    fn store_word(&mut self, address: Self::Word, offset: i32, value: Self::Word);
    /// A relaxed atomic load: another thread may store the word, and a
    /// backend must neither tear it nor treat the race as undefined.
    fn load_shared_word(&mut self, address: Self::Word, offset: i32) -> Self::Word;
}

// The recursion charge of one Python call, inline, as CPython checks its
// thread state's recursion budget. The depth word is this thread's; the
// limit is the runtime's one value, read through the pointer the runtime
// execution binding publishes, so `sys.setrecursionlimit` on any thread
// reaches every thread's next check. The runtime's own guards
// (molt-runtime `state/recursion.rs`) charge the same depth word.

/// Whether the recursion limit admits one more Python frame on `thread`.
/// Counts nothing; [`generate_recursion_count`] counts the admitted frame.
pub fn generate_recursion_admits<B: RecordWordBuilder>(b: &mut B, thread: B::Word) -> B::Predicate {
    let limit_at = b.load_word(thread, THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET);
    let limit = b.load_shared_word(limit_at, 0);
    let depth = b.load_word(thread, THREAD_RECORD_RECURSION_DEPTH_OFFSET);
    b.compare(WordComparison::UnsignedLessThan, depth, limit)
}

/// Count one Python frame that [`generate_recursion_admits`] admitted.
pub fn generate_recursion_count<B: RecordWordBuilder>(b: &mut B, thread: B::Word) {
    let depth = b.load_word(thread, THREAD_RECORD_RECURSION_DEPTH_OFFSET);
    let deeper = b.add_immediate(depth, 1);
    b.store_word(thread, THREAD_RECORD_RECURSION_DEPTH_OFFSET, deeper);
}

/// Release the frame [`generate_recursion_count`] counted.
pub fn generate_recursion_release<B: RecordWordBuilder>(b: &mut B, thread: B::Word) {
    let depth = b.load_word(thread, THREAD_RECORD_RECURSION_DEPTH_OFFSET);
    let shallower = b.add_immediate(depth, -1);
    b.store_word(thread, THREAD_RECORD_RECURSION_DEPTH_OFFSET, shallower);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the loads a generator asks for, as (base, offset) pairs.
    struct Trace {
        loads: [(&'static str, i32); 2],
        count: usize,
    }

    impl ThreadRecordBuilder for Trace {
        type Word = &'static str;

        fn attached_context(&mut self) -> &'static str {
            "context"
        }

        fn load_trusted_word(&mut self, address: &'static str, offset: i32) -> &'static str {
            self.loads[self.count] = (address, offset);
            self.count += 1;
            if address == "context" {
                "record"
            } else {
                "word"
            }
        }
    }

    /// Word memory: the thread record at `RECORD`, the runtime's limit at
    /// `LIMIT`.
    struct Memory(std::collections::HashMap<u64, u64>);
    const RECORD: u64 = 0x1000;
    const LIMIT: u64 = 0x9000;

    impl Memory {
        fn new(limit: u64) -> Self {
            let mut words = std::collections::HashMap::new();
            words.insert(
                RECORD + THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET as u64,
                LIMIT,
            );
            words.insert(RECORD + THREAD_RECORD_RECURSION_DEPTH_OFFSET as u64, 0);
            words.insert(LIMIT, limit);
            Self(words)
        }
        fn depth(&self) -> u64 {
            self.0[&(RECORD + THREAD_RECORD_RECURSION_DEPTH_OFFSET as u64)]
        }
        /// CPython's call protocol: check, count, run, release.
        fn call(&mut self) -> bool {
            if !generate_recursion_admits(self, RECORD) {
                return false;
            }
            generate_recursion_count(self, RECORD);
            true
        }
    }

    impl WordBuilder for Memory {
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

    impl RecordWordBuilder for Memory {
        fn load_word(&mut self, address: u64, offset: i32) -> u64 {
            *self
                .0
                .get(&address.wrapping_add_signed(offset.into()))
                .expect("generated code read a word it never wrote")
        }
        fn store_word(&mut self, address: u64, offset: i32, value: u64) {
            self.0
                .insert(address.wrapping_add_signed(offset.into()), value);
        }
        fn load_shared_word(&mut self, address: u64, offset: i32) -> u64 {
            self.load_word(address, offset)
        }
    }

    #[test]
    fn the_limit_admits_exactly_limit_frames_and_a_release_returns_one() {
        // CPython: with sys.setrecursionlimit(n), the n-th nested Python
        // frame raises RecursionError; n - 1 run.
        let mut memory = Memory::new(3);
        assert!(memory.call() && memory.call() && memory.call());
        assert_eq!(memory.depth(), 3);
        assert!(!memory.call(), "the fourth frame exceeds a limit of three");
        assert_eq!(memory.depth(), 3, "a refused frame counts nothing");
        generate_recursion_release(&mut memory, RECORD);
        assert_eq!(memory.depth(), 2);
        assert!(memory.call());
    }

    #[test]
    fn a_new_limit_reaches_the_next_check_through_the_runtime_word() {
        let mut memory = Memory::new(1);
        assert!(memory.call());
        assert!(!memory.call());
        // Another thread's sys.setrecursionlimit stores the runtime's word.
        memory.0.insert(LIMIT, 5);
        assert!(memory.call());
        // Lowering the limit below the depth refuses at once (CPython 3.12+
        // raises at the next call).
        memory.0.insert(LIMIT, 1);
        assert!(!memory.call());
    }

    #[test]
    fn a_word_is_the_record_loaded_through_the_context_then_its_offset() {
        for word in [
            ThreadRecordWord::RefcountAdmission,
            ThreadRecordWord::ProfileEnabled,
        ] {
            let mut trace = Trace {
                loads: [("", 0); 2],
                count: 0,
            };
            assert_eq!(generate_thread_record_word(&mut trace, word), "word");
            assert_eq!(trace.count, 2);
            assert_eq!(
                trace.loads,
                [
                    ("context", ATTACHED_CONTEXT_THREAD_RECORD_OFFSET),
                    ("record", word.offset())
                ]
            );
        }
    }
}
