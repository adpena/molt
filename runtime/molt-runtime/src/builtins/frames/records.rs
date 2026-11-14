//! The frame-record chain: the one authority for which Python frames a thread
//! is executing, and the arena that holds it.
//!
//! Each executing frame is one record in the thread's frame arena: six header
//! words (`molt_codegen_abi::FRAME_RECORD_*`), then the frame's binding homes
//! ([`super::bindings`]). A record is named by its *base*, the address of its
//! first home; the header sits below it. The thread record's `frame_top` word
//! names the innermost record and each record's `prev` names its caller, as
//! CPython's `tstate->current_frame` and `_PyInterpreterFrame.previous` do.
//!
//! Two kinds of record share the layout:
//! * a **compiled** record, pushed and popped inline by a synchronous
//!   function's generated code
//!   (`molt_codegen_abi::frame_record::generate_frame_push`). It owns one
//!   reference to its function and one to its code object; globals and
//!   builtins are read from the function;
//! * an **owned** record (`FRAME_RECORD_OWNED`), linked by the runtime for
//!   every other frame: a module or class body, a stateful activation, a
//!   runtime-implemented Python body, a compiled entry no invoker handed a
//!   function. Its `function` word addresses a namespace block in the arena,
//!   directly below the header, that holds the frame's [`FrameEntry`].
//!
//! Every reader goes through [`FrameRecord`], which checks the kind first.
//!
//! The arena is a stack of chunks. The thread record's `arena_top` and
//! `arena_limit` words describe the current chunk, so an inline push is a
//! load, a compare and a store. The chunk list is runtime-owned: a record
//! that opens a chunk carries `FRAME_RECORD_CHUNK_FIRST` and its pop returns
//! to the previous chunk at the top saved here. Chunks are kept until the
//! thread's storage is cleared, so a call loop on a chunk edge allocates
//! nothing.
//!
//! The chain is LIFO: only the top record is pushed or popped. A record's
//! address is stable until its pop. Observers copy a record's [`FrameEntry`]
//! out; they never keep a [`FrameRecord`] across a call that can pop frames.

use std::cell::RefCell;
use std::ptr::NonNull;

use molt_codegen_abi::{
    FRAME_HOME_WORDS, FRAME_RECORD_CHUNK_FIRST, FRAME_RECORD_CODE_OFFSET,
    FRAME_RECORD_FLAGS_OFFSET, FRAME_RECORD_FUNCTION_OFFSET, FRAME_RECORD_HEADER_BYTES,
    FRAME_RECORD_OBSERVED, FRAME_RECORD_OBSERVER_OFFSET, FRAME_RECORD_OWNED,
    FRAME_RECORD_PREV_OFFSET,
};

use super::bindings::FrameBindings;
use super::locations::LocationId;
use super::{FrameEntry, FramePlan, PythonArgumentZero, PythonFrameContext};
use crate::object::layout::{code_argcount, function_builtins_bits, function_globals_bits};
use crate::{PyToken, dec_ref_bits, obj_from_bits};

const WORD: usize = std::mem::size_of::<u64>();
const HEADER_WORDS: usize = FRAME_RECORD_HEADER_BYTES as usize / WORD;
/// Arena words per chunk. A frame larger than a chunk gets its own chunk.
const CHUNK_WORDS: usize = 1 << 14;

/// The namespace block of an owned record.
#[repr(C)]
struct Owned {
    /// The frame's facts. Its `bindings.payload_bits` is unused: the record's
    /// observer word owns the payload.
    entry: FrameEntry,
}

// The layout facts the inline push reads through `molt-codegen-abi`.
const _: () = {
    assert!(molt_codegen_abi::FUNCTION_CODE_OFFSET as usize == 4 * WORD);
    assert!(molt_codegen_abi::CODE_FRAME_SLOT_OFFSET as usize == 17 * WORD);
    assert!(molt_codegen_abi::frame_record::FRAME_LOCATION_ABSENT == LocationId::ABSENT.word());
    assert!(molt_codegen_abi::FRAME_LOCATION_OFFSET == -(WORD as i32));
    assert!(FRAME_RECORD_PREV_OFFSET as i64 == -FRAME_RECORD_HEADER_BYTES);
};

const OWNED_WORDS: usize = std::mem::size_of::<Owned>().div_ceil(WORD);
const _: () = assert!(std::mem::align_of::<Owned>() <= std::mem::align_of::<u64>());

fn tls() -> &'static crate::state::tls::ExecutionTls {
    crate::state::tls::execution_tls()
}

struct Chunks {
    chunks: Vec<Box<[u64]>>,
    /// The chunk `arena_top` points into; meaningful once `chunks` is not empty.
    current: usize,
    /// Per chunk, the `arena_top` of the chunk below it when it was opened.
    resume: Vec<u64>,
}

thread_local! {
    static CHUNKS: std::mem::ManuallyDrop<RefCell<Chunks>> = const {
        std::mem::ManuallyDrop::new(RefCell::new(Chunks {
            chunks: Vec::new(),
            current: 0,
            resume: Vec::new(),
        }))
    };
}

/// Take `words` arena words whose contents the caller overwrites in full.
/// The flag says the words open a chunk above the first. `None`: the arena
/// could not grow, or the thread's storage is being destroyed.
fn arena_take(words: usize) -> Option<(*mut u64, bool)> {
    let tls = tls();
    let bytes = u64::try_from(words.checked_mul(WORD)?).ok()?;
    let top = tls.arena_top.get();
    if tls.arena_limit.get() - top >= bytes {
        tls.arena_top.set(top + bytes);
        return Some((crate::provenance::abi::mut_ptr::<u64>(top)?, false));
    }
    CHUNKS
        .try_with(|arena| {
            let mut arena = arena.borrow_mut();
            let next = if arena.chunks.is_empty() {
                0
            } else {
                arena.current + 1
            };
            // Chunks above the current one hold no live record.
            if arena
                .chunks
                .get(next)
                .is_none_or(|chunk| chunk.len() < words)
            {
                let len = words.max(CHUNK_WORDS);
                let mut storage = Vec::new();
                storage.try_reserve_exact(len).ok()?;
                storage.resize(len, 0);
                let chunk = storage.into_boxed_slice();
                if next < arena.chunks.len() {
                    arena.chunks[next] = chunk;
                } else {
                    arena.chunks.try_reserve(1).ok()?;
                    arena.resume.try_reserve(1).ok()?;
                    arena.chunks.push(chunk);
                    arena.resume.push(0);
                }
            }
            arena.resume[next] = top;
            arena.current = next;
            let chunk = &mut arena.chunks[next];
            let start = chunk.as_mut_ptr();
            tls.arena_top.set(start as usize as u64 + bytes);
            tls.arena_limit
                .set(start as usize as u64 + (chunk.len() * WORD) as u64);
            // The first chunk is the baseline: a record at its start pops like
            // any other.
            Some((start, next != 0))
        })
        .ok()
        .flatten()
}

/// Return everything taken since the take that began at `start`.
fn arena_give_back(start: u64, chunk_first: bool) {
    let tls = tls();
    if !chunk_first {
        tls.arena_top.set(start);
        return;
    }
    let _ = CHUNKS.try_with(|arena| {
        let mut arena = arena.borrow_mut();
        let opened = arena.current;
        debug_assert!(opened > 0, "the first arena chunk is never chunk-first");
        arena.current = opened - 1;
        let below = &arena.chunks[opened - 1];
        tls.arena_top.set(arena.resume[opened]);
        tls.arena_limit
            .set(below.as_ptr() as usize as u64 + (below.len() * WORD) as u64);
    });
}

/// Initialize permanent storage; the attached thread-state drain retires its
/// allocation after the final frame exits, including late native callbacks.
pub(crate) fn touch_frame_home_tls_lifetime() {
    let _ = CHUNKS.try_with(|_| {});
}

pub(crate) fn clear_frame_home_tls_storage() {
    let tls = tls();
    assert_eq!(
        tls.frame_top.get(),
        0,
        "retiring the arena of a live compiled frame"
    );
    CHUNKS.with(|arena| {
        let mut arena = arena.borrow_mut();
        arena.chunks = Vec::new();
        arena.resume = Vec::new();
        arena.current = 0;
    });
    tls.arena_top.set(0);
    tls.arena_limit.set(0);
}

/// One linked record, named by its base. Valid until its pop; `Copy` so
/// callers can walk the chain, never to be stored beyond the current runtime
/// call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameRecord(NonNull<u64>);

impl FrameRecord {
    fn from_word(word: u64) -> Option<Self> {
        NonNull::new(crate::provenance::abi::mut_ptr::<u64>(word)?).map(Self)
    }

    #[inline]
    fn header(self, offset: i32) -> *mut u64 {
        unsafe { self.0.as_ptr().byte_offset(offset as isize) }
    }

    /// The record's base: the address of its first home, which compiled code
    /// and a frame's observers name it by.
    #[inline]
    pub(crate) fn base(self) -> usize {
        self.0.as_ptr() as usize
    }

    /// The caller's record.
    pub(crate) fn prev(self) -> Option<Self> {
        Self::from_word(unsafe { *self.header(FRAME_RECORD_PREV_OFFSET) })
    }

    #[inline]
    fn flags(self) -> u64 {
        unsafe { *self.header(FRAME_RECORD_FLAGS_OFFSET) }
    }

    /// The namespace block of an owned record: the one place the record kind
    /// is decided.
    #[inline]
    fn owned(self) -> Option<*mut Owned> {
        if self.flags() & FRAME_RECORD_OWNED == 0 {
            return None;
        }
        crate::provenance::abi::mut_ptr::<Owned>(unsafe {
            *self.header(FRAME_RECORD_FUNCTION_OFFSET)
        })
    }

    /// The function object of a compiled record.
    #[inline]
    fn function(self) -> *mut u8 {
        debug_assert!(self.flags() & FRAME_RECORD_OWNED == 0);
        obj_from_bits(unsafe { *self.header(FRAME_RECORD_FUNCTION_OFFSET) })
            .as_ptr()
            .expect("a compiled frame record holds its function")
    }

    /// The frame's code object, borrowed from the record.
    #[inline]
    pub(crate) fn code_bits(self) -> u64 {
        unsafe { *self.header(FRAME_RECORD_CODE_OFFSET) }
    }

    /// Whether the frame runs a class body.
    pub(crate) fn is_class_body(self) -> bool {
        FrameEntry::code_is_class_body(self.code_bits())
    }

    /// The frame's globals and builtins, borrowed.
    #[inline]
    pub(crate) fn namespaces(self) -> (u64, u64) {
        match self.owned() {
            Some(owned) => unsafe { ((*owned).entry.globals_bits, (*owned).entry.builtins_bits) },
            None => {
                let function = self.function();
                unsafe {
                    (
                        function_globals_bits(function),
                        function_builtins_bits(function),
                    )
                }
            }
        }
    }

    /// The stateful activation the frame runs, or zero.
    #[inline]
    pub(crate) fn activation_bits(self) -> u64 {
        self.owned()
            .map_or(0, |owned| unsafe { (*owned).entry.activation_bits })
    }

    /// The payload the frame's observers share, borrowed, or zero.
    #[inline]
    pub(crate) fn observer(self) -> u64 {
        unsafe { *self.header(FRAME_RECORD_OBSERVER_OFFSET) }
    }

    /// Give the record its observers' payload, whose reference it takes.
    /// `false`: it already has one and takes nothing.
    pub(crate) fn attach_observer(self, payload: u64) -> bool {
        if self.observer() != 0 {
            return false;
        }
        unsafe {
            *self.header(FRAME_RECORD_OBSERVER_OFFSET) = payload;
            *self.header(FRAME_RECORD_FLAGS_OFFSET) |= FRAME_RECORD_OBSERVED;
        }
        true
    }

    /// The frame's binding homes and location word, without its payload.
    pub(crate) fn bindings(self) -> FrameBindings {
        let mut bindings = match self.owned() {
            Some(owned) => unsafe { (*owned).entry.bindings },
            None => FrameBindings::over(compiled_plan(self.code_bits()), self.base()),
        };
        bindings.payload_bits = self.observer();
        bindings
    }

    /// The lexical context zero-argument `super()` reads. A frame with homes
    /// publishes none: as CPython's `super_init_without_args`, "no arguments"
    /// is `co_argcount == 0`, argument zero is home 0 and the class cell is
    /// the home of the code's `__class__` free variable. Any other frame
    /// carries the context its code published.
    pub(crate) fn python_context(self) -> PythonFrameContext {
        let bindings = self.bindings();
        if !bindings.has_python_bindings() {
            return self
                .owned()
                .map_or_else(PythonFrameContext::default, |owned| unsafe {
                    (*owned).entry.python_context
                });
        }
        let code_bits = self.code_bits();
        let no_arguments = obj_from_bits(code_bits)
            .as_ptr()
            .is_none_or(|code| unsafe { code_argcount(code) } == 0);
        PythonFrameContext {
            argument_zero: if no_arguments {
                PythonArgumentZero::NoArgument
            } else {
                PythonArgumentZero::Home
            },
            class_cell_bits: super::bindings::frame_class_cell(&bindings, code_bits),
        }
    }

    /// A copy of the frame's facts, every edge borrowed from the record.
    pub(crate) fn entry(self) -> FrameEntry {
        self.entry_with(self.bindings())
    }

    /// [`Self::entry`] with `bindings` in place of the record's own: a copy
    /// that shares a captured payload needs no frame plan.
    pub(crate) fn entry_with(self, bindings: FrameBindings) -> FrameEntry {
        match self.owned() {
            Some(owned) => FrameEntry {
                bindings,
                ..unsafe { (*owned).entry }
            },
            None => {
                let (globals_bits, builtins_bits) = self.namespaces();
                FrameEntry {
                    code_bits: self.code_bits(),
                    location_snapshot: None,
                    locals_bits: 0,
                    activation_bits: 0,
                    globals_bits,
                    builtins_bits,
                    python_context: PythonFrameContext::default(),
                    bindings,
                }
            }
        }
    }

    /// Change the facts of a frame the runtime linked. `None`: the record is
    /// compiled and has no namespace block. `change` must not run Python code
    /// or touch the chain; the entry's payload field is not the record's.
    pub(crate) fn update_owned<R>(self, change: impl FnOnce(&mut FrameEntry) -> R) -> Option<R> {
        self.owned()
            .map(|owned| change(unsafe { &mut (*owned).entry }))
    }
}

/// The frame plan of a compiled record's code: its compiled slot's.
fn compiled_plan(code_bits: u64) -> FramePlan {
    super::compiled_slot_for_code(code_bits)
        .and_then(|slot| {
            crate::state::runtime_state::runtime_state_for_gil()?
                .code_slots
                .get()?
                .get(usize::try_from(slot).ok()?)
        })
        .map(super::CompiledCodeSlot::frame_plan)
        .unwrap_or_default()
}

/// The executing frame's record.
#[inline]
pub(crate) fn top() -> Option<FrameRecord> {
    FrameRecord::from_word(tls().frame_top.get())
}

/// The chain, innermost first.
pub(crate) fn innermost_first() -> impl Iterator<Item = FrameRecord> {
    std::iter::successors(top(), |record| record.prev())
}

/// How many records the chain holds. Walks the chain: compiled pushes keep no
/// count.
pub(crate) fn depth() -> usize {
    innermost_first().count()
}

/// The record `index` frames above the outermost one (the outermost is 0).
#[cfg(test)]
pub(crate) fn at(index: usize) -> Option<FrameRecord> {
    let skip = depth().checked_sub(index)?.checked_sub(1)?;
    innermost_first().nth(skip)
}

/// Link an owned record with `homes` unbound homes as the executing frame,
/// in one arena transaction. `entry` receives the record's base and returns
/// the frame's facts, whose edges the record takes; a payload in its bindings
/// becomes the record's observer. `None`: the arena could not grow or the
/// thread's storage is being destroyed; nothing was linked and `entry` did
/// not run.
pub(crate) fn push(homes: usize, entry: impl FnOnce(usize) -> FrameEntry) -> Option<FrameRecord> {
    let home_words = homes.checked_mul(FRAME_HOME_WORDS)?;
    let words = OWNED_WORDS
        .checked_add(HEADER_WORDS)?
        .checked_add(home_words)?;
    let (start, chunk_first) = arena_take(words)?;
    let tls = tls();
    let record = unsafe {
        let header = start.add(OWNED_WORDS);
        let base = header.add(HEADER_WORDS);
        // Location first: `entry` may restore a resumed activation's.
        header
            .add(HEADER_WORDS - 1)
            .write(LocationId::ABSENT.word());
        std::ptr::write_bytes(base, 0, home_words);
        let mut entry = entry(base as usize);
        let payload = std::mem::take(&mut entry.bindings.payload_bits);
        let mut flags = FRAME_RECORD_OWNED;
        if chunk_first {
            flags |= FRAME_RECORD_CHUNK_FIRST;
        }
        if payload != 0 {
            flags |= FRAME_RECORD_OBSERVED;
        }
        let record = FrameRecord(NonNull::new_unchecked(base));
        *record.header(FRAME_RECORD_PREV_OFFSET) = tls.frame_top.get();
        *record.header(FRAME_RECORD_FUNCTION_OFFSET) = start as usize as u64;
        *record.header(FRAME_RECORD_CODE_OFFSET) = entry.code_bits;
        *record.header(FRAME_RECORD_FLAGS_OFFSET) = flags;
        *record.header(FRAME_RECORD_OBSERVER_OFFSET) = payload;
        start.cast::<Owned>().write(Owned { entry });
        record
    };
    tls.frame_top.set(record.base() as u64);
    Some(record)
}

/// What an unlinked record owned.
enum Edges {
    /// A runtime-linked frame: every edge of its entry.
    Owned(FrameEntry),
    /// A compiled frame: its function, its code, its homes and its payload.
    Compiled {
        function: u64,
        code: u64,
        bindings: FrameBindings,
    },
}

/// A record that [`unlink`] removed. Its storage stays reserved until
/// [`Unlinked::exit`] finished the frame's releases.
#[must_use]
pub(crate) struct Unlinked {
    edges: Edges,
    start: u64,
    chunk_first: bool,
}

impl Unlinked {
    fn of(record: FrameRecord) -> Self {
        let flags = record.flags();
        let (edges, start) = match record.owned() {
            Some(owned) => (Edges::Owned(record.entry()), owned as usize as u64),
            None => (
                Edges::Compiled {
                    function: unsafe { *record.header(FRAME_RECORD_FUNCTION_OFFSET) },
                    code: record.code_bits(),
                    bindings: record.bindings(),
                },
                record.base() as u64 - FRAME_RECORD_HEADER_BYTES as u64,
            ),
        };
        Self {
            edges,
            start,
            chunk_first: flags & FRAME_RECORD_CHUNK_FIRST != 0,
        }
    }

    /// Run the frame's exit: the bindings leave the homes (a shared payload
    /// takes them over), then the record's own edges are released.
    fn release(&self, py: &PyToken<'_>) {
        match self.edges {
            Edges::Owned(entry) => entry.exit(py),
            Edges::Compiled {
                function,
                code,
                bindings,
            } => {
                super::bindings::frame_bindings_exit(py, &bindings);
                for bits in [bindings.payload_bits, function, code] {
                    if bits != 0 {
                        dec_ref_bits(py, bits);
                    }
                }
            }
        }
    }

    /// Exit the frame, then return its storage.
    pub(crate) fn exit(self, py: &PyToken<'_>) {
        self.release(py);
        arena_give_back(self.start, self.chunk_first);
    }
}

/// Unlink the executing frame, so everything its exit runs sees the caller.
pub(crate) fn unlink() -> Option<Unlinked> {
    let record = top()?;
    let unlinked = Unlinked::of(record);
    tls()
        .frame_top
        .set(record.prev().map_or(0, |prev| prev.base() as u64));
    Some(unlinked)
}

/// The frames of a thread that no longer runs them, copied out and unlinked.
pub(crate) struct Abandoned(Vec<Unlinked>);

impl Abandoned {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Exit every frame, innermost first, then return the arena to its first
    /// chunk.
    pub(crate) fn exit(self, py: &PyToken<'_>) {
        for frame in &self.0 {
            frame.release(py);
        }
        // Innermost first, so each chunk-first record returns to the chunk
        // below it in turn.
        for frame in self.0 {
            arena_give_back(frame.start, frame.chunk_first);
        }
    }
}

/// Unlink every record of this thread. No record is dereferenced afterwards;
/// the homes stay reserved until [`Abandoned::exit`].
pub(crate) fn abandon_all() -> Abandoned {
    let frames = innermost_first().map(Unlinked::of).collect();
    tls().frame_top.set(0);
    Abandoned(frames)
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
    fn a_record_survives_arena_growth_and_its_pop_returns_to_its_chunk() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = depth();
            push(py);
            let first = top().unwrap();
            let top_before = tls().arena_top.get();
            let limit_before = tls().arena_limit.get();
            // More homes than one arena chunk holds forces fresh chunks.
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
            assert!(tls().arena_limit.get() != limit_before);
            assert!(at(base) == Some(first));
            assert_eq!(first.entry().globals_bits, 0);
            for _ in 0..40 {
                frame_stack_pop(py);
            }
            // The pops walked back through every chunk to the first record's.
            assert_eq!(tls().arena_top.get(), top_before);
            assert_eq!(tls().arena_limit.get(), limit_before);
            assert!(top() == Some(first));
            frame_stack_pop(py);
            assert_eq!(depth(), base);
        });
    }
}
