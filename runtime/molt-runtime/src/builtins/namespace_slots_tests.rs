use super::*;
use crate::object::ops::{
    dict_clear_in_place, dict_del_in_place, dict_get_in_place, dict_publish_staged,
    dict_remove_last, dict_set_in_place,
};

const LAYOUT: [&[u8]; 4] = [b"count", b"math", b"total", b"x-y"];

fn name(py: &PyToken<'_>, bytes: &[u8]) -> u64 {
    // A fresh, non-interned object for the one non-identifier name, so the
    // probe's byte comparison is exercised as well as its pointer match.
    MoltObject::from_ptr(crate::alloc_string(py, bytes)).bits()
}

fn slotted(py: &PyToken<'_>) -> *mut u8 {
    let dict = crate::builtins::binding_guards::alloc_namespace_dict(py);
    assert!(!dict.is_null());
    unsafe { bind(py, dict, 7, LAYOUT.iter().copied()) };
    assert!(unsafe { namespace_slots(dict) }.is_some());
    dict
}

/// Independent oracle: the dictionary's own lookup decides every slot.
fn assert_mirror(py: &PyToken<'_>, dict: *mut u8) {
    let table = unsafe { namespace_slots(dict) }.expect("live table");
    for (k, bytes) in LAYOUT.iter().enumerate() {
        let key = name(py, bytes);
        let expected = unsafe { dict_get_in_place(py, dict, key) }.unwrap();
        dec_ref_bits(py, key);
        match expected {
            Some(value) => {
                assert_eq!(table.value(k), value, "slot {k} value");
                let row = table.row(k).expect("bound slot has a row");
                assert_eq!(
                    unsafe { dict_entries(dict)[row].value },
                    value,
                    "slot {k} row"
                );
            }
            None => {
                assert_eq!(table.value(k), UNBOUND, "slot {k} unbound");
                assert_eq!(table.row(k), None);
            }
        }
    }
}

fn set(py: &PyToken<'_>, dict: *mut u8, bytes: &[u8], value: u64) {
    let key = name(py, bytes);
    assert!(unsafe { dict_set_in_place(py, dict, key, value) });
    dec_ref_bits(py, key);
}

fn del(py: &PyToken<'_>, dict: *mut u8, bytes: &[u8]) {
    let key = name(py, bytes);
    assert_eq!(unsafe { dict_del_in_place(py, dict, key) }, Ok(true));
    dec_ref_bits(py, key);
}

fn int(value: i64) -> u64 {
    MoltObject::from_int(value).bits()
}

#[test]
fn every_commit_primitive_keeps_the_slots_equal_to_the_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dict = slotted(py);
        assert_mirror(py, dict);
        // Insert, replace, and a name outside the layout.
        set(py, dict, b"total", int(1));
        set(py, dict, b"x-y", int(2));
        set(py, dict, b"other", int(3));
        assert_mirror(py, dict);
        set(py, dict, b"total", int(4));
        assert_mirror(py, dict);
        // A heap value, replaced by another heap value.
        let list = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        set(py, dict, b"math", list);
        dec_ref_bits(py, list);
        assert_mirror(py, dict);
        set(py, dict, b"math", int(5));
        assert_mirror(py, dict);
        // Delete, then rebind: the new row is the slot's row.
        del(py, dict, b"total");
        assert_mirror(py, dict);
        set(py, dict, b"total", int(6));
        assert_mirror(py, dict);
        // Growth and compaction move rows.
        for index in 0..200 {
            let filler = format!("filler{index}");
            set(py, dict, filler.as_bytes(), int(index));
        }
        for index in 0..200 {
            let filler = format!("filler{index}");
            del(py, dict, filler.as_bytes());
        }
        set(py, dict, b"count", int(7));
        assert_mirror(py, dict);
        // popitem removes the last row.
        drop(unsafe { dict_remove_last(py, dict) });
        assert_mirror(py, dict);
        // Staged publication swaps the whole contents.
        let staged = crate::alloc_dict_with_pairs(py, &[]);
        set(py, staged, b"math", int(8));
        unsafe { dict_publish_staged(py, dict, staged) };
        assert_mirror(py, dict);
        dec_ref_bits(py, MoltObject::from_ptr(staged).bits());
        // clear() empties every slot.
        unsafe { dict_clear_in_place(py, dict) };
        assert_mirror(py, dict);
        set(py, dict, b"total", int(9));
        assert_mirror(py, dict);
        dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
    })
}

#[test]
fn a_key_that_is_not_an_exact_str_degrades_the_table_for_good() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dict = slotted(py);
        set(py, dict, b"total", int(1));
        assert!(unsafe { namespace_slots(dict) }.is_some());
        assert!(unsafe { dict_set_in_place(py, dict, int(3), int(1)) });
        assert!(unsafe { namespace_slots(dict) }.is_none());
        let table = unsafe { &*dict_storage(dict).slots.slots().unwrap() };
        assert!(table.degraded());
        assert!((0..table.len()).all(|k| table.value(k) == UNBOUND));
        unsafe { dict_clear_in_place(py, dict) };
        assert!(unsafe { namespace_slots(dict) }.is_none(), "degraded stays");
        dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
    })
}

#[test]
fn a_layout_name_matches_by_identity_or_bytes_and_other_names_miss() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dict = slotted(py);
        let table = unsafe { namespace_slots(dict) }.unwrap();
        for (expected, bytes) in [
            (Some(2), &b"total"[..]),
            (Some(3), b"x-y"),
            (None, b"other"),
        ] {
            let key = name(py, bytes);
            let hash = unsafe { crate::object::ops::dict_key_hash(py, key) }.unwrap();
            assert_eq!(unsafe { table.slot_of(key, hash) }, expected, "{bytes:?}");
            dec_ref_bits(py, key);
        }
        dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
    })
}

/// Shutdown retirement sets each bound cell to `None`, as CPython clears a
/// module's dictionary, and leaves an unbound cell unbound: compiled code
/// reads an init-dominated cell with no unbound test (HF-846), so a finalizer
/// that runs module code during retirement must find an object there.
#[test]
fn retirement_stores_none_into_bound_cells_and_keeps_unbound_ones() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dict = crate::builtins::binding_guards::alloc_namespace_dict(py);
        assert!(!dict.is_null());
        let words: &'static mut [u64] = Box::leak(vec![UNBOUND; 3].into_boxed_slice());
        let block = words.as_mut_ptr();
        unsafe {
            release(dict);
            let cells = Box::new(StaticCells {
                names: Box::new([&b"a"[..], &b"b"[..], &b"c"[..]]),
                words: block,
            });
            dict_storage(dict).slots = NamespaceBinding(Box::into_raw(cells) as usize | CELLS_TAG);
        }
        let list = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        for bytes in [&b"a"[..], b"c"] {
            let key = name(py, bytes);
            assert!(static_cell_set(py, dict, key, list));
            dec_ref_bits(py, key);
        }
        dec_ref_bits(py, list);
        let none = MoltObject::none().bits();
        let word = |k: usize| unsafe { *block.add(k) };
        assert!(retire_static_cells(py, dict), "two bound cells released");
        assert_eq!([word(0), word(1), word(2)], [none, UNBOUND, none]);
        assert!(
            !retire_static_cells(py, dict),
            "a second pass finds nothing to release"
        );
        // A finalizer's store after retirement is released by the next pass.
        let key = name(py, b"b");
        assert!(static_cell_set(py, dict, key, int(5)));
        dec_ref_bits(py, key);
        assert!(retire_static_cells(py, dict));
        assert_eq!(word(1), none);
    })
}
