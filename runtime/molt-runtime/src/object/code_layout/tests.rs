use super::code_slots;
use crate::object::layout::code_publish_lexical_metadata;
use crate::{MoltObject, PyToken, alloc_code_obj, alloc_string, alloc_tuple, dec_ref_bits};

fn names(py: &PyToken<'_>, items: &[&str]) -> u64 {
    let values: Vec<_> = items
        .iter()
        .map(|text| MoltObject::from_ptr(alloc_string(py, text.as_bytes())).bits())
        .collect();
    let tuple = MoltObject::from_ptr(alloc_tuple(py, &values)).bits();
    for bits in values {
        dec_ref_bits(py, bits);
    }
    tuple
}

fn code(py: &PyToken<'_>, locals: &[&str], cells: &[&str], free: &[&str], argc: u64) -> u64 {
    let text = MoltObject::from_ptr(alloc_string(py, b"<code-layout>")).bits();
    let locals = names(py, locals);
    let cells = names(py, cells);
    let free = names(py, free);
    let empty = names(py, &[]);
    let code = alloc_code_obj(
        py,
        text,
        text,
        1,
        MoltObject::none().bits(),
        locals,
        empty,
        argc,
        0,
        0,
    );
    assert!(!code.is_null());
    assert!(unsafe { code_publish_lexical_metadata(py, code, free, cells) });
    for bits in [text, locals, cells, free, empty] {
        dec_ref_bits(py, bits);
    }
    MoltObject::from_ptr(code).bits()
}

#[test]
fn code_slots_merge_nonparameter_cells_without_reordering_locals() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        // CPython 3.12: item=7; readers=[lambda:item for item in (1,2)].
        // Distinct string allocations across tables must match by content.
        let bits = code(py, &["arg", "item", "readers"], &["item"], &["outer"], 1);
        let layout = unsafe { code_slots(crate::obj_from_bits(bits).as_ptr().unwrap()) }
            .expect("valid code layout");
        let actual: Vec<_> = layout
            .names
            .iter()
            .map(|&name| unsafe {
                let ptr = crate::obj_from_bits(name).as_ptr().unwrap();
                std::slice::from_raw_parts(crate::string_bytes(ptr), crate::string_len(ptr))
            })
            .collect();
        assert_eq!(actual, [b"arg".as_slice(), b"item", b"readers", b"outer"]);
        dec_ref_bits(py, bits);
    });
}

#[test]
fn code_slots_append_cell_only_bindings_after_all_locals() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let bits = code(py, &["arg", "reader"], &["arg", "captured"], &[], 1);
        let layout = unsafe { code_slots(crate::obj_from_bits(bits).as_ptr().unwrap()) }
            .expect("valid code layout");
        let actual: Vec<_> = layout
            .names
            .iter()
            .map(|&name| unsafe {
                let ptr = crate::obj_from_bits(name).as_ptr().unwrap();
                std::slice::from_raw_parts(crate::string_bytes(ptr), crate::string_len(ptr))
            })
            .collect();
        assert_eq!(actual, [b"arg".as_slice(), b"reader", b"captured"]);
        dec_ref_bits(py, bits);
    });
}

#[test]
fn code_slots_reject_parameter_range_and_noncode_objects() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let bits = code(py, &["arg"], &[], &[], u64::MAX);
        assert_eq!(
            unsafe { code_slots(crate::obj_from_bits(bits).as_ptr().unwrap()) }.err(),
            Some("code parameters exceed co_varnames")
        );
        dec_ref_bits(py, bits);
        let tuple = alloc_tuple(py, &[]);
        assert_eq!(
            unsafe { code_slots(tuple) }.err(),
            Some("frame binding layout requires a code object")
        );
        dec_ref_bits(py, MoltObject::from_ptr(tuple).bits());
    });
}
