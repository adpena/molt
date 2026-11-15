//! The generators run on an interpreting builder over byte-addressed memory
//! holding real object headers and `MoltVec` storage. The oracle is Python's
//! own rule written directly: `xs[i]` after the negative-index adjustment, in
//! range, on an exact list; everything else is the runtime's.

use super::*;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_FLAG_HAS_PTRS, QNAN, TAG_BOOL, TAG_INT, TAG_NONE, TAG_PTR,
    TYPE_ID_DICT, TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, TYPE_ID_TUPLE, box_int_bits,
};

const LIST: u64 = 0x1_0000;
const STORAGE: u64 = 0x2_0000;
const DATA: u64 = 0x3_0000;
const COLD: u64 = 0xC01D;

fn boxed(address: u64) -> u64 {
    QNAN | TAG_PTR | address
}

fn int(value: i64) -> u64 {
    box_int_bits(value) as u64
}

fn boolean(value: bool) -> u64 {
    QNAN | TAG_BOOL | u64::from(value)
}

use crate::test_machine::{Event, Machine};

impl Machine {
    /// A list object of `type_id` whose storage holds `elements`: 8-byte
    /// words, or bytes for a bool list.
    fn list(type_id: u32, elements: &[u64]) -> Self {
        let mut m = Self::default();
        m.write_u32(LIST, HEADER_TYPE_ID_OFFSET, type_id);
        m.write_u32(LIST, HEADER_FLAGS_OFFSET, HEADER_FLAG_HAS_PTRS);
        // As the runtime allocates a list: an inline class word of zero.
        m.write_u32(
            LIST,
            HEADER_AUX_KIND_OFFSET - 2,
            u32::from(HEADER_AUX_KIND_CLASS_INLINE) << 16,
        );
        m.write_word(LIST, HEADER_AUX_OFFSET, 0);
        m.write_word(LIST, 0, STORAGE);
        m.write_word(STORAGE, LIST_STORAGE_DATA_OFFSET, DATA);
        m.write_word(STORAGE, LIST_STORAGE_LEN_OFFSET, elements.len() as u64);
        for (index, &element) in elements.iter().enumerate() {
            if type_id == TYPE_ID_LIST_BOOL {
                m.bytes.insert(DATA + index as u64, element as u8);
            } else {
                m.write_word(DATA, 8 * index as i32, element);
            }
        }
        m
    }

    /// An exact tuple at `LIST` holding `items` inline after its length word.
    fn tuple(items: &[u64]) -> Self {
        let mut m = Self::list(TYPE_ID_TUPLE, &[]);
        m.write_word(LIST, crate::TUPLE_STORAGE_LEN_OFFSET, items.len() as u64);
        for (index, &item) in items.iter().enumerate() {
            m.write_word(
                LIST,
                crate::TUPLE_STORAGE_ITEMS_OFFSET + 8 * index as i32,
                item,
            );
        }
        m
    }

    fn element(&self, index: u64) -> u64 {
        self.read_bytes(DATA, 8 * index as i32, 8)
    }

    fn get(&mut self, index: u64) -> u64 {
        self.get_as(ItemContainers::ListsOrTuple, index)
    }

    fn get_as(&mut self, containers: ItemContainers, index: u64) -> u64 {
        generate_list_item_get(
            self,
            boxed(LIST),
            index,
            containers,
            ListLocking::Gil,
            |m| {
                m.events.push(Event::Cold);
                COLD
            },
        )
    }

    fn set(&mut self, index: u64, value: u64) -> u64 {
        generate_list_item_set(
            self,
            boxed(LIST),
            index,
            value,
            ListLocking::Gil,
            QNAN | TAG_NONE,
            |m| {
                m.events.push(Event::Cold);
                COLD
            },
        )
    }
}

/// Python's index rule, the oracle: the element position, or None when the
/// runtime raises IndexError.
fn python_position(index: i64, len: usize) -> Option<u64> {
    let len = len as i64;
    let adjusted = if index < 0 { index + len } else { index };
    (0..len).contains(&adjusted).then_some(adjusted as u64)
}

const OBJECTS: [u64; 3] = [
    0x7ffc_0000_00a0_0000,
    0x7ffc_0000_00b0_0000,
    0x7ffc_0000_00c0_0000,
];

#[test]
fn a_word_list_read_matches_pythons_index_rule_on_every_edge() {
    for index in [-5i64, -4, -3, -1, 0, 1, 2, 3, 4, 1 << 45, -(1 << 45)] {
        let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
        let got = m.get(int(index));
        match python_position(index, OBJECTS.len()) {
            Some(position) => {
                let item = OBJECTS[position as usize];
                assert_eq!((got, &m.events[..]), (item, &[Event::Retain(item)][..]));
            }
            None => assert_eq!((got, &m.events[..]), (COLD, &[Event::Cold][..])),
        }
    }
}

#[test]
fn a_compact_int_list_boxes_its_inline_ints_without_a_retain() {
    let mut m = Machine::list(TYPE_ID_LIST_INT, &[7, (-3i64) as u64]);
    assert_eq!(m.get(int(-1)), int(-3));
    assert_eq!(m.get(int(0)), int(7));
    assert!(m.events.is_empty());
    assert_eq!(m.set(int(0), int(-9)), QNAN | TAG_NONE);
    assert_eq!(m.element(0) as i64, -9);
    // A value the compact kind cannot hold converts the storage: runtime.
    assert_eq!(m.set(int(1), OBJECTS[0]), COLD);
    assert_eq!(m.element(1) as i64, -3);
    assert_eq!(m.events, [Event::Cold]);
    // A compact bool list is the runtime's.
    let mut m = Machine::list(TYPE_ID_LIST_BOOL, &[1, 0]);
    assert_eq!(m.get(int(0)), COLD);
    assert_eq!(m.set(int(0), boolean(false)), COLD);
}

/// Each named change sends the read to the runtime, which owns subclass
/// protocols, C views, other containers and every non-int index.
#[test]
fn everything_but_an_exact_list_at_an_inline_int_takes_the_runtime() {
    type Refusal = (&'static str, fn(&mut Machine) -> u64);
    let refusals: [Refusal; 7] = [
        ("subclass class edge", |m| {
            m.write_word(LIST, HEADER_AUX_OFFSET, 0x7ffc_0000_00e0_0000 & !7);
            int(0)
        }),
        ("sidecar", |m| {
            m.write_bytes(
                LIST,
                HEADER_AUX_KIND_OFFSET,
                crate::HEADER_AUX_KIND_SIDECAR.into(),
                2,
            );
            int(0)
        }),
        ("C view", |m| {
            m.write_u32(LIST, HEADER_FLAGS_OFFSET, HEADER_FLAG_HAS_ABI_VIEW);
            int(0)
        }),
        ("dict", |m| {
            m.write_u32(LIST, HEADER_TYPE_ID_OFFSET, TYPE_ID_DICT);
            int(0)
        }),
        ("bool index", |_| boolean(false)),
        ("big int index", |_| boxed(0x9_0000)),
        ("float index", |_| 0.0f64.to_bits()),
    ];
    for (name, refuse) in refusals {
        let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
        let index = refuse(&mut m);
        assert_eq!(m.get(index), COLD, "{name}");
        assert_eq!(m.events, [Event::Cold], "{name}");
    }
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    assert_eq!(
        generate_list_item_get(
            &mut m,
            int(3),
            int(0),
            ItemContainers::ListsOrTuple,
            ListLocking::Gil,
            |_| COLD
        ),
        COLD,
        "a non-pointer container"
    );
}

#[test]
fn a_word_store_retains_the_new_value_before_releasing_the_old() {
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    let value = 0x7ffc_0000_00d0_0000;
    assert_eq!(m.set(int(-1), value), QNAN | TAG_NONE);
    assert_eq!(m.element(2), value);
    assert_eq!(m.events, [Event::Retain(value), Event::Release(OBJECTS[2])]);
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    assert_eq!(m.set(int(3), value), COLD);
    assert_eq!(m.events, [Event::Cold]);
    assert_eq!(m.element(2), OBJECTS[2], "a refused store changes nothing");
}

#[test]
fn a_store_that_changes_the_heap_edge_count_takes_the_runtime() {
    // The runtime counts a generic list's pointer items exactly; only a
    // store that keeps the count may skip it. `xs[i] = obj` over an int and
    // `xs[i] = 0` over an object both go to `cold`, untouched.
    let pointer = 0x7ffc_0000_00d0_0000;
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    assert_eq!(m.set(int(0), int(5)), COLD, "int over an object");
    assert_eq!(m.events, [Event::Cold]);
    assert_eq!(m.element(0), OBJECTS[0]);
    let ints = [int(1), int(2), int(3)];
    let mut m = Machine::list(TYPE_ID_LIST, &ints);
    assert_eq!(m.set(int(0), pointer), COLD, "an object over an int");
    assert_eq!(m.events, [Event::Cold]);
    assert_eq!(m.element(0), ints[0]);
    let mut m = Machine::list(TYPE_ID_LIST, &ints);
    assert_eq!(
        m.set(int(0), int(9)),
        QNAN | TAG_NONE,
        "int over int keeps it"
    );
    assert_eq!(m.element(0), int(9));
}

#[test]
fn a_free_threaded_build_always_takes_the_runtime() {
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    let read = generate_list_item_get(
        &mut m,
        boxed(LIST),
        int(0),
        ItemContainers::ListsOrTuple,
        ListLocking::FreeThreaded,
        |_| COLD,
    );
    assert_eq!((read, m.events.len()), (COLD, 0));
}

#[test]
fn the_inline_int_tag_is_the_runtime_one() {
    assert_eq!(int(5) & (QNAN | crate::TAG_MASK), QNAN | TAG_INT);
}

#[test]
fn a_list_without_an_aux_word_is_exact_too() {
    let mut m = Machine::list(TYPE_ID_LIST, &OBJECTS);
    m.write_bytes(
        LIST,
        HEADER_AUX_KIND_OFFSET,
        crate::HEADER_AUX_KIND_NONE.into(),
        2,
    );
    assert_eq!(m.get(int(1)), OBJECTS[1]);
    assert_eq!(m.events, [Event::Retain(OBJECTS[1])]);
}

#[test]
fn an_exact_tuple_read_matches_pythons_index_rule_and_a_store_is_the_runtimes() {
    for index in [-4i64, -3, -1, 0, 2, 3] {
        let mut m = Machine::tuple(&OBJECTS);
        let got = m.get(int(index));
        match python_position(index, OBJECTS.len()) {
            Some(position) => {
                let item = OBJECTS[position as usize];
                assert_eq!((got, &m.events[..]), (item, &[Event::Retain(item)][..]));
            }
            None => assert_eq!((got, &m.events[..]), (COLD, &[Event::Cold][..])),
        }
    }
    // Tuples are immutable: the runtime raises the store's TypeError.
    let mut m = Machine::tuple(&OBJECTS);
    assert_eq!(m.set(int(0), OBJECTS[1]), COLD);
    assert_eq!(m.events, [Event::Cold]);
    // A tuple subclass carries its class word.
    let mut m = Machine::tuple(&OBJECTS);
    m.write_word(LIST, HEADER_AUX_OFFSET, 0x7ffc_0000_00e0_0000 & !7);
    assert_eq!(m.get(int(0)), COLD);
    // The empty tuple has no element to read.
    let mut m = Machine::tuple(&[]);
    assert_eq!(m.get(int(0)), COLD);
}

#[test]
fn a_proven_container_kind_drops_tests_but_never_admits_another_kind() {
    let containers = [
        ItemContainers::Lists,
        ItemContainers::Tuple,
        ItemContainers::ListsOrTuple,
    ];
    // (name, the object, whether it is a tuple)
    type Case = (&'static str, fn() -> Machine, bool);
    let objects: [Case; 3] = [
        ("list", || Machine::list(TYPE_ID_LIST, &OBJECTS), false),
        (
            "int list",
            || Machine::list(TYPE_ID_LIST_INT, &[7, 8, 9]),
            false,
        ),
        ("tuple", || Machine::tuple(&OBJECTS), true),
    ];
    for (name, object, is_tuple) in objects {
        let expected = object().get(int(-2));
        assert_ne!(expected, COLD, "{name}");
        for choice in containers {
            let admits = match choice {
                ItemContainers::Lists => !is_tuple,
                ItemContainers::Tuple => is_tuple,
                ItemContainers::ListsOrTuple => true,
            };
            let mut m = object();
            let got = m.get_as(choice, int(-2));
            let want = if admits { expected } else { COLD };
            assert_eq!(got, want, "{name} read as {choice:?}");
        }
    }
}
