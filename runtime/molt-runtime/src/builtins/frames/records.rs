//! The frame-record chain: the one authority for which Python frames a thread
//! is executing.
//!
//! Each executing frame is one record in the thread's frame arena (the arena
//! that also holds binding homes, [`super::bindings`]), linked to its caller
//! by `prev`. The thread record's `frame_top` word
//! (`molt_codegen_abi::THREAD_RECORD_FRAME_TOP_OFFSET`) names the innermost
//! record, as CPython's `tstate->current_frame` names its
//! `_PyInterpreterFrame`. A record's address is stable until its pop: arena
//! chunks never move.
//!
//! The chain is LIFO: only the top record is pushed or popped. Observers copy
//! a record's [`FrameEntry`] out; they never keep a [`FrameRecord`] across a
//! call that can pop frames.

use std::ptr::NonNull;

use super::FrameEntry;
use super::bindings::{HomeMark, arena_give_back, arena_take_record};

#[repr(C)]
struct Node {
    /// The caller's record address, or zero.
    prev: u64,
    /// Where this record begins in the arena; restored at its pop.
    mark: HomeMark,
    entry: FrameEntry,
}

const NODE_WORDS: usize = std::mem::size_of::<Node>().div_ceil(std::mem::size_of::<u64>());
const _: () = assert!(std::mem::align_of::<Node>() <= std::mem::align_of::<u64>());

/// One linked record. Valid until its pop; `Copy` so callers can walk the
/// chain, never to be stored beyond the current runtime call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameRecord(NonNull<Node>);

fn tls() -> &'static crate::state::tls::ExecutionTls {
    crate::state::tls::execution_tls()
}

impl FrameRecord {
    fn from_word(word: u64) -> Option<Self> {
        NonNull::new(word as usize as *mut Node).map(Self)
    }

    /// The caller's record.
    pub(crate) fn prev(self) -> Option<Self> {
        Self::from_word(unsafe { (*self.0.as_ptr()).prev })
    }

    /// A copy of the record's frame facts. Borrowed edges: the record owns them.
    pub(crate) fn entry(self) -> FrameEntry {
        unsafe { (*self.0.as_ptr()).entry }
    }

    /// Read the record's frame facts in place, without copying the entry.
    /// `read` must not run Python code or touch the chain.
    #[inline]
    pub(crate) fn read<R>(self, read: impl FnOnce(&FrameEntry) -> R) -> R {
        read(unsafe { &(*self.0.as_ptr()).entry })
    }

    /// Change the record's frame facts in place. `change` must not run Python
    /// code or touch the chain.
    pub(crate) fn update<R>(self, change: impl FnOnce(&mut FrameEntry) -> R) -> R {
        change(unsafe { &mut (*self.0.as_ptr()).entry })
    }
}

/// The executing frame's record.
pub(crate) fn top() -> Option<FrameRecord> {
    FrameRecord::from_word(tls().frame_top.get())
}

/// How many records the chain holds.
pub(crate) fn depth() -> usize {
    tls().frame_depth.get()
}

/// The chain, innermost first.
pub(crate) fn innermost_first() -> impl Iterator<Item = FrameRecord> {
    std::iter::successors(top(), |record| record.prev())
}

/// The record `index` frames above the outermost one (the outermost is 0).
pub(crate) fn at(index: usize) -> Option<FrameRecord> {
    let skip = depth().checked_sub(index)?.checked_sub(1)?;
    innermost_first().nth(skip)
}

/// Copies of the records in `range` (outermost is 0), outermost first.
/// `None`: the chain is shorter than the range.
pub(crate) fn entries(range: std::ops::Range<usize>) -> Option<Vec<FrameEntry>> {
    let depth = depth();
    if range.end > depth {
        return None;
    }
    let mut entries: Vec<FrameEntry> = innermost_first()
        .skip(depth - range.end)
        .take(range.len())
        .map(FrameRecord::entry)
        .collect();
    entries.reverse();
    Some(entries)
}

/// Link the frame `entry` builds as the executing frame, with `extra`
/// uninitialized words after its record (its location word and homes), in one
/// arena transaction. `entry` receives the address of those words. `None`:
/// the arena could not grow or the thread's storage is being destroyed;
/// nothing was linked and `entry` did not run.
pub(crate) fn push(
    extra: usize,
    entry: impl FnOnce(*mut u64) -> FrameEntry,
) -> Option<FrameRecord> {
    let (words, mark) = arena_take_record(NODE_WORDS.checked_add(extra)?)?;
    let node = words.cast::<Node>();
    let tls = tls();
    unsafe {
        node.write(Node {
            prev: tls.frame_top.get(),
            mark,
            entry: entry(words.add(NODE_WORDS)),
        });
    }
    tls.frame_top.set(node as usize as u64);
    tls.frame_depth.set(tls.frame_depth.get() + 1);
    FrameRecord::from_word(node as usize as u64)
}

/// Storage of a record that [`unlink`] removed; released by [`Unlinked::free`]
/// once the frame's exit finished, so its homes and record stay reserved while
/// releases run.
#[must_use]
pub(crate) struct Unlinked {
    pub(crate) entry: FrameEntry,
    mark: HomeMark,
}

impl Unlinked {
    pub(crate) fn free(self) {
        arena_give_back(self.mark);
    }
}

/// Unlink the executing frame, so everything its exit runs sees the caller.
pub(crate) fn unlink() -> Option<Unlinked> {
    let record = top()?;
    let tls = tls();
    let node = unsafe { record.0.as_ptr().read() };
    tls.frame_top.set(node.prev);
    tls.frame_depth.set(tls.frame_depth.get() - 1);
    Some(Unlinked {
        entry: node.entry,
        mark: node.mark,
    })
}

/// The frames of a thread that no longer runs them, copied out and unlinked.
pub(crate) struct Abandoned {
    /// Innermost first: the order their exits run in.
    pub(crate) entries: Vec<FrameEntry>,
    outermost: Option<HomeMark>,
}

impl Abandoned {
    /// Return the records' storage, after every entry's exit finished.
    pub(crate) fn free(self) {
        if let Some(mark) = self.outermost {
            arena_give_back(mark);
        }
    }
}

/// Unlink every record of this thread. No record is dereferenced afterwards.
pub(crate) fn abandon_all() -> Abandoned {
    let entries = innermost_first().map(FrameRecord::entry).collect();
    let outermost = innermost_first()
        .last()
        .map(|record| unsafe { (*record.0.as_ptr()).mark });
    let tls = tls();
    tls.frame_top.set(0);
    tls.frame_depth.set(0);
    Abandoned { entries, outermost }
}

#[cfg(test)]
mod tests {
    use super::super::{FramePlan, frame_stack_enter, frame_stack_pop, frame_stack_push_owned};
    use super::*;

    /// A runtime context frame: no code, so it owns no edge.
    fn push(py: &crate::PyToken<'_>) {
        frame_stack_push_owned(py, 0, 0, 0, 0);
    }

    #[test]
    fn the_chain_is_lifo_and_indexes_from_the_outermost_record() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = depth();
            let outer_top = top();
            // Homes taken with a record must not disturb the chain.
            assert!(frame_stack_enter(
                py,
                0,
                0,
                0,
                0,
                FramePlan {
                    homes: 3,
                    ..FramePlan::default()
                }
            ));
            let first = top().unwrap();
            push(py);
            let second = top().unwrap();
            assert_eq!(depth(), base + 2);
            assert!(second != first);
            assert!(second.prev() == Some(first));
            assert!(first.prev() == outer_top);
            assert!(at(base) == Some(first));
            assert!(at(base + 1) == Some(second));
            assert!(at(base + 2).is_none());
            assert_eq!(entries(base..base + 2).unwrap().len(), 2);
            assert!(entries(base..base + 3).is_none());
            let walked: Vec<_> = innermost_first().take(2).collect();
            assert!(walked == [second, first]);

            frame_stack_pop(py);
            assert!(top() == Some(first));
            // The popped record's storage is reused by the next push.
            push(py);
            assert!(top() == Some(second));
            frame_stack_pop(py);
            frame_stack_pop(py);
            assert_eq!(depth(), base);
            assert!(top() == outer_top);
        });
    }

    #[test]
    fn a_record_address_survives_growth_of_the_arena() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = depth();
            push(py);
            let first = top().unwrap();
            first.update(|entry| entry.location_snapshot = None);
            // More homes than one arena chunk holds forces a fresh chunk.
            for _ in 0..40 {
                assert!(frame_stack_enter(
                    py,
                    0,
                    0,
                    0,
                    0,
                    FramePlan {
                        homes: 1000,
                        ..FramePlan::default()
                    }
                ));
            }
            assert!(at(base) == Some(first));
            assert_eq!(first.entry().globals_bits, 0);
            for _ in 0..41 {
                frame_stack_pop(py);
            }
            assert_eq!(depth(), base);
        });
    }
}
