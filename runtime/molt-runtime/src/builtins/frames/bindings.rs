//! Binding storage of Python activations and the frame-object state that
//! observes it.
//!
//! A synchronous Python activation's *homes* own its bindings: two words per
//! code slot, a storage kind and its bits (`molt_codegen_abi::FRAME_HOME_*`),
//! indexed by the code object's localsplus layout ([`code_slots`]):
//! * `PLAIN` owns one reference to the bound object and `RAW_INT` holds an
//!   integer with no reference; either is a CPython plain local;
//! * `CELL` owns a reference to the frame's cell of a variable that closures
//!   capture (or of a free variable); the binding is the cell's contents;
//! * `PRIVATE_CELL` owns a reference to a cell the compiler keeps for a plain
//!   local; the binding is its contents, with plain-local proxy semantics;
//! * `UNBOUND` holds nothing.
//!
//! `molt_trace_enter_slot` takes the homes of an optimized code object's
//! synchronous activation from a thread-local arena, zeroed, as its code
//! slot's [`FramePlan`] says; the frame's exit returns them. Frames nest, so
//! the arena is a stack. Compiled code borrows the base address from
//! `molt_frame_homes` only once the entry succeeded. Compiled code owns
//! nothing a home holds: a store hands its operand's reference to the home,
//! publishes the new pair, then releases the displaced reference (CPython's
//! STORE_FAST); `del` releases it; a read borrows it while no write can
//! intervene, otherwise it reads the home again; a PEP 709 comprehension takes
//! the enclosing binding out and stores it back.
//!
//! A stateful activation (generator, coroutine) keeps its bindings in its task
//! payload, which `activation.rs` projects through the compiler's registered
//! layout.
//!
//! An observer (traceback capture, `sys._getframe`, `gi_frame`, `locals()`)
//! never copies bindings eagerly. It shares the activation's `FRAME_BINDINGS`
//! payload, the state of CPython's frame object:
//! * live over homes: it reads and writes the running frame's homes. Any
//!   thread holding the runtime GIL may do so; the owning thread touches its
//!   homes only while it holds the GIL too;
//! * live over an activation: it reads and writes the task payload;
//! * retired: it owns every binding in its own slot pairs.
//!
//! The payload also owns what only the frame object owns: before PEP 667 the
//! activation's one `f_locals` dict, afterwards the extra locals a proxy
//! wrote, and from 3.14 the plain values proxy writes displaced (CPython's
//! overwritten fast locals).
//!
//! An exiting activation is unlinked first, so every finalizer its releases
//! run sees the caller as the executing frame. A payload an observer shares
//! then takes the bindings over (CPython's `take_ownership`); otherwise they
//! are released in the target's clear order, after an unshared 3.13+ frame
//! object's own extra locals and overwritten values. The homes stay reserved
//! until those releases finish. Runtime teardown exits every abandoned entry
//! the same way. A retired payload releases its bindings when `frame.clear()`
//! runs (CPython `frame_tp_clear` order) or when it dies (`frame_dealloc`
//! order); the two orders differ.

use std::cell::RefCell;

use molt_codegen_abi::{
    FRAME_HOME_CELL, FRAME_HOME_HOLDS_REFERENCE, FRAME_HOME_PLAIN, FRAME_HOME_PRIVATE_CELL,
    FRAME_HOME_RAW_INT, FRAME_HOME_UNBOUND, FRAME_HOME_WORDS,
};
use molt_obj_model::MoltObject;

use crate::builtins::exceptions::raise_key_error_with_key;
use crate::object::HEADER_FLAG_IMMORTAL;
use crate::object::cells::{cell_detach_value, cell_ptr_from_bits, cell_value_bits};
use crate::object::code_layout::code_slots;
use crate::object::layout::{CO_OPTIMIZED, code_flags};
use crate::{
    FRAME_STACK, MoltHeader, PyToken, TYPE_ID_DICT, TYPE_ID_FRAME_BINDINGS, TYPE_ID_STRING,
    alloc_dict_with_pairs, alloc_object, dec_ref_bits, dict_del_in_place, dict_get_in_place,
    dict_set_in_place, exception_pending, header_from_obj_ptr, inc_ref_bits, is_missing_bits,
    missing_bits, obj_from_bits, object_mark_has_ptrs, object_type_id, raise_exception,
};

const WORD: usize = std::mem::size_of::<u64>();

// Payload words.
const CODE_WORD: usize = 0;
const STATE_WORD: usize = 1;
/// The code object's localsplus size; the payload has a slot pair for each.
const COUNT_WORD: usize = 2;
/// The live source: the homes' address, or the task (borrowed: the task owns
/// the payload and retires or detaches it before it dies). Zero otherwise.
const SOURCE_WORD: usize = 3;
/// Before PEP 667 the activation's one `f_locals` dict; from 3.13 the dict of
/// the names a proxy wrote that are not code slots. Owned, or zero.
const LOCALS_WORD: usize = 4;
/// From 3.14, a heap `Vec<u64>` of the plain values proxy writes displaced,
/// oldest first, each owned. Zero while empty.
const HISTORY_WORD: usize = 5;
const SLOT_BASE_WORD: usize = 6;

/// The bindings are the running frame's homes.
const STATE_LIVE_HOMES: u64 = 1 << 0;
/// The bindings are a stateful activation's task payload.
const STATE_LIVE_ACTIVATION: u64 = 1 << 1;
/// The payload's own slot pairs own the bindings.
const STATE_RETIRED: u64 = 1 << 2;
/// Cycle collection, destruction or runtime teardown detached everything.
const STATE_DETACHED: u64 = 1 << 3;
const STATE_SOURCE_MASK: u64 =
    STATE_LIVE_HOMES | STATE_LIVE_ACTIVATION | STATE_RETIRED | STATE_DETACHED;
const STATE_POLICY_SHIFT: u32 = 8;

/// A retired slot whose home held no valid kind when it retired. Never an
/// ABI kind; an observer reports it as `SystemError`.
const RETIRED_MALFORMED: i64 = -1;

/// Arena words per chunk. A frame larger than a chunk gets its own chunk.
const HOME_CHUNK_WORDS: usize = 1 << 14;

/// The target's frame-object semantics. Derived once per compiled code object
/// (`CompiledCodeSlot`) and carried by each activation and payload, never
/// re-read from the target version at a call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FramePolicy(u8);

impl FramePolicy {
    /// `f_locals` is a write-through proxy and `locals()` a snapshot (3.13+).
    const PEP667: u8 = 1 << 0;
    /// A frame clears its slots last to first (3.14+).
    const DESCENDING: u8 = 1 << 1;
    /// A proxy write keeps the plain value it displaces (3.14+).
    const KEEPS_HISTORY: u8 = 1 << 2;
    /// Closing a created generator or coroutine releases its frame storage
    /// (3.14+); earlier targets keep it until the task dies.
    const RELEASES_UNSTARTED: u8 = 1 << 3;
    const MASK: u8 =
        Self::PEP667 | Self::DESCENDING | Self::KEEPS_HISTORY | Self::RELEASES_UNSTARTED;

    pub(crate) fn for_minor(minor: i64) -> Self {
        let mut bits = 0;
        if minor >= 13 {
            bits |= Self::PEP667;
        }
        if minor >= 14 {
            bits |= Self::DESCENDING | Self::KEEPS_HISTORY | Self::RELEASES_UNSTARTED;
        }
        Self(bits)
    }

    /// Reads the runtime target version: code publication and first
    /// observation of code without a compiled slot only, never a call.
    pub(crate) fn of_runtime(py: &PyToken<'_>) -> Self {
        Self::for_minor(crate::object::ops_sys::runtime_target_minor(py))
    }

    pub(crate) fn bits(self) -> u64 {
        u64::from(self.0)
    }

    pub(crate) fn from_bits(bits: u64) -> Self {
        Self((bits as u8) & Self::MASK)
    }

    pub(crate) fn pep667(self) -> bool {
        self.0 & Self::PEP667 != 0
    }

    pub(crate) fn descending(self) -> bool {
        self.0 & Self::DESCENDING != 0
    }

    fn keeps_history(self) -> bool {
        self.0 & Self::KEEPS_HISTORY != 0
    }

    pub(crate) fn releases_unstarted(self) -> bool {
        self.0 & Self::RELEASES_UNSTARTED != 0
    }

    /// Visit `count` slot indices in this target's frame-clear order.
    pub(crate) fn for_each_slot(self, count: usize, visit: impl FnMut(usize)) {
        if self.descending() {
            (0..count).rev().for_each(visit);
        } else {
            (0..count).for_each(visit);
        }
    }
}

/// What a compiled code slot's synchronous activations take at frame entry,
/// derived once when the slot is published. Only an optimized code object
/// (CPython `CO_OPTIMIZED`) keeps its bindings in homes, one per localsplus
/// slot, even when it has none; a module body binds a namespace instead. The
/// default plan (an unpublished slot) takes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FramePlan {
    pub(crate) optimized: bool,
    pub(crate) homes: u32,
    pub(crate) policy: FramePolicy,
}

impl FramePlan {
    const OPTIMIZED: u64 = 1 << 63;
    const POLICY_SHIFT: u32 = 32;

    /// The plan of `code_bits` on this runtime's target. `Err`: the code
    /// object's slot layout is malformed.
    pub(crate) fn for_code(py: &PyToken<'_>, code_bits: u64) -> Result<Self, &'static str> {
        let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
            return Err("frame plan requires a code object");
        };
        let optimized = unsafe { code_flags(code_ptr) } & CO_OPTIMIZED != 0;
        let homes = if optimized {
            let count = unsafe { code_slots(code_ptr) }?.names.len();
            u32::try_from(count).map_err(|_| "code object has too many frame slots")?
        } else {
            0
        };
        Ok(Self {
            optimized,
            homes,
            policy: FramePolicy::of_runtime(py),
        })
    }

    pub(crate) fn bits(self) -> u64 {
        if !self.optimized {
            return 0;
        }
        Self::OPTIMIZED | (self.policy.bits() << Self::POLICY_SHIFT) | u64::from(self.homes)
    }

    pub(crate) fn from_bits(bits: u64) -> Self {
        if bits & Self::OPTIMIZED == 0 {
            return Self::default();
        }
        Self {
            optimized: true,
            homes: bits as u32,
            policy: FramePolicy::from_bits(bits >> Self::POLICY_SHIFT),
        }
    }
}

/// Which of CPython's frame-object release paths runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameRelease {
    /// `frame.clear()` and cycle collection (`frame_tp_clear`), and an
    /// exiting frame whose unshared frame object dies first
    /// (`frame_dealloc`, then `_PyFrame_ClearLocals`).
    Clear,
    /// A frame object's destruction after it took over the bindings
    /// (`frame_dealloc` of a frame it owns).
    Destroy,
}

/// One part of what a frame object releases.
#[derive(Clone, Copy)]
enum FramePart {
    /// Every code slot, in the target's clear order.
    Slots,
    /// Before PEP 667 the `f_locals` dict, afterwards the extra locals.
    Locals,
    /// The overwritten values, newest first (3.14).
    History,
}

impl FrameRelease {
    fn parts(self, policy: FramePolicy) -> &'static [FramePart] {
        use FramePart::{History, Locals, Slots};
        match (self, policy.pep667()) {
            (Self::Clear, false) => &[Slots, Locals],
            (Self::Clear, true) => &[Locals, History, Slots],
            (Self::Destroy, false) => &[Locals, Slots],
            (Self::Destroy, true) => &[Slots, Locals, History],
        }
    }
}

#[inline]
unsafe fn word(ptr: *mut u8, index: usize) -> *mut u64 {
    unsafe { ptr.add(index * WORD).cast::<u64>() }
}

#[inline]
fn home_words(count: usize) -> Option<usize> {
    count.checked_mul(FRAME_HOME_WORDS)
}

/// Whether a kind is valid in a home or a retired pair.
#[inline]
fn valid_kind(kind: i64) -> bool {
    matches!(
        kind,
        FRAME_HOME_UNBOUND
            | FRAME_HOME_PLAIN
            | FRAME_HOME_RAW_INT
            | FRAME_HOME_CELL
            | FRAME_HOME_PRIVATE_CELL
    )
}

/// Whether a valid kind owns a reference to its bits.
#[inline]
fn holds_reference(kind: i64) -> bool {
    kind & FRAME_HOME_HOLDS_REFERENCE != 0 && valid_kind(kind)
}

/// Whether the frame object keeps `bits` when a 3.14 proxy write displaces
/// it: every heap object that is not immortal.
fn history_keeps(bits: u64) -> bool {
    obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { !(*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_IMMORTAL) })
}

/// Where a frame entry's homes end in the arena; restored at the entry's exit.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct HomeMark {
    chunk: u32,
    used: u32,
}

#[derive(Default)]
struct HomeArena {
    chunks: Vec<Box<[u64]>>,
    chunk: usize,
    used: usize,
}

impl HomeArena {
    fn take(&mut self, words: usize) -> Option<(*mut u64, HomeMark)> {
        let mark = HomeMark {
            chunk: u32::try_from(self.chunk).ok()?,
            used: u32::try_from(self.used).ok()?,
        };
        let fits = self
            .chunks
            .get(self.chunk)
            .is_some_and(|chunk| chunk.len() - self.used >= words);
        if !fits {
            let next = if self.chunks.is_empty() {
                0
            } else {
                self.chunk + 1
            };
            // Chunks beyond the current one hold no live homes.
            if self
                .chunks
                .get(next)
                .is_none_or(|chunk| chunk.len() < words)
            {
                let len = words.max(HOME_CHUNK_WORDS);
                let mut storage = Vec::new();
                storage.try_reserve_exact(len).ok()?;
                storage.resize(len, 0);
                let chunk = storage.into_boxed_slice();
                if next < self.chunks.len() {
                    self.chunks[next] = chunk;
                } else {
                    self.chunks.try_reserve(1).ok()?;
                    self.chunks.push(chunk);
                }
            }
            self.chunk = next;
            self.used = 0;
        }
        let chunk = &mut self.chunks[self.chunk];
        let homes = &mut chunk[self.used..self.used + words];
        homes.fill(0);
        self.used += words;
        Some((homes.as_mut_ptr(), mark))
    }

    fn give_back(&mut self, mark: HomeMark) {
        self.chunk = mark.chunk as usize;
        self.used = mark.used as usize;
    }
}

thread_local! {
    static HOME_ARENA: RefCell<HomeArena> = RefCell::new(HomeArena::default());
}

/// Initialize the thread's home arena before the runtime's thread-local
/// guard (`state::lifecycle::touch_tls_guard`). Thread-local storage is
/// destroyed in reverse initialization order, so the guard's teardown, which
/// exits every frame the thread still has, runs while the arena that holds
/// their homes is alive.
pub(crate) fn touch_frame_home_tls_lifetime() {
    let _ = HOME_ARENA.try_with(|_| {});
}

/// One frame entry's binding homes and its observers' payload.
#[derive(Clone, Copy, Default)]
pub(crate) struct FrameBindings {
    /// Owned `FRAME_BINDINGS` payload of a synchronous frame, zero until an
    /// observer asks for one; the captured payload of a traceback frame. A
    /// stateful frame's payload belongs to its task.
    pub(crate) payload_bits: u64,
    /// An optimized synchronous activation: its bindings are its homes.
    optimized: bool,
    /// Homes base address, or zero when the activation has no code slots.
    homes: usize,
    home_count: u32,
    policy: FramePolicy,
    mark: HomeMark,
}

impl FrameBindings {
    /// The frame of a traceback payload or a frame view: no homes, the
    /// captured payload.
    pub(crate) fn captured(payload_bits: u64) -> Self {
        Self {
            payload_bits,
            ..Self::default()
        }
    }

    /// Take the homes `plan` calls for, for the frame entry being pushed.
    /// `None`: the arena could not grow, or the thread's storage is already
    /// being destroyed.
    pub(crate) fn enter(plan: FramePlan) -> Option<Self> {
        let mut bindings = Self {
            optimized: plan.optimized,
            policy: plan.policy,
            ..Self::default()
        };
        if plan.optimized && plan.homes != 0 {
            let words = home_words(plan.homes as usize)?;
            let (homes, mark) = HOME_ARENA
                .try_with(|arena| arena.borrow_mut().take(words))
                .ok()
                .flatten()?;
            bindings.homes = homes as usize;
            bindings.home_count = plan.homes;
            bindings.mark = mark;
        }
        Some(bindings)
    }

    /// Return the homes of an entry whose bindings already left them
    /// ([`frame_bindings_exit`]). The arena outlives every frame of its
    /// thread ([`touch_frame_home_tls_lifetime`]), so it is still there.
    pub(crate) fn exit(self) {
        if self.homes != 0 {
            let _ = HOME_ARENA.try_with(|arena| arena.borrow_mut().give_back(self.mark));
        }
    }
}

/// Why a frame's payload could not be attached. A traceback capture that
/// meets one is abandoned whole: the raise keeps its original exception.
#[derive(Debug)]
pub(crate) enum ObserveError {
    Layout(&'static str),
    Allocation,
}

impl ObserveError {
    /// Report on behalf of an observer that is not itself recording a raise.
    pub(crate) fn raise(self, py: &PyToken<'_>) {
        if exception_pending(py) {
            return;
        }
        match self {
            Self::Layout(message) => raise_exception::<u64>(py, "SystemError", message),
            Self::Allocation => {
                raise_exception::<u64>(py, "MemoryError", "cannot allocate frame bindings")
            }
        };
    }
}

fn code_slot_count(code_bits: u64) -> Result<usize, ObserveError> {
    let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
        return Ok(0);
    };
    unsafe { code_slots(code_ptr) }
        .map(|slots| slots.names.len())
        .map_err(ObserveError::Layout)
}

fn payload_words(count: usize) -> Option<usize> {
    home_words(count)?.checked_add(SLOT_BASE_WORD)
}

/// A payload over `source` for an activation of `code_bits`, owned by the
/// caller, every slot pair unbound.
fn alloc_payload(
    py: &PyToken<'_>,
    code_bits: u64,
    count: usize,
    policy: FramePolicy,
    state: u64,
    source: u64,
) -> Result<u64, ObserveError> {
    let size = payload_words(count)
        .and_then(|words| words.checked_mul(WORD))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MoltHeader>()))
        .ok_or(ObserveError::Allocation)?;
    let ptr = alloc_object(py, size, TYPE_ID_FRAME_BINDINGS);
    if ptr.is_null() {
        return Err(ObserveError::Allocation);
    }
    unsafe {
        inc_ref_bits(py, code_bits);
        *word(ptr, CODE_WORD) = code_bits;
        *word(ptr, STATE_WORD) = state | (policy.bits() << STATE_POLICY_SHIFT);
        *word(ptr, COUNT_WORD) = count as u64;
        *word(ptr, SOURCE_WORD) = source;
        *word(ptr, LOCALS_WORD) = 0;
        *word(ptr, HISTORY_WORD) = 0;
        std::ptr::write_bytes(word(ptr, SLOT_BASE_WORD), 0, count * FRAME_HOME_WORDS);
        object_mark_has_ptrs(py, ptr);
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

/// The payload of the Python frame at `FRAME_STACK[index]`, attaching one on
/// first observation. Borrowed: a synchronous frame's entry owns it, a
/// stateful frame's task does. `Ok(0)`: the entry has no optimized Python
/// bindings (a runtime context, a module body). No stack borrow is held
/// across the allocation; an entry that a reentrant finalizer changed keeps
/// its state and the fresh payload is dropped.
pub(crate) fn frame_bindings_observe(py: &PyToken<'_>, index: usize) -> Result<u64, ObserveError> {
    let candidate = FRAME_STACK.with(|stack| {
        stack
            .borrow()
            .get(index)
            .map(|entry| (entry.code_bits, entry.activation_bits, entry.bindings))
    });
    let Some((code_bits, activation_bits, bindings)) = candidate else {
        return Ok(0);
    };
    if let Some(task) = obj_from_bits(activation_bits).as_ptr() {
        return frame_bindings_attach_activation(py, task, true);
    }
    if !bindings.optimized || code_bits == 0 || bindings.payload_bits != 0 {
        return Ok(bindings.payload_bits);
    }
    let count = code_slot_count(code_bits)?;
    if count != bindings.home_count as usize {
        return Err(ObserveError::Layout(
            "frame binding homes do not match the code layout",
        ));
    }
    let payload = alloc_payload(
        py,
        code_bits,
        count,
        bindings.policy,
        STATE_LIVE_HOMES,
        bindings.homes as u64,
    )?;
    let installed = FRAME_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        match stack.get_mut(index) {
            Some(entry)
                if entry.code_bits == code_bits
                    && entry.bindings.homes == bindings.homes
                    && entry.bindings.payload_bits == 0 =>
            {
                entry.bindings.payload_bits = payload;
                Some(payload)
            }
            Some(entry) if entry.bindings.homes == bindings.homes => {
                Some(entry.bindings.payload_bits)
            }
            _ => None,
        }
    });
    match installed {
        Some(bits) if bits == payload => Ok(payload),
        other => {
            dec_ref_bits(py, payload);
            Ok(other.unwrap_or(0))
        }
    }
}

/// The payload of a stateful activation, attached to its task on first
/// observation. Borrowed from the task. `Ok(0)`: a runtime-native task has no
/// Python frame, and a finished activation seen from outside (`gi_frame`)
/// reports none. An `executing` activation's frame, on this thread's stack,
/// keeps its bindings until it exits, whatever its completion flags say.
pub(crate) fn frame_bindings_attach_activation(
    py: &PyToken<'_>,
    task: *mut u8,
    executing: bool,
) -> Result<u64, ObserveError> {
    if !executing && unsafe { super::activation::activation_finished(task) } {
        return Ok(0);
    }
    let existing = crate::object::aux_header::object_frame_bindings_bits(task);
    if existing != 0 {
        return Ok(existing);
    }
    let code_bits = crate::object::aux_header::object_frame_code_bits(task);
    if code_bits == 0 {
        return Ok(0);
    }
    let count = code_slot_count(code_bits)?;
    let policy = frame_policy_for_code(py, code_bits);
    let payload = alloc_payload(
        py,
        code_bits,
        count,
        policy,
        STATE_LIVE_ACTIVATION,
        task as usize as u64,
    )?;
    // Allocation can run a finalizer that observed the same activation.
    let installed =
        unsafe { crate::object::aux_header::object_install_frame_bindings(task, payload) };
    if installed == payload {
        Ok(payload)
    } else {
        dec_ref_bits(py, payload);
        Ok(installed)
    }
}

/// The frame policy of an activation of `code_bits`: its compiled code slot's,
/// or the runtime target's for code that no compiled slot publishes.
pub(crate) fn frame_policy_for_code(py: &PyToken<'_>, code_bits: u64) -> FramePolicy {
    super::compiled_slot_for_code(code_bits)
        .and_then(|slot| {
            crate::runtime_state(py)
                .code_slots
                .get()
                .and_then(|slots| slots.get(usize::try_from(slot).ok()?))
                .map(|slot| slot.frame_plan())
                .filter(|plan| plan.optimized)
                .map(|plan| plan.policy)
        })
        .unwrap_or_else(|| FramePolicy::of_runtime(py))
}

/// Report bindings that held an invalid kind: malformed transport, never an
/// unbound slot. An exception already pending wins.
fn report_malformed(py: &PyToken<'_>) {
    if !exception_pending(py) {
        raise_exception::<u64>(
            py,
            "SystemError",
            "frame binding home has an invalid storage kind",
        );
    }
}

fn state_of(ptr: *mut u8) -> u64 {
    unsafe { *word(ptr, STATE_WORD) }
}

pub(crate) fn frame_bindings_policy(ptr: *mut u8) -> FramePolicy {
    FramePolicy::from_bits(state_of(ptr) >> STATE_POLICY_SHIFT)
}

fn set_source_state(ptr: *mut u8, source_state: u64) {
    unsafe {
        let state = *word(ptr, STATE_WORD);
        *word(ptr, STATE_WORD) = (state & !STATE_SOURCE_MASK) | source_state;
        if source_state & (STATE_LIVE_HOMES | STATE_LIVE_ACTIVATION) == 0 {
            *word(ptr, SOURCE_WORD) = 0;
        }
    }
}

/// Whether anything besides the one owner the caller holds shares `ptr`.
pub(crate) fn shared(ptr: *mut u8) -> bool {
    unsafe { !(*header_from_obj_ptr(ptr)).is_uniquely_owned() }
}

/// Take the frame object's extras out of a payload: its locals dict, and its
/// overwritten values, newest last.
unsafe fn take_extras(ptr: *mut u8) -> (u64, Vec<u64>) {
    unsafe {
        let locals = word(ptr, LOCALS_WORD).replace(0);
        let history = word(ptr, HISTORY_WORD).replace(0);
        let history = if history == 0 {
            Vec::new()
        } else {
            *Box::from_raw(history as usize as *mut Vec<u64>)
        };
        (locals, history)
    }
}

/// Release a frame's slots at `pairs` and its frame object's detached
/// `locals` and `history` in `parts` order, each pair unbound before its
/// object is released. `Err` when a pair held an invalid kind.
unsafe fn release_parts(
    py: &PyToken<'_>,
    parts: &[FramePart],
    policy: FramePolicy,
    pairs: *mut u64,
    count: usize,
    locals: u64,
    history: Vec<u64>,
) -> Result<(), ()> {
    let mut malformed = false;
    let mut history = Some(history);
    for part in parts {
        match part {
            FramePart::Slots => policy.for_each_slot(count, |slot| unsafe {
                let pair = pairs.add(slot * FRAME_HOME_WORDS);
                let kind = pair.replace(FRAME_HOME_UNBOUND as u64) as i64;
                let bits = pair.add(1).replace(0);
                if holds_reference(kind) {
                    dec_ref_bits(py, bits);
                } else if !valid_kind(kind) && kind != RETIRED_MALFORMED {
                    malformed = true;
                }
            }),
            FramePart::Locals => dec_ref_bits(py, locals),
            FramePart::History => {
                for bits in history.take().into_iter().flatten().rev() {
                    dec_ref_bits(py, bits);
                }
            }
        }
    }
    // Only a 3.14 frame keeps overwritten values, and it releases them above.
    debug_assert!(history.is_none_or(|history| history.is_empty()));
    if malformed { Err(()) } else { Ok(()) }
}

/// Move a live payload's bindings from `homes` into its own pairs: CPython's
/// frame object taking ownership of an exiting frame's locals. Nothing is
/// retained or released.
unsafe fn retire_homes(py: &PyToken<'_>, ptr: *mut u8, homes: *mut u64, count: usize) {
    let mut malformed = false;
    unsafe {
        for slot in 0..count {
            let home = homes.add(slot * FRAME_HOME_WORDS);
            let kind = home.replace(FRAME_HOME_UNBOUND as u64) as i64;
            let bits = home.add(1).replace(0);
            let (kind, bits) = if valid_kind(kind) {
                (kind, bits)
            } else {
                // An invalid kind's bits cannot be trusted as an owner.
                malformed = true;
                (RETIRED_MALFORMED, 0)
            };
            let pair = word(ptr, SLOT_BASE_WORD + slot * FRAME_HOME_WORDS);
            *pair = kind as u64;
            *pair.add(1) = bits;
        }
    }
    set_source_state(ptr, STATE_RETIRED);
    if malformed {
        report_malformed(py);
    }
}

/// The popped frame's bindings leave its homes. A payload an observer shares
/// takes them over. Otherwise the frame object dies first, as CPython's
/// `frame_dealloc` precedes `_PyFrame_ClearLocals`: 3.13+ extra locals, then
/// 3.14 overwritten values newest first, then the slots in the target's
/// clear order; before PEP 667 the slots precede the activation's `f_locals`
/// dict. Runs after the pop, so each finalizer sees the caller; the homes stay
/// reserved until [`FrameBindings::exit`].
pub(crate) fn frame_bindings_exit(py: &PyToken<'_>, bindings: &FrameBindings) {
    if !bindings.optimized {
        return;
    }
    // Null only when the code has no slots: nothing below reads it then.
    let homes = bindings.homes as *mut u64;
    let count = bindings.home_count as usize;
    let (locals, history) = match obj_from_bits(bindings.payload_bits).as_ptr() {
        Some(ptr) if shared(ptr) => {
            unsafe { retire_homes(py, ptr, homes, count) };
            return;
        }
        Some(ptr) => {
            // Only the entry holds the frame object: it owns no binding from
            // here on and dies when the entry drops it.
            set_source_state(ptr, STATE_RETIRED);
            unsafe { take_extras(ptr) }
        }
        None => (0, Vec::new()),
    };
    let parts = FrameRelease::Clear.parts(bindings.policy);
    if unsafe { release_parts(py, parts, bindings.policy, homes, count, locals, history) }.is_err()
    {
        report_malformed(py);
    }
}

/// Lend the executing frame's homes to its compiled code, which addresses at
/// most `min_count` code slots: raw bits, never an object. A frame entry calls
/// this right after `molt_trace_enter_slot` and before that entry's exception
/// check; a split chunk calls it at its own entry, inside the frame it runs
/// in. Zero is a failure for the caller's exception edge, never storage, and
/// compiled code runs no home operation after it: either an exception is
/// already pending (the entry failed and pushed no homes; a call, and so a
/// chunk, never starts with one pending) and stays so, or the executing frame
/// has fewer homes than its compiled code addresses, a compiler and runtime
/// layout disagreement raised as `SystemError`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_homes(min_count: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return 0;
        }
        let min_count = usize::try_from(min_count).unwrap_or(usize::MAX);
        let homes = FRAME_STACK.with(|stack| {
            stack.borrow().last().and_then(|entry| {
                let bindings = entry.bindings;
                (bindings.homes != 0 && bindings.home_count as usize >= min_count)
                    .then_some(bindings.homes)
            })
        });
        match homes {
            Some(homes) => homes as u64,
            None => {
                raise_exception::<u64>(
                    py,
                    "SystemError",
                    "compiled frame has no binding homes for its code slots",
                );
                0
            }
        }
    })
}

/// A home address compiled code was never lent: malformed ABI input, since a
/// failed lend leaves on its exception edge before any home operation. An
/// exception already pending is kept.
fn unlent_home(py: &PyToken<'_>) -> u64 {
    if !exception_pending(py) {
        raise_exception::<u64>(
            py,
            "SystemError",
            "compiled frame used a binding home it was never lent",
        );
    }
    MoltObject::none().bits()
}

/// A compiled read of a plain binding whose home does not hold a boxed
/// object (`frame_home_load`'s slow path; backends read `PLAIN` inline).
/// Returns the binding borrowed from its home: an unbound slot reads as the
/// missing sentinel; a raw integer is boxed into the home first, so the
/// result borrows the home's own reference. `home` is the slot's address in
/// the homes `molt_frame_homes` lent. A failed boxing leaves its exception
/// pending and the home unchanged; a cell or invalid kind raises
/// `SystemError`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_home_load(home: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let pair = home as usize as *mut u64;
        if pair.is_null() {
            return unlent_home(py);
        }
        unsafe { plain_home_value(py, pair) }.unwrap_or_else(|()| missing_bits(py))
    })
}

/// Borrow the plain binding from its persistent owner. Every raw-home
/// observation publishes the first box into this pair, including observations
/// of retired frame storage; subsequent observations retain or move that same
/// object. Boxing failure preserves the raw binding and the pending exception.
///
/// `pair` must address a live home reserved by its activation or a pair in an
/// owned retired payload, under the GIL. Detached payloads have no pair. Integer
/// boxing allocates a non-cyclic BigInt without calling Python or releasing the
/// GIL, so publication cannot race a rebinding, retirement or clear.
unsafe fn plain_home_value(py: &PyToken<'_>, pair: *mut u64) -> Result<u64, ()> {
    let (kind, bits) = unsafe { (*pair as i64, *pair.add(1)) };
    match kind {
        FRAME_HOME_PLAIN => Ok(bits),
        FRAME_HOME_UNBOUND => Ok(missing_bits(py)),
        FRAME_HOME_RAW_INT => {
            // Do not mint and then discard an owner under an earlier error.
            if exception_pending(py) {
                return Err(());
            }
            let boxed = crate::int_bits_from_i64(py, bits as i64);
            if exception_pending(py) {
                return Err(());
            }
            unsafe {
                *pair.add(1) = boxed;
                *pair = FRAME_HOME_PLAIN as u64;
            }
            Ok(boxed)
        }
        _ => invalid(py, "frame binding home holds no plain binding"),
    }
}

/// Move a binding out of its home (`frame_home_take`): the home becomes
/// unbound and the caller owns what it held, an object or a cell, a boxed raw
/// integer, or the missing sentinel for an unbound slot. PEP 709's save of an
/// enclosing binding, which a store puts back. A failed boxing leaves its
/// exception pending and the home unchanged.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_home_take(home: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let pair = home as usize as *mut u64;
        if pair.is_null() {
            return unlent_home(py);
        }
        let missing = missing_bits(py);
        let (kind, bits) = unsafe { (*pair as i64, *pair.add(1)) };
        let taken = match kind {
            FRAME_HOME_PLAIN | FRAME_HOME_CELL | FRAME_HOME_PRIVATE_CELL => bits,
            FRAME_HOME_UNBOUND => return missing,
            FRAME_HOME_RAW_INT => {
                let Ok(boxed) = (unsafe { plain_home_value(py, pair) }) else {
                    return missing;
                };
                boxed
            }
            _ => {
                report_malformed(py);
                return missing;
            }
        };
        unsafe {
            *pair = FRAME_HOME_UNBOUND as u64;
            *pair.add(1) = 0;
        }
        taken
    })
}

/// Argument zero of the executing synchronous frame, for zero-argument
/// `super()`: the current binding of its first code slot, owned (a cell
/// slot's contents), as CPython reads `localsplus[0]`. Nothing else owns a
/// copy, so a rebinding or a proxy write is what `super()` sees. `Ok(None)`:
/// the slot is unbound (`arg[0] deleted`). `Err`: an exception is pending.
pub(crate) fn frame_argument_zero(py: &PyToken<'_>) -> Result<Option<u64>, ()> {
    let home = FRAME_STACK.with(|stack| {
        stack
            .borrow()
            .last()
            .map(|entry| entry.bindings.homes as *mut u64)
            .filter(|homes| !homes.is_null())
    });
    let Some(pair) = home else {
        return invalid(py, "super(): the executing frame has no argument-zero home");
    };
    let (kind, bits) = unsafe { (*pair as i64, *pair.add(1)) };
    let value = match kind {
        FRAME_HOME_UNBOUND => return Ok(None),
        FRAME_HOME_PLAIN | FRAME_HOME_RAW_INT => unsafe { plain_home_value(py, pair) }?,
        FRAME_HOME_CELL | FRAME_HOME_PRIVATE_CELL => match cell_ptr_from_bits(bits) {
            Some(cell) => unsafe { cell_value_bits(cell) },
            None => return invalid(py, "frame cell slot does not hold a cell"),
        },
        _ => return invalid(py, "frame binding home has an invalid storage kind"),
    };
    if is_missing_bits(py, value) {
        return Ok(None);
    }
    inc_ref_bits(py, value);
    Ok(Some(value))
}

/// Code slots' current bindings, as an observer sees them.
pub(crate) struct FrameBindingItems<'a, 'py> {
    py: &'a PyToken<'py>,
    /// Owned name and value edges in slot order; `None` is an unbound slot.
    pub(crate) slots: Vec<(u64, Option<u64>)>,
    /// Owned extra-locals dict of a PEP 667 frame, or zero.
    extra: u64,
}

impl FrameBindingItems<'_, '_> {
    pub(crate) fn bound(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.slots
            .iter()
            .filter_map(|&(name, value)| value.map(|value| (name, value)))
    }

    /// Record one slot, retaining its name and value.
    fn push(&mut self, name: u64, value: Option<u64>) {
        inc_ref_bits(self.py, name);
        if let Some(value) = value {
            inc_ref_bits(self.py, value);
        }
        self.slots.push((name, value));
    }
}

impl Drop for FrameBindingItems<'_, '_> {
    fn drop(&mut self) {
        for (name, value) in self.slots.drain(..) {
            dec_ref_bits(self.py, name);
            if let Some(bits) = value {
                dec_ref_bits(self.py, bits);
            }
        }
        if self.extra != 0 {
            dec_ref_bits(self.py, std::mem::take(&mut self.extra));
        }
    }
}

fn invalid<T>(py: &PyToken<'_>, message: &'static str) -> Result<T, ()> {
    if !exception_pending(py) {
        raise_exception::<u64>(py, "SystemError", message);
    }
    Err(())
}

/// Check a payload reference before reading any payload word. A frame
/// object's, a proxy's or a traceback's typed field is the only way to reach
/// one, but the extent is checked at every fallible entry regardless.
pub(crate) fn frame_bindings_ptr(py: &PyToken<'_>, payload_bits: u64) -> Result<*mut u8, ()> {
    let Some(ptr) = obj_from_bits(payload_bits).as_ptr() else {
        return invalid(py, "frame bindings reference is not an object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FRAME_BINDINGS {
            return invalid(py, "frame bindings reference has the wrong type");
        }
        let size = crate::object::object_payload_size(ptr);
        let count = *word(ptr, COUNT_WORD) as usize;
        let required = payload_words(count).and_then(|words| words.checked_mul(WORD));
        if size < SLOT_BASE_WORD * WORD || required.is_none_or(|required| required > size) {
            return invalid(py, "frame bindings payload lies outside its object");
        }
    }
    Ok(ptr)
}

/// Whether the target observes this payload's frame through a PEP 667 proxy.
pub(crate) fn frame_bindings_pep667(ptr: *mut u8) -> bool {
    frame_bindings_policy(ptr).pep667()
}

/// The slot pairs a payload reads: the running frame's homes while live over
/// them, its own once retired. `None` for a stateful activation's live
/// bindings (`activation.rs` projects them) and once detached.
unsafe fn pair_words(ptr: *mut u8) -> Option<*mut u64> {
    unsafe {
        let state = state_of(ptr);
        if state & STATE_LIVE_HOMES != 0 {
            Some(*word(ptr, SOURCE_WORD) as usize as *mut u64)
        } else if state & STATE_RETIRED != 0 {
            Some(word(ptr, SLOT_BASE_WORD))
        } else {
            None
        }
    }
}

/// The stateful activation a live payload reads, borrowed.
pub(crate) fn frame_bindings_live_activation(ptr: *mut u8) -> Option<*mut u8> {
    (state_of(ptr) & STATE_LIVE_ACTIVATION != 0)
        .then(|| unsafe { *word(ptr, SOURCE_WORD) } as usize as *mut u8)
        .filter(|task| !task.is_null())
}

/// The contents of a `str` object.
///
/// # Safety
/// The object must outlive the returned slice.
unsafe fn string_key<'a>(bits: u64) -> Option<&'a [u8]> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        (object_type_id(ptr) == TYPE_ID_STRING).then(|| {
            std::slice::from_raw_parts(
                crate::object::string_bytes(ptr),
                crate::object::string_len(ptr),
            )
        })
    }
}

fn code_of(py: &PyToken<'_>, ptr: *mut u8) -> Result<*mut u8, ()> {
    match obj_from_bits(unsafe { *word(ptr, CODE_WORD) }).as_ptr() {
        Some(code) => Ok(code),
        None => invalid(py, "frame bindings payload lost its code object"),
    }
}

/// The payload's code slot named `key`, in localsplus order (a name that is
/// both a local and a cell has one slot). `Ok(None)`: not a code slot.
fn code_slot_of(py: &PyToken<'_>, ptr: *mut u8, key: u64) -> Result<Option<usize>, ()> {
    // The caller holds the key; the code object holds the slot names.
    let Some(key) = (unsafe { string_key(key) }) else {
        return Ok(None);
    };
    let code = code_of(py, ptr)?;
    let layout = match unsafe { code_slots(code) } {
        Ok(layout) => layout,
        Err(message) => return invalid(py, message),
    };
    if layout.names.len() != unsafe { *word(ptr, COUNT_WORD) } as usize {
        return invalid(py, "frame bindings payload does not match its code layout");
    }
    Ok(layout
        .names
        .iter()
        .position(|&name| unsafe { string_key(name) } == Some(key)))
}

/// Every code slot of the observed frame in slot order, each bound value
/// retained (a cell slot shows the cell's contents, a raw integer is boxed into
/// its home before the observation retains its own reference),
/// plus a PEP 667 frame's extra locals. `Err`: an exception is pending; an
/// invalid payload is reported as `SystemError`, never as empty locals.
pub(crate) fn frame_bindings_items<'a, 'py>(
    py: &'a PyToken<'py>,
    payload_bits: u64,
) -> Result<FrameBindingItems<'a, 'py>, ()> {
    let ptr = frame_bindings_ptr(py, payload_bits)?;
    let mut items = FrameBindingItems {
        py,
        slots: Vec::new(),
        extra: 0,
    };
    if frame_bindings_pep667(ptr) {
        let extra = unsafe { *word(ptr, LOCALS_WORD) };
        if extra != 0 {
            inc_ref_bits(py, extra);
            items.extra = extra;
        }
    }
    if let Some(task) = frame_bindings_live_activation(ptr) {
        // Owned pairs, in the activation's layout order.
        let pairs = unsafe { super::activation::activation_binding_pairs(py, task) }?;
        items.slots = pairs
            .into_iter()
            .map(|(name, value)| (name, Some(value)))
            .collect();
        return Ok(items);
    }
    let Some(pairs) = (unsafe { pair_words(ptr) }) else {
        return Ok(items);
    };
    let code = code_of(py, ptr)?;
    let layout = match unsafe { code_slots(code) } {
        Ok(layout) => layout,
        Err(message) => return invalid(py, message),
    };
    let count = unsafe { *word(ptr, COUNT_WORD) } as usize;
    if layout.names.len() != count {
        return invalid(py, "frame bindings payload does not match its code layout");
    }
    items.slots.reserve(count);
    for (slot, &name) in layout.names.iter().enumerate() {
        let pair = unsafe { pairs.add(slot * FRAME_HOME_WORDS) };
        let (kind, bits) = unsafe { (*pair as i64, *pair.add(1)) };
        let value = match kind {
            FRAME_HOME_UNBOUND => None,
            FRAME_HOME_PLAIN | FRAME_HOME_RAW_INT => {
                let value = unsafe { plain_home_value(py, pair) }?;
                (!is_missing_bits(py, value)).then_some(value)
            }
            FRAME_HOME_CELL | FRAME_HOME_PRIVATE_CELL => {
                let Some(cell) = cell_ptr_from_bits(bits) else {
                    return invalid(py, "frame cell slot does not hold a cell");
                };
                let contents = unsafe { cell_value_bits(cell) };
                (!is_missing_bits(py, contents)).then_some(contents)
            }
            _ => return invalid(py, "frame binding home has an invalid storage kind"),
        };
        items.push(name, value);
    }
    Ok(items)
}

fn alloc_locals_dict(py: &PyToken<'_>, items: &FrameBindingItems<'_, '_>) -> Result<u64, ()> {
    let mut pairs: Vec<u64> = items
        .bound()
        .flat_map(|(name, value)| [name, value])
        .collect();
    let extra = if let Some(extra) = obj_from_bits(items.extra)
        .as_ptr()
        .filter(|dict| unsafe { object_type_id(*dict) } == TYPE_ID_DICT)
    {
        Some(
            unsafe {
                crate::object::ops_dict::dict_snapshot(
                    py,
                    extra,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }
            .ok_or(())?,
        )
    } else {
        None
    };
    if let Some(extra) = &extra {
        pairs.extend(extra.iter().copied());
    }
    let dict = alloc_dict_with_pairs(py, &pairs);
    if dict.is_null() {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "MemoryError", "cannot allocate frame locals");
        }
        return Err(());
    }
    Ok(MoltObject::from_ptr(dict).bits())
}

/// A fresh dict of the bound slots and extra locals (PEP 667 `locals()` and
/// proxy reads). Owned. `Err`: an exception is pending.
pub(crate) fn frame_bindings_snapshot(py: &PyToken<'_>, payload_bits: u64) -> Result<u64, ()> {
    let items = frame_bindings_items(py, payload_bits)?;
    alloc_locals_dict(py, &items)
}

/// The activation's one `f_locals` dict before PEP 667 (3.12), refreshed from
/// its bindings as `PyFrame_FastToLocals` does: bound slots are set, unbound
/// ones removed, other keys kept. Shared by every frame object of the
/// activation and by `locals()`. Owned. `Err`: an exception is pending.
pub(crate) fn frame_bindings_locals_dict(py: &PyToken<'_>, payload_bits: u64) -> Result<u64, ()> {
    let items = frame_bindings_items(py, payload_bits)?;
    let ptr = frame_bindings_ptr(py, payload_bits)?;
    let cached = unsafe { *word(ptr, LOCALS_WORD) };
    let Some(dict) = obj_from_bits(cached)
        .as_ptr()
        .filter(|dict| unsafe { object_type_id(*dict) } == TYPE_ID_DICT)
    else {
        let dict = alloc_locals_dict(py, &items)?;
        // Allocation can run a finalizer that asked for the same dict.
        let previous = unsafe { word(ptr, LOCALS_WORD).replace(dict) };
        inc_ref_bits(py, dict);
        if previous != 0 {
            dec_ref_bits(py, previous);
        }
        return Ok(dict);
    };
    inc_ref_bits(py, cached);
    for &(name, value) in &items.slots {
        unsafe {
            match value {
                Some(value) => dict_set_in_place(py, dict, name, value),
                None => {
                    if dict_get_in_place(py, dict, name).is_some() {
                        let _ = dict_del_in_place(py, dict, name);
                    }
                }
            }
        }
        if exception_pending(py) {
            dec_ref_bits(py, cached);
            return Err(());
        }
    }
    Ok(cached)
}

/// `locals()` of an optimized activation: before PEP 667 its one refreshed
/// `f_locals` dict, afterwards an independent snapshot. Owned. `Err`: an
/// exception is pending.
pub(crate) fn frame_bindings_locals(py: &PyToken<'_>, payload_bits: u64) -> Result<u64, ()> {
    let ptr = frame_bindings_ptr(py, payload_bits)?;
    if frame_bindings_pep667(ptr) {
        frame_bindings_snapshot(py, payload_bits)
    } else {
        frame_bindings_locals_dict(py, payload_bits)
    }
}

/// Make room for one more overwritten value, allocating the history on first
/// use. `Err`: `MemoryError` is pending and nothing changed.
unsafe fn history_reserve(py: &PyToken<'_>, ptr: *mut u8) -> Result<*mut Vec<u64>, ()> {
    unsafe {
        let slot = word(ptr, HISTORY_WORD);
        if *slot == 0 {
            let layout = std::alloc::Layout::new::<Vec<u64>>();
            let history = std::alloc::alloc(layout).cast::<Vec<u64>>();
            if history.is_null() {
                raise_exception::<u64>(py, "MemoryError", "cannot keep an overwritten frame local");
                return Err(());
            }
            history.write(Vec::new());
            *slot = history as usize as u64;
        }
        let history = *slot as usize as *mut Vec<u64>;
        // Amortized growth: one reservation per write, never a copy per write.
        if (*history).try_reserve(1).is_err() {
            raise_exception::<u64>(py, "MemoryError", "cannot keep an overwritten frame local");
            return Err(());
        }
        Ok(history)
    }
}

/// How a proxy write disposes of the plain value it displaces: from 3.14 the
/// frame object keeps a displaced non-immortal object until it is cleared;
/// earlier targets release it during the write. Room is reserved before the
/// write publishes anything.
pub(crate) enum Displacement {
    Release,
    Keep(*mut Vec<u64>),
}

impl Displacement {
    /// Plan displacing `bits` from a plain slot of the frame whose payload is
    /// `ptr`. `Err`: `MemoryError` is pending and nothing changed.
    pub(crate) fn plan(py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> Result<Self, ()> {
        if frame_bindings_policy(ptr).keeps_history() && history_keeps(bits) {
            Ok(Self::Keep(unsafe { history_reserve(py, ptr)? }))
        } else {
            Ok(Self::Release)
        }
    }

    /// Dispose of the displaced owned reference, after the new binding is
    /// published.
    pub(crate) fn finish(self, py: &PyToken<'_>, bits: u64) {
        match self {
            Self::Keep(history) => unsafe { (*history).push(bits) },
            Self::Release => dec_ref_bits(py, bits),
        }
    }
}

/// Write `value` into one pair of live homes or a retired payload. An actual
/// cell's contents change and the old contents are released during the
/// write, on every target. A plain binding (a private cell's contents too) is
/// published before what it displaces is released (3.13) or kept by the frame
/// object (3.14). Writing the identical object changes nothing.
unsafe fn write_pair(py: &PyToken<'_>, ptr: *mut u8, pair: *mut u64, value: u64) -> Result<(), ()> {
    unsafe {
        let kind = *pair as i64;
        let bits = *pair.add(1);
        match kind {
            FRAME_HOME_CELL | FRAME_HOME_PRIVATE_CELL => {
                let Some(cell) = cell_ptr_from_bits(bits) else {
                    return invalid(py, "frame cell slot does not hold a cell");
                };
                let contents = cell_value_bits(cell);
                if contents == value {
                    return Ok(());
                }
                let displacement =
                    if kind == FRAME_HOME_PRIVATE_CELL && !is_missing_bits(py, contents) {
                        Displacement::plan(py, ptr, contents)?
                    } else {
                        Displacement::Release
                    };
                inc_ref_bits(py, value);
                let displaced = cell_detach_value(cell, value);
                displacement.finish(py, displaced);
                Ok(())
            }
            FRAME_HOME_PLAIN | FRAME_HOME_RAW_INT | FRAME_HOME_UNBOUND => {
                if kind == FRAME_HOME_PLAIN && bits == value {
                    return Ok(());
                }
                let displacement = if kind == FRAME_HOME_PLAIN {
                    Some(Displacement::plan(py, ptr, bits)?)
                } else {
                    None
                };
                inc_ref_bits(py, value);
                *pair.add(1) = value;
                *pair = FRAME_HOME_PLAIN as u64;
                // Published first: a finalizer the release runs sees the new
                // binding.
                if let Some(displacement) = displacement {
                    displacement.finish(py, bits);
                }
                Ok(())
            }
            _ => invalid(py, "frame binding home has an invalid storage kind"),
        }
    }
}

/// The payload's extra-locals dict, created on demand. `Ok(None)`: none yet
/// and `create` is false.
pub(crate) fn frame_bindings_extra_locals(
    py: &PyToken<'_>,
    ptr: *mut u8,
    create: bool,
) -> Result<Option<*mut u8>, ()> {
    let bits = unsafe { *word(ptr, LOCALS_WORD) };
    if let Some(dict) = obj_from_bits(bits).as_ptr() {
        if unsafe { object_type_id(dict) } != TYPE_ID_DICT {
            return invalid(py, "frame extra locals are not a dict");
        }
        return Ok(Some(dict));
    }
    if !create {
        return Ok(None);
    }
    let dict = alloc_dict_with_pairs(py, &[]);
    if dict.is_null() {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "MemoryError", "cannot allocate frame locals");
        }
        return Err(());
    }
    // Allocation can run a finalizer that created the same dict.
    let previous = unsafe { word(ptr, LOCALS_WORD).replace(MoltObject::from_ptr(dict).bits()) };
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
    Ok(Some(dict))
}

/// A PEP 667 proxy write (`value`) or deletion (`None`) of `key` in the frame
/// of `payload_bits`. A code slot's binding is replaced in the frame's storage
/// (a cell's contents for a cell slot) and cannot be deleted (`ValueError`);
/// any other key lives in the frame's extra locals, where deleting a missing
/// key raises `KeyError`. `Err`: an exception is pending.
pub(crate) fn frame_bindings_write(
    py: &PyToken<'_>,
    payload_bits: u64,
    key: u64,
    value: Option<u64>,
) -> Result<(), ()> {
    let ptr = frame_bindings_ptr(py, payload_bits)?;
    if !frame_bindings_pep667(ptr) {
        return invalid(
            py,
            "frame locals are written through a proxy only from 3.13",
        );
    }
    if let Some(slot) = code_slot_of(py, ptr, key)? {
        let Some(value) = value else {
            raise_exception::<u64>(
                py,
                "ValueError",
                "cannot remove local variables from FrameLocalsProxy",
            );
            return Err(());
        };
        if let Some(task) = frame_bindings_live_activation(ptr) {
            return unsafe {
                super::activation::activation_write_binding(py, task, ptr, key, value)
            };
        }
        let Some(pairs) = (unsafe { pair_words(ptr) }) else {
            return invalid(py, "frame bindings were detached");
        };
        let result = unsafe { write_pair(py, ptr, pairs.add(slot * FRAME_HOME_WORDS), value) };
        return match result {
            Ok(()) if exception_pending(py) => Err(()),
            other => other,
        };
    }
    let Some(dict) = frame_bindings_extra_locals(py, ptr, value.is_some())? else {
        raise_key_error_with_key::<u64>(py, key);
        return Err(());
    };
    unsafe {
        match value {
            Some(value) => dict_set_in_place(py, dict, key, value),
            None => {
                if !dict_del_in_place(py, dict, key) && !exception_pending(py) {
                    raise_key_error_with_key::<u64>(py, key);
                    return Err(());
                }
            }
        }
    }
    if exception_pending(py) {
        Err(())
    } else {
        Ok(())
    }
}

/// The code slot count of a payload.
pub(crate) fn frame_bindings_slot_count(ptr: *mut u8) -> usize {
    unsafe { *word(ptr, COUNT_WORD) as usize }
}

/// Take ownership of a finishing stateful activation's bindings: `pairs` are
/// `(code slot, kind, owned bits)`, moved in. The payload stops reading the
/// task; slots `pairs` does not name stay unbound.
pub(crate) fn frame_bindings_retire_activation(
    py: &PyToken<'_>,
    ptr: *mut u8,
    pairs: impl IntoIterator<Item = (usize, i64, u64)>,
) {
    let count = frame_bindings_slot_count(ptr);
    for (slot, kind, bits) in pairs {
        if slot >= count || !valid_kind(kind) {
            // No slot can receive it: the owner it carries ends here.
            if holds_reference(kind) {
                dec_ref_bits(py, bits);
            }
            continue;
        }
        unsafe {
            let pair = word(ptr, SLOT_BASE_WORD + slot * FRAME_HOME_WORDS);
            let previous_kind = pair.replace(kind as u64) as i64;
            let previous = pair.add(1).replace(bits);
            if holds_reference(previous_kind) {
                dec_ref_bits(py, previous);
            }
        }
    }
    set_source_state(ptr, STATE_RETIRED);
}

/// Stop a stateful payload from reading its task, keeping no binding: the
/// task finishes or dies without an observer that shares the payload, or
/// runtime teardown abandons it.
pub(crate) fn frame_bindings_detach_activation(ptr: *mut u8) {
    if state_of(ptr) & STATE_LIVE_ACTIVATION != 0 {
        set_source_state(ptr, STATE_RETIRED);
    }
}

/// Release what an unshared stateful payload owns as its frame object dies
/// before its activation's slots are cleared (3.13+: extra locals, then 3.14
/// overwritten values newest first). Before PEP 667 the activation's
/// `f_locals` dict is returned for release after the slots. Owned.
pub(crate) fn frame_bindings_release_extras(py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let (locals, history) = unsafe { take_extras(ptr) };
    if !frame_bindings_pep667(ptr) {
        return locals;
    }
    dec_ref_bits(py, locals);
    for bits in history.into_iter().rev() {
        dec_ref_bits(py, bits);
    }
    0
}

/// `frame.clear()` through a frame object's payload. An executing frame (a
/// synchronous frame on the stack, or a running activation) raises
/// `RuntimeError`; a stateful frame follows the target's generator rules; a
/// finished frame releases what its frame object owns in the explicit-clear
/// order (CPython `frame_tp_clear`) and stays retired with every slot
/// unbound. `Err`: an exception is pending.
pub(crate) fn frame_bindings_clear(py: &PyToken<'_>, payload_bits: u64) -> Result<(), ()> {
    let ptr = frame_bindings_ptr(py, payload_bits)?;
    let state = state_of(ptr);
    let policy = frame_bindings_policy(ptr);
    if state & STATE_LIVE_HOMES != 0 {
        raise_exception::<u64>(py, "RuntimeError", "cannot clear an executing frame");
        return Err(());
    }
    if let Some(task) = frame_bindings_live_activation(ptr) {
        return unsafe { super::activation::activation_frame_clear(py, task, policy) };
    }
    if state & STATE_RETIRED != 0 {
        let count = frame_bindings_slot_count(ptr);
        let pairs = unsafe { word(ptr, SLOT_BASE_WORD) };
        let (locals, history) = unsafe { take_extras(ptr) };
        let parts = FrameRelease::Clear.parts(policy);
        if unsafe { release_parts(py, parts, policy, pairs, count, locals, history) }.is_err() {
            report_malformed(py);
        }
    }
    if exception_pending(py) {
        Err(())
    } else {
        Ok(())
    }
}

/// Every owned edge of a payload: its code object, its locals dict, its
/// overwritten values, and the bound objects and cells once retired. A live
/// payload's bindings belong to the running frame or task.
///
/// # Safety
/// `ptr` must be a live `FRAME_BINDINGS` payload.
pub(crate) unsafe fn frame_bindings_visit(ptr: *mut u8, visit: &mut dyn FnMut(u64)) {
    unsafe {
        visit(*word(ptr, CODE_WORD));
        visit(*word(ptr, LOCALS_WORD));
        let history = *word(ptr, HISTORY_WORD);
        if history != 0 {
            for &bits in (*(history as usize as *const Vec<u64>)).iter() {
                visit(bits);
            }
        }
        if state_of(ptr) & STATE_RETIRED == 0 {
            return;
        }
        for slot in 0..frame_bindings_slot_count(ptr) {
            let kind = *word(ptr, SLOT_BASE_WORD + slot * FRAME_HOME_WORDS) as i64;
            if holds_reference(kind) {
                visit(*word(ptr, SLOT_BASE_WORD + slot * FRAME_HOME_WORDS + 1));
            }
        }
    }
}

/// Detach every edge of a payload for release, in the order of the frame
/// object's `entry` path, then the code object. A live payload stops reading
/// its frame: its bindings stay with their owner. The payload is left
/// detached.
///
/// # Safety
/// `ptr` must be a live `FRAME_BINDINGS` payload under the GIL.
pub(crate) unsafe fn frame_bindings_detach(
    ptr: *mut u8,
    entry: FrameRelease,
    mut detach: impl FnMut(u64),
) {
    unsafe {
        let policy = frame_bindings_policy(ptr);
        let retired = state_of(ptr) & STATE_RETIRED != 0;
        let count = frame_bindings_slot_count(ptr);
        set_source_state(ptr, STATE_DETACHED);
        let (locals, history) = take_extras(ptr);
        for part in entry.parts(policy) {
            match part {
                FramePart::Slots if retired => policy.for_each_slot(count, |slot| {
                    let pair = word(ptr, SLOT_BASE_WORD + slot * FRAME_HOME_WORDS);
                    let kind = pair.replace(FRAME_HOME_UNBOUND as u64) as i64;
                    let bits = pair.add(1).replace(0);
                    if holds_reference(kind) {
                        detach(bits);
                    }
                }),
                FramePart::Slots => {}
                FramePart::Locals => {
                    if locals != 0 {
                        detach(locals);
                    }
                }
                FramePart::History => {
                    for &bits in history.iter().rev() {
                        detach(bits);
                    }
                }
            }
        }
        detach(word(ptr, CODE_WORD).replace(MoltObject::none().bits()));
    }
}

#[cfg(test)]
#[path = "bindings_tests.rs"]
pub(super) mod tests;
