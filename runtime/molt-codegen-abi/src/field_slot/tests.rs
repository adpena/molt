//! The generators run on an interpreting builder over byte-addressed memory
//! holding real object headers. The oracle is the runtime's field rule
//! written directly (molt-runtime `object_field_load_ptr_raw` and
//! `object_field_set_ptr_raw`): a dictionary-backed, empty or non-object
//! receiver is the runtime's; otherwise the inline word is the field.

use super::*;
use crate::test_machine::{Event, Machine};
use crate::{HEADER_AUX_KIND_SIDECAR, TAG_INT, TAG_PTR, TYPE_ID_LIST, TYPE_ID_STRING};

const OBJECT: u64 = 0x1_0000;
const VALUE: u64 = 0x2_0000;
const CLASS: u64 = 0x3_0000;
const OFFSET: i32 = 16;
const COLD: u64 = 0xC01D;

fn boxed(address: u64) -> u64 {
    QNAN | TAG_PTR | address
}

fn int(value: u64) -> u64 {
    QNAN | TAG_INT | value
}

impl Machine {
    /// An object of `type_id` at `address` with `flags`, an inline class word
    /// `class` and `field` at `OFFSET`.
    fn object(&mut self, address: u64, type_id: u32, flags: u32, class: u64, field: u64) {
        self.write_bytes(address, HEADER_TYPE_ID_OFFSET, type_id.into(), 4);
        self.write_bytes(address, HEADER_FLAGS_OFFSET, flags.into(), 4);
        self.write_bytes(
            address,
            HEADER_AUX_KIND_OFFSET - 2,
            u64::from(HEADER_AUX_KIND_CLASS_INLINE) << 16,
            4,
        );
        self.write_bytes(address, HEADER_AUX_OFFSET, class, 8);
        self.write_bytes(address, OFFSET, field, 8);
    }

    /// A receiver of the proven class holding `field`.
    fn receiver(flags: u32, field: u64) -> Self {
        let mut m = Self::default();
        m.object(OBJECT, TYPE_ID_OBJECT, flags, boxed(CLASS), field);
        m
    }

    fn field(&self) -> u64 {
        self.read_bytes(OBJECT, OFFSET, 8)
    }

    fn flags(&self) -> u32 {
        self.read_bytes(OBJECT, HEADER_FLAGS_OFFSET, 4) as u32
    }

    fn get_on(&mut self, object: u64, locking: ListLocking) -> u64 {
        generate_field_slot_get(self, object, OFFSET, locking, |m, pointer| {
            assert_eq!(pointer, OBJECT, "the miss receives the unboxed receiver");
            m.events.push(Event::Cold);
            COLD
        })
    }

    fn read_field(&mut self) -> u64 {
        self.get_on(boxed(OBJECT), ListLocking::Gil)
    }

    fn set_on(&mut self, object: u64, value: u64, locking: ListLocking) {
        generate_field_slot_set(
            self,
            object,
            OFFSET,
            value,
            FieldStoreSuffix::Unknown,
            locking,
            |m, pointer| {
                assert_eq!(pointer, OBJECT, "the miss receives the unboxed receiver");
                m.events.push(Event::Cold);
            },
        );
    }

    fn write_field(&mut self, value: u64) {
        self.set_on(boxed(OBJECT), value, ListLocking::Gil);
    }
}

impl FieldSlotBuilder for Machine {
    fn or_header_flag(&mut self, pointer: u64, flag: u32) {
        if self.bailed {
            return;
        }
        let flags = self.read_bytes(pointer, HEADER_FLAGS_OFFSET, 4) as u32;
        self.write_bytes(pointer, HEADER_FLAGS_OFFSET, u64::from(flags | flag), 4);
        self.events.push(Event::Flag(flag));
    }
}

#[test]
fn an_inline_field_reads_as_an_owned_word() {
    let mut m = Machine::receiver(0, int(7));
    assert_eq!(m.read_field(), int(7));
    assert_eq!(m.events, [Event::Retain(int(7))]);

    // A string, and an instance of a class, are field values.
    for (type_id, class) in [(TYPE_ID_STRING, 0), (TYPE_ID_OBJECT, boxed(CLASS))] {
        let mut m = Machine::receiver(HEADER_FLAG_HAS_PTRS, boxed(VALUE));
        m.object(VALUE, type_id, 0, class, 0);
        assert_eq!(m.read_field(), boxed(VALUE));
        assert_eq!(m.events, [Event::Retain(boxed(VALUE))]);
    }
}

#[test]
fn an_inline_field_probe_borrows_an_initialized_value_without_dispatch() {
    let field = boxed(VALUE);
    let mut m = Machine::receiver(HEADER_FLAG_HAS_PTRS, field);
    m.object(VALUE, TYPE_ID_STRING, 0, 0, 0);
    assert_eq!(
        generate_field_slot_probe(&mut m, boxed(OBJECT), OFFSET, ListLocking::Gil),
        field
    );
    assert!(m.events.is_empty(), "probing does not retain the field");

    for (receiver, locking) in [
        (int(4), ListLocking::Gil),
        (boxed(OBJECT), ListLocking::FreeThreaded),
    ] {
        assert_eq!(
            generate_field_slot_probe(&mut m, receiver, OFFSET, locking),
            QNAN | TAG_NONE
        );
    }
    m.write_bytes(
        OBJECT,
        HEADER_FLAGS_OFFSET,
        HEADER_FLAG_HAS_INSTANCE_DICT.into(),
        4,
    );
    assert_eq!(
        generate_field_slot_probe(&mut m, boxed(OBJECT), OFFSET, ListLocking::Gil),
        QNAN | TAG_NONE
    );
    assert!(
        m.events.is_empty(),
        "misses retain no word and make no runtime call"
    );
}

/// The runtime owns an empty field (class fallback or AttributeError), a
/// dictionary-backed one, and a receiver whose field base may move.
#[test]
fn every_word_the_inline_slot_does_not_own_takes_the_runtime() {
    type Refusal = (&'static str, fn(&mut Machine));
    let refusals: [Refusal; 4] = [
        ("instance dictionary", |m| {
            m.write_bytes(
                OBJECT,
                HEADER_FLAGS_OFFSET,
                HEADER_FLAG_HAS_INSTANCE_DICT.into(),
                4,
            )
        }),
        ("builtin-payload subclass", |m| {
            m.write_bytes(OBJECT, HEADER_TYPE_ID_OFFSET, TYPE_ID_LIST.into(), 4)
        }),
        ("missing singleton", |m| {
            m.write_bytes(OBJECT, OFFSET, boxed(VALUE), 8);
            m.object(VALUE, TYPE_ID_OBJECT, crate::HEADER_FLAG_IMMORTAL, 0, 0);
        }),
        ("classless object behind a sidecar", |m| {
            m.write_bytes(OBJECT, OFFSET, boxed(VALUE), 8);
            m.object(VALUE, TYPE_ID_OBJECT, 0, 0x4_0000, 0);
            m.write_bytes(
                VALUE,
                HEADER_AUX_KIND_OFFSET,
                HEADER_AUX_KIND_SIDECAR.into(),
                2,
            );
        }),
    ];
    for (name, refuse) in refusals {
        let mut m = Machine::receiver(0, int(3));
        refuse(&mut m);
        assert_eq!(m.read_field(), COLD, "{name}");
        assert_eq!(m.events, [Event::Cold], "{name}");
    }
    for (name, refuse) in &refusals {
        let mut m = Machine::receiver(0, int(3));
        refuse(&mut m);
        let old = m.field();
        m.write_field(int(4));
        assert_eq!(m.field(), old, "{name}");
        assert_eq!(m.events, [Event::Cold], "{name}");
    }
}

/// A failed allocation leaves a non-pointer sentinel with its exception
/// pending: the access must neither touch memory nor raise again.
#[test]
fn a_non_pointer_receiver_reads_none_and_stores_nothing() {
    let mut m = Machine::default();
    assert_eq!(m.get_on(int(0), ListLocking::Gil), QNAN | TAG_NONE);
    m.set_on(int(0), int(1), ListLocking::Gil);
    assert!(m.events.is_empty());
}

#[test]
fn a_store_retains_the_new_word_before_it_releases_the_old_one() {
    let mut m = Machine::receiver(0, int(1));
    m.write_field(boxed(VALUE));
    assert_eq!(m.field(), boxed(VALUE));
    assert_eq!(m.flags(), HEADER_FLAG_HAS_PTRS);
    // Before the first pointer the old word owns nothing: no release.
    assert_eq!(
        m.events,
        [
            Event::Flag(HEADER_FLAG_HAS_PTRS),
            Event::Retain(boxed(VALUE))
        ]
    );
    // HAS_PTRS is already set: no second write of the header.
    m.object(VALUE, TYPE_ID_STRING, 0, 0, 0);
    m.events.clear();
    m.write_field(int(2));
    m.write_field(boxed(VALUE));
    assert_eq!(
        m.events,
        [
            Event::Retain(int(2)),
            Event::Release(boxed(VALUE)),
            Event::Retain(boxed(VALUE)),
            Event::Release(int(2)),
        ]
    );
}

#[test]
fn an_immediate_store_leaves_the_header_alone() {
    let mut m = Machine::receiver(0, int(1));
    m.write_field(int(9));
    assert_eq!((m.field(), m.flags()), (int(9), 0));
    assert_eq!(m.events, [Event::Retain(int(9))]);
    // Once the object holds pointers, the old word is released.
    let mut m = Machine::receiver(HEADER_FLAG_HAS_PTRS, int(1));
    m.write_field(int(9));
    assert_eq!(m.events, [Event::Retain(int(9)), Event::Release(int(1))]);
}

#[test]
fn a_known_allocation_suffix_admits_only_order_preserving_first_writes() {
    let missing = boxed(VALUE);
    for (words, later, cold) in [
        (3, int(7), false),
        (4, missing, false),
        (4, int(7), true),
        (5, missing, true),
    ] {
        let mut m = Machine::receiver(0, missing);
        m.object(VALUE, TYPE_ID_OBJECT, crate::HEADER_FLAG_IMMORTAL, 0, 0);
        m.write_bytes(OBJECT, OFFSET + 8, later, 8);
        let suffix = FieldStoreSuffix::from_allocation(Some(words), OFFSET);
        generate_field_slot_set(
            &mut m,
            boxed(OBJECT),
            OFFSET,
            int(5),
            suffix,
            ListLocking::Gil,
            |m, _| m.events.push(Event::Cold),
        );
        assert_eq!(m.events.contains(&Event::Cold), cold);
        assert_eq!(m.field(), if cold { missing } else { int(5) });
    }
}

#[test]
fn a_free_threaded_build_takes_the_runtime() {
    let mut m = Machine::receiver(0, int(1));
    assert_eq!(m.get_on(boxed(OBJECT), ListLocking::FreeThreaded), COLD);
    m.set_on(boxed(OBJECT), int(2), ListLocking::FreeThreaded);
    assert_eq!(m.field(), int(1));
    assert_eq!(m.events, [Event::Cold, Event::Cold]);
}
