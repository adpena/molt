//! Intrusive membership of runtime objects in the cycle collector (HF-816).
//!
//! CPython links each tracked object into its generation's list through a
//! `PyGC_Head` placed before the object, so track and untrack touch no shared
//! table. Molt reserves one word, the GC head, before the `MoltHeader` of each
//! allocation whose kind can be tracked; `HEADER_FLAG_GC_HEAD` records the
//! reservation. Each runtime owns one dense vector of members per generation,
//! and an object's head names its own entry (generation and position). Track,
//! untrack and promotion are therefore a few loads and stores: no hash and no
//! lock. The runtime GIL, proven by a `PyToken`, gives the owner exclusive
//! access in every build (the free-threaded build still serializes execution).
//!
//! Invariant I1: the head of a tracked object reads (g, p) exactly when
//! `members[g][p]` names that object, and the head of an untracked object
//! reads zero. Every membership change goes through this module, whose fields
//! are private.

use super::{GcMembershipAdmission, NUM_GENERATIONS, PERMANENT_GENERATION};
use crate::object::{MoltHeader, PtrSlot};
use std::sync::atomic::{AtomicU64, Ordering};

/// One member list per generation, the permanent one included.
pub(super) const GENERATION_LISTS: usize = NUM_GENERATIONS + 1;

/// Bytes reserved before the `MoltHeader` of a head-bearing allocation.
pub(crate) const GC_HEAD_BYTES: usize = molt_codegen_abi::object_lifecycle::GC_HEAD_BYTES as usize;

// Head word: bit 0 = tracked, bits 1..=2 = generation, bits 3.. = position.
// Compiled code reads and writes it with the same layout.
use molt_codegen_abi::object_lifecycle::{
    GC_HEAD_GENERATION_MASK as HEAD_GENERATION_MASK,
    GC_HEAD_GENERATION_SHIFT as HEAD_GENERATION_SHIFT,
    GC_HEAD_POSITION_SHIFT as HEAD_POSITION_SHIFT, GC_HEAD_TRACKED as HEAD_TRACKED,
};

const _: () = assert!(GC_HEAD_BYTES == std::mem::size_of::<GcHead>());
const _: () = assert!(GC_HEAD_BYTES.is_multiple_of(molt_codegen_abi::HEADER_ALLOC_ALIGN_BYTES));
const _: () = assert!(PERMANENT_GENERATION as u64 <= HEAD_GENERATION_MASK);

/// The membership word before a head-bearing object. Every writer holds the
/// owning runtime's GIL; relaxed atomics keep a GIL-free reader (such as
/// `gc.is_tracked` through a C view) free of data races at no cost.
#[repr(transparent)]
struct GcHead(AtomicU64);

impl GcHead {
    /// # Safety
    /// `slot` is a live object whose allocation reserved a GC head.
    #[inline(always)]
    unsafe fn of<'a>(slot: PtrSlot) -> &'a Self {
        unsafe {
            &*slot
                .0
                .sub(std::mem::size_of::<MoltHeader>() + GC_HEAD_BYTES)
                .cast::<Self>()
        }
    }

    #[inline(always)]
    fn place(&self) -> Option<(Generation, usize)> {
        let word = self.0.load(Ordering::Relaxed);
        (word & HEAD_TRACKED != 0).then_some((
            Generation(((word >> HEAD_GENERATION_SHIFT) & HEAD_GENERATION_MASK) as u8),
            (word >> HEAD_POSITION_SHIFT) as usize,
        ))
    }

    #[inline(always)]
    fn set_place(&self, generation: Generation, position: usize) {
        let word = ((position as u64) << HEAD_POSITION_SHIFT)
            | (u64::from(generation.0) << HEAD_GENERATION_SHIFT)
            | HEAD_TRACKED;
        self.0.store(word, Ordering::Relaxed);
    }

    #[inline(always)]
    fn clear(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

/// A collector generation, including CPython's permanent (frozen) generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Generation(u8);

impl Generation {
    pub(super) const YOUNGEST: Self = Self(0);
    pub(super) const PERMANENT: Self = Self(PERMANENT_GENERATION);

    /// A generation from the collector's numbering; anything past the
    /// permanent generation is a caller defect.
    #[inline]
    pub(super) fn new(raw: u8) -> Self {
        assert!(raw <= PERMANENT_GENERATION, "invalid GC generation {raw}");
        Self(raw)
    }

    #[inline]
    pub(super) fn raw(self) -> u8 {
        self.0
    }

    /// This generation's member list, `0..GENERATION_LISTS`.
    #[inline]
    pub(super) fn list(self) -> usize {
        usize::from(self.0)
    }

    /// Every generation, youngest first, the permanent one last.
    pub(super) fn all() -> impl Iterator<Item = Self> {
        (0..=PERMANENT_GENERATION).map(Self)
    }
}

/// One tracked object and its allocation ordinal. Ordinals are unique per
/// runtime, so an ordinal names one tracking lifetime of one object.
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct GcMember {
    pub(super) slot: PtrSlot,
    pub(super) ordinal: u64,
}

/// One generation's members as a vector with a fixed layout: compiled code
/// pushes and swap-removes generation-0 members inline at fixed offsets
/// (`molt_codegen_abi::object_lifecycle`, GC record). The buffer is a
/// `Vec<GcMember>`'s, taken apart; growth reassembles it.
#[repr(C)]
pub(super) struct MemberList {
    ptr: *mut GcMember,
    len: usize,
    cap: usize,
}

impl MemberList {
    pub(super) const fn new() -> Self {
        Self {
            ptr: std::ptr::NonNull::<GcMember>::dangling().as_ptr(),
            len: 0,
            cap: 0,
        }
    }

    /// Run `body` on the vector the parts describe, then take it apart again.
    fn with_vec<R>(&mut self, body: impl FnOnce(&mut Vec<GcMember>) -> R) -> R {
        // SAFETY: the parts always describe a live `Vec<GcMember>` buffer
        // (or the empty dangling one), and no other owner exists.
        let mut vec = unsafe { Vec::from_raw_parts(self.ptr, self.len, self.cap) };
        let result = body(&mut vec);
        let mut vec = std::mem::ManuallyDrop::new(vec);
        self.ptr = vec.as_mut_ptr();
        self.len = vec.len();
        self.cap = vec.capacity();
        result
    }

    #[inline(always)]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub(super) fn as_slice(&self) -> &[GcMember] {
        // SAFETY: `len` initialized members at `ptr`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    #[inline(always)]
    pub(super) fn push(&mut self, member: GcMember) {
        if self.len == self.cap {
            self.with_vec(|vec| vec.reserve(1));
        }
        // SAFETY: capacity exceeds the length.
        unsafe { self.ptr.add(self.len).write(member) };
        self.len += 1;
    }

    #[inline(always)]
    pub(super) fn swap_remove(&mut self, position: usize) -> GcMember {
        assert!(position < self.len, "member position out of range");
        self.len -= 1;
        // SAFETY: both positions are within the initialized prefix.
        unsafe {
            let member = self.ptr.add(position).read();
            if position != self.len {
                self.ptr.add(position).write(self.ptr.add(self.len).read());
            }
            member
        }
    }

    pub(super) fn extend_from_slice(&mut self, members: &[GcMember]) {
        self.with_vec(|vec| vec.extend_from_slice(members));
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
    }

    pub(super) fn take(&mut self) -> Self {
        std::mem::replace(self, Self::new())
    }
}

impl Drop for MemberList {
    fn drop(&mut self) {
        self.with_vec(|vec| std::mem::take(vec));
    }
}

/// A runtime's tracked-object membership. Collection snapshots sort members by
/// ordinal, so vector order (which swap-removal permutes) never reaches
/// finalizer, weakref or clear order. The member lists, the ordinal counter and
/// the watch word lead a fixed layout that compiled code reads and writes
/// inline (`molt_codegen_abi::object_lifecycle`, the GC record).
#[repr(C)]
pub(crate) struct Membership {
    members: [MemberList; GENERATION_LISTS],
    next_ordinal: u64,
    /// Nonzero while a retirement watch is armed: compiled code then leaves
    /// every untrack to the runtime, which counts watched retirements.
    watch_armed: u64,
    // Retirement watch for one collection: the sorted ordinals of its first
    // unreachable members, and how many of them left membership since armed.
    watched: Vec<u64>,
    watched_retired: usize,
}

// The GC record is the 64-bit layout. A target with narrower pointers lends no
// record (`state::runtime_state::bind_execution_state`), so compiled code
// leaves tracking to the runtime there.
#[cfg(target_pointer_width = "64")]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<GcMember>() == 16 && offset_of!(GcMember, ordinal) == 8);
    assert!(size_of::<MemberList>() == 24);
    assert!(offset_of!(MemberList, ptr) == 0);
    assert!(offset_of!(MemberList, len) == 8);
    assert!(offset_of!(MemberList, cap) == 16);
    assert!(offset_of!(Membership, members) == 0);
    use molt_codegen_abi::object_lifecycle::{
        GC_GENERATION_LISTS, GC_MEMBER_BYTES, GC_MEMBER_LIST_BYTES, GC_RECORD_MEMBERS_OFFSET,
        GC_RECORD_NEXT_ORDINAL_OFFSET, GC_RECORD_WATCH_ARMED_OFFSET,
    };
    assert!(GENERATION_LISTS == GC_GENERATION_LISTS as usize);
    assert!(size_of::<GcMember>() == GC_MEMBER_BYTES as usize);
    assert!(size_of::<MemberList>() == GC_MEMBER_LIST_BYTES as usize);
    assert!(
        offset_of!(Membership, next_ordinal)
            == (GC_RECORD_NEXT_ORDINAL_OFFSET - GC_RECORD_MEMBERS_OFFSET) as usize
    );
    assert!(
        offset_of!(Membership, watch_armed)
            == (GC_RECORD_WATCH_ARMED_OFFSET - GC_RECORD_MEMBERS_OFFSET) as usize
    );
};

impl Membership {
    pub(crate) const fn new() -> Self {
        Self {
            members: [const { MemberList::new() }; GENERATION_LISTS],
            next_ordinal: 1,
            watch_armed: 0,
            watched: Vec::new(),
            watched_retired: 0,
        }
    }

    /// Take the next deterministic allocation ordinal.
    #[inline]
    pub(super) fn take_ordinal(&mut self) -> u64 {
        let ordinal = self.next_ordinal;
        self.next_ordinal = ordinal
            .checked_add(1)
            .expect("GC allocation ordinal exhausted");
        ordinal
    }

    /// Enroll `slot` in the youngest generation, or report it already tracked.
    /// An immortal C root is never enrolled. The ordinal is taken only for a
    /// new member.
    ///
    /// # Safety
    /// `slot` is a live object whose allocation reserved a GC head.
    #[inline]
    pub(super) unsafe fn admit(
        &mut self,
        slot: PtrSlot,
        immortal_root: bool,
    ) -> GcMembershipAdmission {
        if let Some((generation, position)) = unsafe { GcHead::of(slot) }.place() {
            self.assert_member(generation, position, slot);
            return GcMembershipAdmission::AlreadyTracked;
        }
        if immortal_root {
            return GcMembershipAdmission::ImmortalRoot;
        }
        let ordinal = self.take_ordinal();
        unsafe { self.push(Generation::YOUNGEST, GcMember { slot, ordinal }) };
        GcMembershipAdmission::Inserted
    }

    /// Remove `slot` from membership; its ordinal when it was tracked.
    ///
    /// # Safety
    /// `slot` is a live object whose allocation reserved a GC head.
    #[inline]
    pub(super) unsafe fn remove(&mut self, slot: PtrSlot) -> Option<u64> {
        let head = unsafe { GcHead::of(slot) };
        let (generation, position) = head.place()?;
        let member = unsafe { self.detach(generation, position, slot) };
        head.clear();
        if !self.watched.is_empty() && self.watched.binary_search(&member.ordinal).is_ok() {
            self.watched_retired += 1;
        }
        Some(member.ordinal)
    }

    /// Move every member of `from` to `to`, keeping ordinals.
    pub(super) fn move_generation(&mut self, from: Generation, to: Generation) {
        if from == to {
            return;
        }
        let mut moved = self.members[from.list()].take();
        let list = &mut self.members[to.list()];
        let base = list.len();
        list.extend_from_slice(moved.as_slice());
        for (offset, member) in moved.as_slice().iter().enumerate() {
            // SAFETY: every member is a live, head-bearing tracked object.
            unsafe { GcHead::of(member.slot) }.set_place(to, base + offset);
        }
        // Keep the drained list's capacity for the generation's next members.
        moved.clear();
        self.members[from.list()] = moved;
    }

    /// Whether `slot` is tracked, read from its head alone.
    ///
    /// # Safety
    /// `slot` is a live object whose allocation reserved a GC head.
    #[inline]
    pub(super) unsafe fn is_tracked(slot: PtrSlot) -> bool {
        unsafe { GcHead::of(slot) }.place().is_some()
    }

    /// `slot`'s generation and position in its member list, read from its head
    /// alone; `None` when it is untracked.
    ///
    /// # Safety
    /// `slot` is a live object whose allocation reserved a GC head.
    #[inline(always)]
    pub(super) unsafe fn place(slot: PtrSlot) -> Option<(Generation, usize)> {
        unsafe { GcHead::of(slot) }.place()
    }

    /// The ordinal of `slot`'s current tracking lifetime.
    ///
    /// # Safety
    /// `slot` is a live object whose allocation reserved a GC head.
    pub(super) unsafe fn ordinal(&self, slot: PtrSlot) -> Option<u64> {
        let (generation, position) = unsafe { GcHead::of(slot) }.place()?;
        self.assert_member(generation, position, slot);
        Some(self.members[generation.list()].as_slice()[position].ordinal)
    }

    /// Untrack every member and release all capacity, for embedded-runtime
    /// teardown; returns how many members there were. Objects that outlive
    /// their runtime (process-held values a later teardown retires) then read
    /// "untracked" instead of naming a dropped list.
    pub(super) fn release_all(&mut self) -> usize {
        let released = self.len();
        for list in &self.members {
            for member in list.as_slice() {
                // SAFETY: every member is a live, head-bearing tracked object.
                unsafe { GcHead::of(member.slot) }.clear();
            }
        }
        *self = Self::new();
        released
    }

    /// The exact members of one generation, in no particular order.
    #[inline]
    pub(super) fn members(&self, generation: Generation) -> &[GcMember] {
        self.members[generation.list()].as_slice()
    }

    pub(super) fn len(&self) -> usize {
        self.members.iter().map(MemberList::len).sum()
    }

    /// Reserve the retirement watch for a collection of `candidates` members,
    /// before any callback can run.
    pub(super) fn try_reserve_watch(&mut self, candidates: usize) -> bool {
        debug_assert!(self.watched.is_empty(), "retirement watch already armed");
        self.watched.capacity() >= candidates || self.watched.try_reserve_exact(candidates).is_ok()
    }

    /// Arm the retirement watch over `(slot, ordinal)` lifetimes, which the
    /// reservation covers. A lifetime that already ended is not watched;
    /// returns how many had.
    ///
    /// # Safety
    /// Every slot is a live, head-bearing object.
    pub(super) unsafe fn arm_watch(
        &mut self,
        lifetimes: impl Iterator<Item = (PtrSlot, u64)>,
    ) -> usize {
        debug_assert!(self.watched.is_empty(), "retirement watch already armed");
        let mut ended = 0;
        for (slot, ordinal) in lifetimes {
            if unsafe { self.ordinal(slot) } != Some(ordinal) {
                ended += 1;
                continue;
            }
            assert!(
                self.watched.len() < self.watched.capacity(),
                "retirement watch exceeded its reservation"
            );
            self.watched.push(ordinal);
        }
        self.watched.sort_unstable();
        self.watched_retired = 0;
        self.watch_armed = 1;
        ended
    }

    /// Disarm the watch; how many watched members left membership.
    pub(super) fn disarm_watch(&mut self) -> usize {
        self.watched.clear();
        self.watch_armed = 0;
        std::mem::take(&mut self.watched_retired)
    }

    #[inline(always)]
    fn assert_member(&self, generation: Generation, position: usize, slot: PtrSlot) {
        // A head that names another object's entry means the object belongs
        // to another runtime, or membership is corrupt.
        assert!(
            self.members[generation.list()]
                .as_slice()
                .get(position)
                .is_some_and(|member| member.slot == slot),
            "GC membership crossed runtime ownership"
        );
    }

    /// # Safety
    /// `member.slot` is a live, untracked, head-bearing object.
    #[inline(always)]
    unsafe fn push(&mut self, generation: Generation, member: GcMember) {
        let list = &mut self.members[generation.list()];
        let position = list.len();
        list.push(member);
        unsafe { GcHead::of(member.slot) }.set_place(generation, position);
    }

    // Swap-remove the member at `position`, then repoint the member that moved
    // into its place.
    #[inline(always)]
    unsafe fn detach(
        &mut self,
        generation: Generation,
        position: usize,
        slot: PtrSlot,
    ) -> GcMember {
        self.assert_member(generation, position, slot);
        let list = &mut self.members[generation.list()];
        let member = list.swap_remove(position);
        if let Some(moved) = list.as_slice().get(position) {
            unsafe { GcHead::of(moved.slot) }.set_place(generation, position);
        }
        member
    }

    /// Check I1 for every member. Test support.
    #[cfg(test)]
    pub(super) fn assert_membership_exact(&self) {
        for generation in Generation::all() {
            for (position, member) in self.members(generation).iter().enumerate() {
                assert_eq!(
                    unsafe { GcHead::of(member.slot) }.place(),
                    Some((generation, position)),
                    "member head is stale"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // Deterministic xorshift: the sequence is a fixture, not a property search.
    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    // Head, header and one payload word per object.
    const OBJECT_WORDS: usize = (GC_HEAD_BYTES + std::mem::size_of::<MoltHeader>()) / 8 + 1;

    struct Objects(Vec<[u64; OBJECT_WORDS]>);

    impl Objects {
        fn new(count: usize) -> Self {
            Self(vec![[0; OBJECT_WORDS]; count])
        }

        fn slot(&mut self, index: usize) -> PtrSlot {
            PtrSlot(unsafe { self.0[index].as_mut_ptr().add(OBJECT_WORDS - 1) }.cast())
        }
    }

    /// Every operation keeps heads and generation lists equal to a plain
    /// model: index -> (generation, ordinal).
    #[test]
    fn heads_and_generation_lists_stay_exact_under_track_untrack_retrack_and_moves() {
        let mut objects = Objects::new(257);
        let mut membership = Membership::new();
        let mut model: BTreeMap<usize, (u8, u64)> = BTreeMap::new();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let index = (next(&mut state) % 257) as usize;
            let slot = objects.slot(index);
            match next(&mut state) % 4 {
                0 | 1 => {
                    let expected_ordinal = membership.next_ordinal;
                    let admission = unsafe { membership.admit(slot, false) };
                    match model.get(&index) {
                        Some(_) => assert_eq!(admission, GcMembershipAdmission::AlreadyTracked),
                        None => {
                            assert_eq!(admission, GcMembershipAdmission::Inserted);
                            model.insert(index, (0, expected_ordinal));
                        }
                    }
                }
                2 => {
                    let removed = unsafe { membership.remove(slot) };
                    assert_eq!(removed, model.remove(&index).map(|(_, ordinal)| ordinal));
                }
                _ => {
                    let from = Generation::new((next(&mut state) % 4) as u8);
                    let to = Generation::new((next(&mut state) % 4) as u8);
                    membership.move_generation(from, to);
                    for entry in model.values_mut() {
                        if entry.0 == from.raw() {
                            entry.0 = to.raw();
                        }
                    }
                }
            }
            membership.assert_membership_exact();
            assert_eq!(
                unsafe { Membership::is_tracked(slot) },
                model.contains_key(&index)
            );
            assert_eq!(
                unsafe { membership.ordinal(slot) },
                model.get(&index).map(|(_, ordinal)| *ordinal)
            );
        }
        for generation in Generation::all() {
            let mut listed: Vec<u64> = membership
                .members(generation)
                .iter()
                .map(|member| member.ordinal)
                .collect();
            listed.sort_unstable();
            let mut expected: Vec<u64> = model
                .values()
                .filter(|(g, _)| *g == generation.raw())
                .map(|(_, ordinal)| *ordinal)
                .collect();
            expected.sort_unstable();
            assert_eq!(listed, expected, "generation {generation:?} membership");
        }
        assert_eq!(membership.len(), model.len());
    }

    #[test]
    fn an_immortal_root_is_never_enrolled_and_ordinals_are_taken_only_on_insert() {
        let mut objects = Objects::new(1);
        let slot = objects.slot(0);
        let mut membership = Membership::new();
        assert_eq!(
            unsafe { membership.admit(slot, true) },
            GcMembershipAdmission::ImmortalRoot
        );
        assert_eq!(membership.next_ordinal, 1);
        assert!(!unsafe { Membership::is_tracked(slot) });
        assert_eq!(
            unsafe { membership.admit(slot, false) },
            GcMembershipAdmission::Inserted
        );
        assert_eq!(unsafe { membership.ordinal(slot) }, Some(1));
        assert_eq!(
            unsafe { membership.admit(slot, true) },
            GcMembershipAdmission::AlreadyTracked,
            "an enrolled object stays enrolled"
        );
        assert_eq!(membership.next_ordinal, 2);
    }

    /// A watched member counts once when it leaves; a retracked object is a
    /// new lifetime and is not watched.
    #[test]
    fn the_retirement_watch_counts_watched_lifetimes_that_leave() {
        let mut objects = Objects::new(3);
        let slots = [objects.slot(0), objects.slot(1), objects.slot(2)];
        let mut membership = Membership::new();
        for slot in slots {
            unsafe { membership.admit(slot, false) };
        }
        let ordinals: Vec<u64> = slots
            .iter()
            .map(|slot| unsafe { membership.ordinal(*slot) }.unwrap())
            .collect();
        assert!(membership.try_reserve_watch(3));
        unsafe { membership.remove(slots[2]) };
        unsafe { membership.admit(slots[2], false) };
        let ended = unsafe {
            membership.arm_watch(
                [
                    (slots[1], ordinals[1]),
                    (slots[0], ordinals[0]),
                    (slots[2], ordinals[2]),
                ]
                .into_iter(),
            )
        };
        assert_eq!(ended, 1, "a retracked object is a new lifetime");
        unsafe {
            membership.remove(slots[0]);
            membership.admit(slots[0], false);
            membership.remove(slots[0]);
            membership.remove(slots[2]);
        }
        assert_eq!(membership.disarm_watch(), 1);
        unsafe { membership.remove(slots[1]) };
        assert_eq!(
            membership.disarm_watch(),
            0,
            "a disarmed watch counts nothing"
        );
    }

    /// A head that names another runtime's entry fails loud.
    #[test]
    fn a_foreign_head_fails_closed() {
        let mut objects = Objects::new(1);
        let slot = objects.slot(0);
        let mut owner = Membership::new();
        unsafe { owner.admit(slot, false) };
        let mut other = Membership::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            other.remove(slot)
        }));
        assert!(result.is_err());
        assert!(unsafe { Membership::is_tracked(slot) });
    }
}
