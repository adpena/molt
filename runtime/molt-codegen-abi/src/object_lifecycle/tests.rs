//! The lifecycle generators run on the interpreting builder over byte memory
//! holding real headers and plan words. The oracles are the runtime's own
//! rules written directly: its planned allocation's header (molt-runtime
//! `alloc_planned_instance`: type id and an owner count of 1, published
//! flags, the size class, an inline owned class word, empty payload) and its
//! terminal fast path's admission (`free_plain_leaf`) and its GC membership
//! (molt-runtime `object/gc/membership.rs`: a tracked object's head names
//! its generation and position; admission pushes, untrack swap-removes and
//! rewrites the moved member's head; count 0 counts allocations minus frees).

use super::*;
use crate::test_machine::{Event, Machine};
use crate::{
    HEADER_AUX_KIND_SIDECAR, HEADER_FLAG_GC_HEAD, MOLT_REFCOUNT_ATOMIC, TAG_INT, TYPE_ID_STRING,
};

const CLASS: u64 = 0x1_0000;
const BASE: u64 = 0x2_0000;
const OBJECT: u64 = BASE + HEADER_SIZE_BYTES as u64;
const COLD: u64 = 0xC01D;
const BYTES: u64 = 32;
const SIZE_CLASS: u64 = 3;
const GC: u64 = 0x4_0000;
const MEMBERS: u64 = 0x5_0000;
const MISSING_SLOT: u64 = 0x6_0000;
const MISSING: u64 = 0x7_0000;
/// A `Point` instance: two fields and the dictionary word, 48 object bytes.
const POINT_BYTES: u64 = 48;
const POINT_SIZE_CLASS: u64 = 5;
/// A tracked object's data address: past its head and header.
const TRACKED: u64 = BASE + (GC_HEAD_BYTES + HEADER_SIZE_BYTES) as u64;

fn boxed(address: u64) -> u64 {
    QNAN | TAG_PTR | address
}

/// The plan of the root `object`: present, plain, inline-allocated,
/// untracked, no fields, a 32-byte size class.
fn root_plan() -> u64 {
    PLAN_PRESENT
        | PLAN_PLAIN
        | PLAN_INLINE_ALLOC
        | PLAN_UNTRACKED
        | (SIZE_CLASS << PLAN_SIZE_CLASS_SHIFT)
        | (BYTES << PLAN_OBJECT_BYTES_SHIFT)
}

/// The plan of a heap class with two fields: tracked instances.
fn point_plan() -> u64 {
    PLAN_PRESENT
        | PLAN_PLAIN
        | PLAN_INLINE_ALLOC
        | (2 << PLAN_FIELD_WORDS_SHIFT)
        | (POINT_SIZE_CLASS << PLAN_SIZE_CLASS_SHIFT)
        | (POINT_BYTES << PLAN_OBJECT_BYTES_SHIFT)
}

fn words(profile: u64) -> LifecycleWords<u64> {
    LifecycleWords {
        profile_enabled: profile,
        gc_record: GC,
        missing_slot: MISSING_SLOT,
    }
}

fn head_word(generation: u64, position: u64) -> u64 {
    (position << GC_HEAD_POSITION_SHIFT)
        | (generation << GC_HEAD_GENERATION_SHIFT)
        | GC_HEAD_TRACKED
}

impl Machine {
    /// A GC record with `length` members of generation 0 (the members' data
    /// addresses are `0x9_0000 + 0x100 * i` with ordinals `i + 1`), `capacity`
    /// slots, count 0 `count` and threshold 0 `threshold`.
    fn gc(&mut self, length: u64, capacity: u64, count: u64, threshold: u64) {
        self.write_word(GC, GC_RECORD_THRESHOLD0_OFFSET, threshold);
        self.write_word(GC, GC_RECORD_COUNT0_OFFSET, count);
        for generation in 0..GC_GENERATION_LISTS as u64 {
            let list = GC_RECORD_MEMBERS_OFFSET + (generation as i32) * GC_MEMBER_LIST_BYTES;
            let (pointer, len, cap) = if generation == 0 {
                (MEMBERS, length, capacity)
            } else {
                (0x8_0000, 0, 0)
            };
            self.write_word(GC, list, pointer);
            self.write_word(GC, list + 8, len);
            self.write_word(GC, list + 16, cap);
        }
        self.write_word(GC, GC_RECORD_NEXT_ORDINAL_OFFSET, 42);
        self.write_word(GC, GC_RECORD_WATCH_ARMED_OFFSET, 0);
        for index in 0..length {
            let slot = 0x9_0000 + 0x100 * index;
            self.member(0, index, slot, index + 1);
        }
        self.write_word(MISSING_SLOT, 0, MISSING);
    }

    /// Member `position` of generation `generation`'s list is `slot`.
    fn member(&mut self, generation: u64, position: u64, slot: u64, ordinal: u64) {
        let list = GC_RECORD_MEMBERS_OFFSET + (generation as i32) * GC_MEMBER_LIST_BYTES;
        let members = self.read_bytes(GC, list, 8);
        self.write_word(members + 16 * position, 0, slot);
        self.write_word(members + 16 * position, 8, ordinal);
        self.write_word(
            slot,
            -(HEADER_SIZE_BYTES + GC_HEAD_BYTES),
            head_word(generation, position),
        );
    }

    fn list_len(&self, generation: u64) -> u64 {
        let list = GC_RECORD_MEMBERS_OFFSET + (generation as i32) * GC_MEMBER_LIST_BYTES;
        self.read_bytes(GC, list + 8, 8)
    }

    fn class(&mut self, plan: u64) {
        self.write_bytes(CLASS, HEADER_TYPE_ID_OFFSET, TYPE_ID_TYPE.into(), 4);
        self.write_word(CLASS, CLASS_LIFECYCLE_PLAN_OFFSET, plan);
    }

    fn allocate(&mut self, class: u64, profile: u64, allocator: u64) -> u64 {
        self.allocate_from(class, PlanSource::Guarded, profile, allocator)
    }

    fn allocate_from(&mut self, class: u64, plan: PlanSource, profile: u64, allocator: u64) -> u64 {
        self.allocate_with(class, plan, words(profile), allocator)
    }

    fn allocate_with(
        &mut self,
        class: u64,
        plan: PlanSource,
        words: LifecycleWords<u64>,
        allocator: u64,
    ) -> u64 {
        generate_object_alloc(
            self,
            class,
            plan,
            words,
            ListLocking::Gil,
            |m, bytes| {
                if m.bailed {
                    return 0;
                }
                m.events.push(Event::Call("heap_alloc", std::vec![bytes]));
                allocator
            },
            |m| {
                m.events.push(Event::Cold);
                COLD
            },
        )
    }

    /// A live object at `OBJECT` with `flags`, `aux_kind` and `class_word`.
    fn dying(&mut self, type_id: u32, flags: u32, aux_kind: u16, class_word: u64) {
        self.write_bytes(OBJECT, HEADER_TYPE_ID_OFFSET, type_id.into(), 4);
        self.write_bytes(OBJECT, HEADER_FLAGS_OFFSET, flags.into(), 4);
        self.write_bytes(OBJECT, HEADER_FLAGS_OFFSET + 4, SIZE_CLASS, 2);
        self.write_bytes(OBJECT, HEADER_AUX_KIND_OFFSET, aux_kind.into(), 2);
        self.write_word(OBJECT, HEADER_AUX_OFFSET, class_word);
    }

    fn release_terminal(&mut self, profile: u64, terminal_release: u64) {
        self.release_at(OBJECT, profile, terminal_release);
    }

    fn release_at(&mut self, object: u64, profile: u64, terminal_release: u64) {
        generate_release_terminal(
            self,
            boxed(object),
            words(profile),
            terminal_release,
            ListLocking::Gil,
            |m, object| {
                if !m.bailed {
                    m.events.push(Event::Call("heap_free", std::vec![object]));
                }
            },
            |m| m.events.push(Event::Cold),
        );
    }
}

#[test]
fn header_image_matches_the_runtime_allocation() {
    let [first, second] = planned_header_words(SIZE_CLASS as u16, HEADER_FLAG_GC_HEAD);
    assert_eq!(first as u32, TYPE_ID_OBJECT);
    assert_eq!((first >> 32) as u32, 1, "an owner count of 1");
    assert_eq!(second as u32, HEADER_FLAG_GC_HEAD);
    assert_eq!((second >> 32) as u16, SIZE_CLASS as u16);
    assert_eq!((second >> 48) as u16, HEADER_AUX_KIND_CLASS_INLINE);
}

#[test]
fn root_object_allocates_inline_with_its_header_and_class() {
    let mut m = Machine::default();
    m.class(root_plan());
    let object = m.allocate(boxed(CLASS), 0, BASE);
    assert_eq!(object, boxed(OBJECT));
    assert_eq!(
        m.events,
        std::vec![
            Event::Call("heap_alloc", std::vec![BYTES]),
            Event::Retain(boxed(CLASS)),
        ]
    );
    let [first, second] = planned_header_words(SIZE_CLASS as u16, 0);
    assert_eq!(m.read_bytes(BASE, 0, 8), first);
    assert_eq!(m.read_bytes(BASE, 8, 8), second);
    assert_eq!(
        m.read_bytes(BASE, 16, 8),
        boxed(CLASS),
        "an owned class word"
    );
    // The one payload word, the dictionary word, is empty.
    assert_eq!(m.read_bytes(OBJECT, 0, 8), 0);
}

#[test]
fn allocation_takes_the_runtime_when_the_plan_or_process_refuses() {
    let refusals: [(&str, u64, u64, u64); 5] = [
        ("profile counters on", root_plan(), 1, BASE),
        ("no plan", 0, 0, BASE),
        (
            "abstract or no size class",
            root_plan() & !PLAN_INLINE_ALLOC,
            0,
            BASE,
        ),
        ("allocator refused", root_plan(), 0, 0),
        ("absent plan bit", root_plan() & !PLAN_PRESENT, 0, BASE),
    ];
    for (label, plan, profile, allocator) in refusals {
        let mut m = Machine::default();
        m.class(plan);
        assert_eq!(
            m.allocate(boxed(CLASS), profile, allocator),
            COLD,
            "{label}"
        );
        assert!(
            !m.events
                .iter()
                .any(|event| matches!(event, Event::Retain(_))),
            "{label}: no class retain on the cold path"
        );
        assert_eq!(m.events.last(), Some(&Event::Cold), "{label}");
    }
    // A class operand that is no class object (or not a pointer) is the
    // runtime's, which raises.
    let mut m = Machine::default();
    m.write_bytes(CLASS, HEADER_TYPE_ID_OFFSET, TYPE_ID_STRING.into(), 4);
    m.write_word(CLASS, CLASS_LIFECYCLE_PLAN_OFFSET, root_plan());
    assert_eq!(m.allocate(boxed(CLASS), 0, BASE), COLD);
    let mut m = Machine::default();
    assert_eq!(m.allocate(QNAN | TAG_INT | 5, 0, BASE), COLD);
    let mut m = Machine::default();
    assert_eq!(m.allocate(0, 0, BASE), COLD, "an unpublished root class");
}

#[test]
fn free_threaded_builds_take_the_runtime() {
    let mut m = Machine::default();
    m.class(root_plan());
    let value = generate_object_alloc(
        &mut m,
        boxed(CLASS),
        PlanSource::Guarded,
        words(0),
        ListLocking::FreeThreaded,
        |_, _| unreachable!("no inline allocation"),
        |_| COLD,
    );
    assert_eq!(value, COLD);
    let _ = MOLT_REFCOUNT_ATOMIC;
}

#[test]
fn an_unobserved_plain_instance_frees_inline() {
    let mut m = Machine::default();
    m.class(root_plan());
    m.dying(
        TYPE_ID_OBJECT,
        0,
        HEADER_AUX_KIND_CLASS_INLINE,
        boxed(CLASS),
    );
    m.release_terminal(0, 0);
    assert_eq!(
        m.events,
        std::vec![
            Event::Call("heap_free", std::vec![OBJECT]),
            Event::Release(boxed(CLASS)),
        ]
    );
}

/// Label, type id, flags, aux kind, class word, plan, profile word and
/// terminal-release word of one dying object.
type DeathCase = (&'static str, u32, u32, u16, u64, u64, u64, u64);

#[test]
fn an_observable_death_takes_the_runtime() {
    let finalizing = root_plan() | PLAN_FINALIZER;
    let cases: [DeathCase; 12] = [
        (
            "weakref",
            TYPE_ID_OBJECT,
            HEADER_FLAG_HAS_WEAKREF,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "c view",
            TYPE_ID_OBJECT,
            HEADER_FLAG_HAS_ABI_VIEW,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "scheduler retain",
            TYPE_ID_OBJECT,
            HEADER_FLAG_SPAWN_RETAIN,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "scoped storage",
            TYPE_ID_OBJECT,
            HEADER_FLAG_SCOPED,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "pointer field",
            TYPE_ID_OBJECT,
            HEADER_FLAG_HAS_PTRS,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "accounted without a head",
            TYPE_ID_OBJECT,
            HEADER_FLAG_GC_ACCOUNTED,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "sidecar",
            TYPE_ID_OBJECT,
            0,
            HEADER_AUX_KIND_SIDECAR,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
        (
            "borrowed class",
            TYPE_ID_OBJECT,
            0,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS) | HEADER_CLASS_WORD_BORROWED,
            root_plan(),
            0,
            0,
        ),
        (
            "finalizer",
            TYPE_ID_OBJECT,
            0,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            finalizing,
            0,
            0,
        ),
        (
            "terminal release active",
            TYPE_ID_OBJECT,
            0,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            1,
        ),
        (
            "profile counters on",
            TYPE_ID_OBJECT,
            0,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            1,
            0,
        ),
        (
            "another kind",
            TYPE_ID_STRING,
            0,
            HEADER_AUX_KIND_CLASS_INLINE,
            boxed(CLASS),
            root_plan(),
            0,
            0,
        ),
    ];
    for (label, type_id, flags, aux_kind, class_word, plan, profile, terminal) in cases {
        let mut m = Machine::default();
        m.class(plan);
        m.dying(type_id, flags, aux_kind, class_word);
        m.release_terminal(profile, terminal);
        assert_eq!(m.events, std::vec![Event::Cold], "{label}");
    }
}

#[test]
fn a_proven_plan_allocates_without_loading_or_testing_the_plan_word() {
    // No class header and no plan word in memory: a load would fail.
    let mut m = Machine::default();
    let object = m.allocate_from(boxed(CLASS), PlanSource::Proven(root_plan()), 0, BASE);
    assert_eq!(object, boxed(OBJECT));
    let [first, _] = planned_header_words(SIZE_CLASS as u16, 0);
    assert_eq!(m.read_bytes(BASE, 0, 8), first);
    // A proven plan that refuses inline allocation is the runtime's at once.
    let mut m = Machine::default();
    let refused = PlanSource::Proven(root_plan() & !PLAN_INLINE_ALLOC);
    assert_eq!(m.allocate_from(boxed(CLASS), refused, 0, BASE), COLD);
    assert_eq!(m.events, std::vec![Event::Cold]);
}

#[test]
fn a_heap_class_instance_allocates_and_joins_generation_zero() {
    let mut m = Machine::default();
    m.gc(3, 8, 10, 700);
    m.class(point_plan());
    let object = m.allocate(boxed(CLASS), 0, BASE);
    assert_eq!(object, boxed(TRACKED));
    assert_eq!(
        m.events,
        std::vec![
            Event::Call("heap_alloc", std::vec![POINT_BYTES + GC_HEAD_BYTES as u64]),
            Event::Retain(boxed(CLASS)),
        ]
    );
    let header = TRACKED - HEADER_SIZE_BYTES as u64;
    let [first, second] = planned_header_words(POINT_SIZE_CLASS as u16, TRACKED_HEADER_FLAGS);
    assert_eq!(m.read_bytes(header, 0, 8), first);
    assert_eq!(m.read_bytes(header, 8, 8), second);
    assert_eq!(m.read_bytes(header, 16, 8), boxed(CLASS));
    // Both fields hold the missing word; the dictionary word is empty.
    assert_eq!(m.read_bytes(TRACKED, 0, 8), boxed(MISSING));
    assert_eq!(m.read_bytes(TRACKED, 8, 8), boxed(MISSING));
    assert_eq!(m.read_bytes(TRACKED, 16, 8), 0);
    // Admission: the head names generation 0, position 3; the entry holds the
    // object and the next ordinal; length, ordinal and count 0 advance.
    assert_eq!(m.read_bytes(BASE, 0, 8), head_word(0, 3));
    assert_eq!(m.read_bytes(MEMBERS + 3 * 16, 0, 8), TRACKED);
    assert_eq!(m.read_bytes(MEMBERS + 3 * 16, 8, 8), 42);
    assert_eq!(m.list_len(0), 4);
    assert_eq!(m.read_bytes(GC, GC_RECORD_NEXT_ORDINAL_OFFSET, 8), 43);
    assert_eq!(m.read_bytes(GC, GC_RECORD_COUNT0_OFFSET, 8), 11);
}

#[test]
fn admission_the_runtime_must_decide_takes_the_runtime() {
    let cases: [(&str, u64, u64, u64, u64, u64); 4] = [
        ("full member list", 8, 8, 10, 700, MISSING),
        ("count at its threshold", 3, 8, 700, 700, MISSING),
        ("collection threshold 0", 3, 8, 0, 0, MISSING),
        ("missing singleton not created", 3, 8, 10, 700, 0),
    ];
    for (label, length, capacity, count, threshold, missing) in cases {
        let mut m = Machine::default();
        m.gc(length, capacity, count, threshold);
        m.write_word(MISSING_SLOT, 0, missing);
        m.class(point_plan());
        assert_eq!(m.allocate(boxed(CLASS), 0, BASE), COLD, "{label}");
        assert_eq!(
            m.events,
            std::vec![Event::Cold],
            "{label}: nothing allocated"
        );
        assert_eq!(m.list_len(0), length, "{label}: membership unchanged");
        assert_eq!(
            m.read_bytes(GC, GC_RECORD_COUNT0_OFFSET, 8),
            count,
            "{label}"
        );
    }
    // No GC record lent (a free-threaded binding) or no missing slot lent.
    for (label, gc_record, missing_slot) in [
        ("no gc record", 0, MISSING_SLOT),
        ("no missing slot", GC, 0),
    ] {
        let mut m = Machine::default();
        m.gc(3, 8, 10, 700);
        m.class(point_plan());
        let words = LifecycleWords {
            profile_enabled: 0,
            gc_record,
            missing_slot,
        };
        assert_eq!(
            m.allocate_with(boxed(CLASS), PlanSource::Guarded, words, BASE),
            COLD,
            "{label}"
        );
    }
}

impl Machine {
    /// A dying tracked `Point` at member `position` of `generation`.
    fn dying_member(&mut self, object: u64, generation: u64, position: u64) {
        self.write_bytes(object, HEADER_TYPE_ID_OFFSET, TYPE_ID_OBJECT.into(), 4);
        self.write_bytes(object, HEADER_FLAGS_OFFSET, TRACKED_HEADER_FLAGS.into(), 4);
        self.write_bytes(object, HEADER_FLAGS_OFFSET + 4, POINT_SIZE_CLASS, 2);
        self.write_bytes(
            object,
            HEADER_AUX_KIND_OFFSET,
            HEADER_AUX_KIND_CLASS_INLINE.into(),
            2,
        );
        self.write_word(object, HEADER_AUX_OFFSET, boxed(CLASS));
        self.member(generation, position, object, 100);
    }

    fn freed(&self) -> Option<u64> {
        self.events.iter().find_map(|event| match event {
            Event::Call("heap_free", args) => Some(args[0]),
            _ => None,
        })
    }
}

#[test]
fn a_member_leaves_its_list_and_the_moved_member_follows() {
    let mut m = Machine::default();
    m.gc(3, 8, 10, 700);
    m.class(point_plan());
    m.dying_member(TRACKED, 0, 1);
    m.release_at(TRACKED, 0, 0);
    assert_eq!(m.freed(), Some(TRACKED));
    assert_eq!(m.events.last(), Some(&Event::Release(boxed(CLASS))));
    // The last member (position 2) moved into position 1 and its head says so.
    let moved = 0x9_0000 + 0x200;
    assert_eq!(m.list_len(0), 2);
    assert_eq!(m.read_bytes(MEMBERS + 16, 0, 8), moved);
    assert_eq!(m.read_bytes(MEMBERS + 16, 8, 8), 3);
    assert_eq!(
        m.read_bytes(moved, -(HEADER_SIZE_BYTES + GC_HEAD_BYTES), 8),
        head_word(0, 1)
    );
    assert_eq!(
        m.read_bytes(BASE, 0, 8),
        0,
        "the freed head reads untracked"
    );
    assert_eq!(m.read_bytes(GC, GC_RECORD_COUNT0_OFFSET, 8), 9);
}

#[test]
fn the_last_member_of_an_older_generation_leaves_without_a_move() {
    let mut m = Machine::default();
    m.gc(0, 8, 0, 700);
    // Generation 2 holds one member.
    let list = GC_RECORD_MEMBERS_OFFSET + 2 * GC_MEMBER_LIST_BYTES;
    m.write_word(GC, list, 0x5_8000);
    m.write_word(GC, list + 8, 1);
    m.write_word(GC, list + 16, 4);
    m.class(point_plan());
    m.dying_member(TRACKED, 2, 0);
    m.release_at(TRACKED, 0, 0);
    assert!(m.freed().is_some());
    assert_eq!(m.list_len(2), 0);
    // Count 0 never falls below zero.
    assert_eq!(m.read_bytes(GC, GC_RECORD_COUNT0_OFFSET, 8), 0);
}

#[test]
fn an_explicitly_untracked_accounted_object_only_returns_its_count() {
    let mut m = Machine::default();
    m.gc(1, 8, 5, 700);
    m.class(point_plan());
    m.dying_member(TRACKED, 0, 0);
    // A C-API untrack cleared the head; the allocation stays accounted.
    m.write_word(BASE, 0, 0);
    m.write_word(GC, GC_RECORD_MEMBERS_OFFSET + 8, 0);
    m.release_at(TRACKED, 0, 0);
    assert_eq!(m.freed(), Some(TRACKED));
    assert_eq!(m.list_len(0), 0);
    assert_eq!(m.read_bytes(GC, GC_RECORD_COUNT0_OFFSET, 8), 4);
}

#[test]
fn an_armed_retirement_watch_leaves_untracking_to_the_runtime() {
    let mut m = Machine::default();
    m.gc(2, 8, 10, 700);
    m.write_word(GC, GC_RECORD_WATCH_ARMED_OFFSET, 1);
    m.class(point_plan());
    m.dying_member(TRACKED, 0, 1);
    m.release_at(TRACKED, 0, 0);
    assert_eq!(m.events, std::vec![Event::Cold]);
    assert_eq!(m.list_len(0), 2);
    assert_eq!(m.read_bytes(GC, GC_RECORD_COUNT0_OFFSET, 8), 10);
}
