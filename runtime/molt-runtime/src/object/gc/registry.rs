//! Tracked-object membership for the cycle collector.
//!
//! The address map is the authority for each tracked runtime object's owner,
//! allocation ordinal and generation. Beside it, each generation keeps the exact
//! list of its members, so a young collection visits only young objects, as
//! CPython's one intrusive list per generation does. Every membership change goes
//! through this module, whose fields are private: `members[g]` always holds
//! exactly the entries whose generation is `g` (invariant I1), and each entry
//! records its own position in that list.

use super::{GcMembershipAdmission, NUM_GENERATIONS, PERMANENT_GENERATION, RegistryAddressHash};
use crate::object::PtrSlot;
use std::collections::{HashMap, hash_map::Entry};

const GENERATION_LISTS: usize = NUM_GENERATIONS + 1;

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

    #[inline]
    fn list(self) -> usize {
        usize::from(self.0)
    }

    /// Every generation, youngest first, the permanent one last.
    pub(super) fn all() -> impl Iterator<Item = Self> {
        (0..=PERMANENT_GENERATION).map(Self)
    }
}

#[derive(Clone, Copy)]
pub(super) struct TrackedEntry {
    pub(super) runtime_owner: usize,
    pub(super) allocation_id: u64,
    generation: Generation,
    // Index of this entry in `members[generation]`.
    position: u32,
}

/// CPython links each tracked object into a generation list through a 16-byte
/// `PyGC_Head` before the object, so track and untrack are four pointer
/// writes. Molt's 24-byte header has no free word or flag bit, and
/// `docs/design/foundation/rc_gc_redesign.md` keeps both the GC head and
/// registry identity out of the header and its aux word. The registry is
/// therefore keyed by address, and both maps hash that address with one folded
/// multiply ([`RegistryAddressHasher`](super::RegistryAddressHasher)) instead of
/// keyed SipHash. The per-generation member lists take the place of CPython's
/// generation lists.
pub(super) struct TrackedRegistryShard {
    entries: HashMap<PtrSlot, TrackedEntry, RegistryAddressHash>,
    members: [Vec<PtrSlot>; GENERATION_LISTS],
    pub(super) native_nodes: HashMap<usize, super::NativeTrackedEntry, RegistryAddressHash>,
}

#[inline]
fn position_at(length: usize) -> u32 {
    u32::try_from(length).expect("GC generation member list exceeds u32 positions")
}

impl TrackedRegistryShard {
    pub(super) const fn new() -> Self {
        Self {
            entries: HashMap::with_hasher(RegistryAddressHash::new()),
            members: [const { Vec::new() }; GENERATION_LISTS],
            native_nodes: HashMap::with_hasher(RegistryAddressHash::new()),
        }
    }

    /// Enroll `slot` in the youngest generation, or report it already tracked
    /// by `owner`. An immortal C root is never enrolled. The ordinal is taken
    /// only for a new entry.
    #[inline]
    pub(super) fn admit(
        &mut self,
        slot: PtrSlot,
        owner: usize,
        immortal_root: bool,
        ordinal: impl FnOnce() -> u64,
    ) -> GcMembershipAdmission {
        match self.entries.entry(slot) {
            Entry::Occupied(entry) => {
                assert_eq!(
                    entry.get().runtime_owner,
                    owner,
                    "tracked object crossed runtime ownership"
                );
                GcMembershipAdmission::AlreadyTracked
            }
            Entry::Vacant(entry) => {
                if immortal_root {
                    return GcMembershipAdmission::ImmortalRoot;
                }
                let members = &mut self.members[Generation::YOUNGEST.list()];
                entry.insert(TrackedEntry {
                    runtime_owner: owner,
                    allocation_id: ordinal(),
                    generation: Generation::YOUNGEST,
                    position: position_at(members.len()),
                });
                members.push(slot);
                GcMembershipAdmission::Inserted
            }
        }
    }

    /// Remove `slot` from membership, keeping its generation list exact.
    #[inline]
    pub(super) fn remove(&mut self, slot: PtrSlot) -> Option<TrackedEntry> {
        let entry = self.entries.remove(&slot)?;
        self.detach_member(entry.generation, entry.position);
        Some(entry)
    }

    /// Move `slot` to `generation`; false when it is not tracked.
    pub(super) fn set_generation(&mut self, slot: PtrSlot, generation: Generation) -> bool {
        let Some(entry) = self.entries.get(&slot).copied() else {
            return false;
        };
        if entry.generation == generation {
            return true;
        }
        self.detach_member(entry.generation, entry.position);
        let members = &mut self.members[generation.list()];
        let position = position_at(members.len());
        members.push(slot);
        let entry = self
            .entries
            .get_mut(&slot)
            .expect("generation change keeps the entry");
        entry.generation = generation;
        entry.position = position;
        true
    }

    // Swap-remove the member at `position`, then repoint the member that moved
    // into its place.
    #[inline]
    fn detach_member(&mut self, generation: Generation, position: u32) {
        let members = &mut self.members[generation.list()];
        let position = position as usize;
        members.swap_remove(position);
        if let Some(&moved) = members.get(position) {
            self.entries
                .get_mut(&moved)
                .expect("generation member list names a tracked entry")
                .position = position_at(position);
        }
    }

    #[inline]
    pub(super) fn get(&self, slot: &PtrSlot) -> Option<&TrackedEntry> {
        self.entries.get(slot)
    }

    #[inline]
    pub(super) fn contains(&self, slot: &PtrSlot) -> bool {
        self.entries.contains_key(slot)
    }

    /// The exact members of one generation, in no particular order.
    #[inline]
    pub(super) fn members(&self, generation: Generation) -> &[PtrSlot] {
        &self.members[generation.list()]
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = (&PtrSlot, &TrackedEntry)> {
        self.entries.iter()
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check I1 and each entry's recorded position. Test support.
    #[cfg(test)]
    pub(super) fn assert_membership_exact(&self) {
        let listed: usize = self.members.iter().map(Vec::len).sum();
        assert_eq!(
            listed,
            self.entries.len(),
            "generation lists and map disagree"
        );
        for generation in Generation::all() {
            for (position, slot) in self.members(generation).iter().enumerate() {
                let entry = self.entries.get(slot).expect("listed member is tracked");
                assert_eq!(
                    entry.generation, generation,
                    "member listed under the wrong generation"
                );
                assert_eq!(
                    entry.position as usize, position,
                    "member position is stale"
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

    fn slot(index: u64) -> PtrSlot {
        PtrSlot(((index + 1) * 16) as usize as *mut u8)
    }

    /// Every operation keeps each generation's list equal to the map: the
    /// model is a plain slot -> (owner, generation) table.
    #[test]
    fn generation_lists_stay_exact_under_track_untrack_retrack_and_moves() {
        let mut shard = TrackedRegistryShard::new();
        let mut model: BTreeMap<u64, (usize, u8)> = BTreeMap::new();
        let mut ordinal = 0u64;
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let index = next(&mut state) % 257;
            let owner = 1 + (index % 2) as usize; // two runtimes share the shard
            match next(&mut state) % 4 {
                0 | 1 => {
                    let admission = shard.admit(slot(index), owner, false, || {
                        ordinal += 1;
                        ordinal
                    });
                    match model.get(&index) {
                        Some(_) => assert_eq!(admission, GcMembershipAdmission::AlreadyTracked),
                        None => {
                            assert_eq!(admission, GcMembershipAdmission::Inserted);
                            model.insert(index, (owner, 0));
                        }
                    }
                }
                2 => {
                    let removed = shard.remove(slot(index));
                    assert_eq!(removed.is_some(), model.remove(&index).is_some());
                }
                _ => {
                    let generation =
                        (next(&mut state) % (u64::from(PERMANENT_GENERATION) + 1)) as u8;
                    let moved = shard.set_generation(slot(index), Generation::new(generation));
                    assert_eq!(moved, model.contains_key(&index));
                    if let Some(entry) = model.get_mut(&index) {
                        entry.1 = generation;
                    }
                }
            }
            shard.assert_membership_exact();
        }
        for generation in Generation::all() {
            let mut listed: Vec<u64> = shard
                .members(generation)
                .iter()
                .map(|slot| (slot.0 as usize / 16 - 1) as u64)
                .collect();
            listed.sort_unstable();
            let expected: Vec<u64> = model
                .iter()
                .filter(|(_, (_, g))| *g == generation.raw())
                .map(|(index, _)| *index)
                .collect();
            assert_eq!(listed, expected, "generation {generation:?} membership");
        }
    }

    #[test]
    fn an_immortal_root_is_never_enrolled_and_ordinals_are_taken_only_on_insert() {
        let mut shard = TrackedRegistryShard::new();
        let mut taken = 0;
        let admission = shard.admit(slot(1), 1, true, || {
            taken += 1;
            7
        });
        assert_eq!(admission, GcMembershipAdmission::ImmortalRoot);
        assert_eq!(taken, 0);
        assert!(!shard.contains(&slot(1)));
        assert_eq!(
            shard.admit(slot(1), 1, false, || 9),
            GcMembershipAdmission::Inserted
        );
        assert_eq!(
            shard.get(&slot(1)).map(|entry| entry.allocation_id),
            Some(9)
        );
        assert_eq!(
            shard.admit(slot(1), 1, false, || unreachable!()),
            GcMembershipAdmission::AlreadyTracked
        );
        assert_eq!(shard.members(Generation::YOUNGEST), &[slot(1)]);
    }
}
