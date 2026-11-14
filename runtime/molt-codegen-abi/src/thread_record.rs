//! Words of the thread record that compiled code reads with plain loads, as
//! CPython's code reads its `PyThreadState`.
//!
//! One generator owns the address chain: the invocation's attached execution
//! context lends the thread record (`ATTACHED_CONTEXT_THREAD_RECORD_OFFSET`),
//! and each word sits at a fixed record offset that the runtime asserts
//! (molt-runtime `state/tls.rs`). A backend spells the two operations only.

use crate::attached_context_generated::ATTACHED_CONTEXT_THREAD_RECORD_OFFSET;
use crate::{THREAD_RECORD_PROFILE_ENABLED_OFFSET, THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET};

/// A u64 word of the thread record that compiled code reads at entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadRecordWord {
    /// The `inline_refcount::CompiledRefcountAdmission` of this invocation.
    RefcountAdmission,
    /// 1 when the process collects runtime profile counters, else 0.
    ProfileEnabled,
}

impl ThreadRecordWord {
    pub const fn offset(self) -> i32 {
        match self {
            Self::RefcountAdmission => THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET,
            Self::ProfileEnabled => THREAD_RECORD_PROFILE_ENABLED_OFFSET,
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
