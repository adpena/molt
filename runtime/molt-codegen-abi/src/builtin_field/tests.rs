//! The generator runs on an interpreting builder over byte-addressed memory.
//! The oracle is the runtime getter's rule written directly (molt-runtime
//! `exception_typed_field_get`): the slot word, None or AttributeError for
//! the missing marker per member, a boxed `Py_ssize_t`, -1 raising where the
//! member says so.

use super::*;
use crate::exception_layout::{EXCEPTION_MSG_SLOT, EXCEPTION_TYPED_TAIL_SLOT};
use crate::test_machine::{Event, Machine};
use crate::{TAG_INT, TAG_PTR};

const OBJECT: u64 = 0x1_0000;
const VALUE: u64 = 0x2_0000;
const COLD: u64 = 0xC01D;

fn boxed(address: u64) -> u64 {
    QNAN | TAG_PTR | address
}

fn int(value: u64) -> u64 {
    QNAN | TAG_INT | value
}

impl Machine {
    fn read_member(&mut self, value: u64, read: BuiltinFieldRead, locking: ListLocking) -> u64 {
        generate_builtin_field_get(self, value, read, locking, |m, cold_value| {
            assert_eq!(cold_value, value, "the miss receives the boxed receiver");
            m.events.push(Event::Cold);
            COLD
        })
    }
}

fn holding(slot: usize, word: u64) -> Machine {
    let mut m = Machine::default();
    m.write_word(OBJECT, slot_offset(slot), word);
    m
}

/// The slot table, derived independently: a message alias is the message
/// slot; every other member follows the prefix in its layout's tail order.
#[test]
fn every_member_slot_matches_the_runtime_layout_rule() {
    for layout in ExceptionLayoutKind::ALL {
        let mut tail = 0;
        for policy in layout.field_policies() {
            let expected = if policy.storage == ExceptionFieldStorage::RuntimeMessage {
                EXCEPTION_MSG_SLOT
            } else {
                tail += 1;
                EXCEPTION_TYPED_TAIL_SLOT + tail - 1
            };
            assert_eq!(exception_field_slot(layout, policy.field), Some(expected));
            assert_eq!(exception_field_layout(policy.field), layout);
            let slot = match exception_field_read(policy.field) {
                BuiltinFieldRead::Object { slot, .. } | BuiltinFieldRead::PySsize { slot, .. } => {
                    slot
                }
            };
            assert_eq!(slot, expected, "{:?}", policy.field);
        }
        assert_eq!(tail, layout.tail_word_count());
    }
}

#[test]
fn a_present_object_member_reads_as_an_owned_word() {
    for missing_raises in [false, true] {
        let read = BuiltinFieldRead::Object {
            slot: 11,
            missing_raises,
        };
        let mut m = holding(11, boxed(VALUE));
        assert_eq!(
            m.read_member(boxed(OBJECT), read, ListLocking::Gil),
            boxed(VALUE)
        );
        assert_eq!(m.events, [Event::Retain(boxed(VALUE))]);
    }
}

#[test]
fn an_absent_object_member_reads_none_or_takes_the_runtime() {
    let mut m = holding(12, EXCEPTION_FIELD_MISSING_BITS);
    let none_rule = BuiltinFieldRead::Object {
        slot: 12,
        missing_raises: false,
    };
    assert_eq!(
        m.read_member(boxed(OBJECT), none_rule, ListLocking::Gil),
        QNAN | TAG_NONE
    );
    assert!(m.events.is_empty());
    let raising = BuiltinFieldRead::Object {
        slot: 12,
        missing_raises: true,
    };
    assert_eq!(
        m.read_member(boxed(OBJECT), raising, ListLocking::Gil),
        COLD
    );
    assert_eq!(m.events, [Event::Cold]);
}

#[test]
fn a_py_ssize_member_boxes_inline_and_leaves_the_rest_to_the_runtime() {
    let raising = BuiltinFieldRead::PySsize {
        slot: 13,
        missing_raises: true,
    };
    let plain = BuiltinFieldRead::PySsize {
        slot: 13,
        missing_raises: false,
    };
    let mut m = holding(13, 5);
    assert_eq!(
        m.read_member(boxed(OBJECT), plain, ListLocking::Gil),
        int(5)
    );
    assert!(m.events.is_empty());
    // -1: unset where the member raises (characters_written), a value elsewhere.
    let mut m = holding(13, EXCEPTION_PY_SSIZE_MISSING);
    assert_eq!(
        m.read_member(boxed(OBJECT), raising, ListLocking::Gil),
        COLD
    );
    let mut m = holding(13, EXCEPTION_PY_SSIZE_MISSING);
    let minus_one = m.read_member(boxed(OBJECT), plain, ListLocking::Gil);
    assert_eq!(minus_one, crate::box_int_bits(-1) as u64);
    // Past the inline range the runtime boxes.
    let mut m = holding(13, 1 << 60);
    assert_eq!(m.read_member(boxed(OBJECT), plain, ListLocking::Gil), COLD);
}

#[test]
fn a_non_pointer_or_free_threaded_read_takes_the_runtime() {
    let read = BuiltinFieldRead::Object {
        slot: 11,
        missing_raises: false,
    };
    let mut m = holding(11, int(1));
    assert_eq!(m.read_member(int(0), read, ListLocking::Gil), COLD);
    assert_eq!(
        m.read_member(boxed(OBJECT), read, ListLocking::FreeThreaded),
        COLD
    );
    assert_eq!(m.events, [Event::Cold, Event::Cold]);
}
