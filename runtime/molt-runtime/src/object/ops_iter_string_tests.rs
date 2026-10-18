use super::{molt_iter, molt_iter_next, molt_iter_next_unboxed, molt_reversed_builtin};
use crate::{MoltObject, PyToken, alloc_string, dec_ref_bits, inc_ref_bits};

// CPython chr/encode('utf-8', 'surrogatepass') observations: adjacent high
// and low surrogates are two characters, distinct from the final non-BMP scalar.
const STORAGE: &[u8] = &[
    0, b'A', 0xc2, 0xa2, 0xe2, 0x82, 0xac, 0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80, 0xef, 0xbf, 0xbd,
    0xf0, 0x9f, 0x98, 0x80,
];
const CHARACTERS: &[&[u8]] = &[
    &[0],
    b"A",
    &[0xc2, 0xa2],
    &[0xe2, 0x82, 0xac],
    &[0xed, 0xa0, 0xbd],
    &[0xed, 0xb8, 0x80],
    &[0xef, 0xbf, 0xbd],
    &[0xf0, 0x9f, 0x98, 0x80],
];

unsafe fn next_owned(py: &PyToken<'_>, iter: u64, unboxed: bool) -> (bool, u64) {
    unsafe {
        if unboxed {
            let mut value = MoltObject::none().bits();
            let done = molt_iter_next_unboxed(iter, (&raw mut value) as usize as u64);
            assert!(!crate::exception_pending(py));
            return (MoltObject::from_bits(done).as_bool().unwrap(), value);
        }
        let pair = molt_iter_next(iter);
        assert!(!crate::exception_pending(py));
        let ptr = MoltObject::from_bits(pair)
            .as_ptr()
            .expect("iteration returns a pair, never an unraised None");
        let (value, done) = crate::object::seq_access::tuple_pair(ptr).expect("iteration pair");
        let done = MoltObject::from_bits(done)
            .as_bool()
            .expect("completion flag");
        inc_ref_bits(py, value);
        dec_ref_bits(py, pair);
        (done, value)
    }
}

unsafe fn assert_character(bits: u64, expected: &[u8]) {
    unsafe {
        let ptr = MoltObject::from_bits(bits).as_ptr().expect("character str");
        assert_eq!(crate::object_type_id(ptr), crate::TYPE_ID_STRING);
        assert_eq!(
            std::slice::from_raw_parts(crate::string_bytes(ptr), crate::string_len(ptr)),
            expected
        );
    }
}

unsafe fn refcount(bits: u64) -> u32 {
    let ptr = MoltObject::from_bits(bits).as_ptr().expect("heap owner");
    unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

#[test]
fn string_iterators_preserve_codepoint_spans_and_exhaustion_in_both_directions() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for storage in [STORAGE, b"".as_slice()] {
                let source = MoltObject::from_ptr(alloc_string(py, storage)).bits();
                assert!(!crate::exception_pending(py));
                for reverse in [false, true] {
                    for unboxed in [false, true] {
                        let iter = if reverse {
                            molt_reversed_builtin(source)
                        } else {
                            molt_iter(source)
                        };
                        assert!(!crate::exception_pending(py));
                        let ptr = MoltObject::from_bits(iter).as_ptr().expect("iterator");
                        let mut cursor = if reverse { storage.len() } else { 0 };
                        let count = if storage.is_empty() {
                            0
                        } else {
                            CHARACTERS.len()
                        };
                        for index in 0..count {
                            let expected =
                                CHARACTERS[if reverse { count - index - 1 } else { index }];
                            let (done, value) = next_owned(py, iter, unboxed);
                            assert!(!done);
                            assert_character(value, expected);
                            dec_ref_bits(py, value);
                            if reverse {
                                cursor -= expected.len();
                                assert_eq!(crate::object::layout::reversed_index(ptr), cursor);
                            } else {
                                cursor += expected.len();
                                assert_eq!(crate::iter_index(ptr), cursor);
                            }
                        }
                        for _ in 0..2 {
                            let (done, value) = next_owned(py, iter, unboxed);
                            assert!(done);
                            assert!(MoltObject::from_bits(value).is_none());
                            dec_ref_bits(py, value);
                        }
                        let target = if reverse {
                            crate::object::layout::reversed_target_bits(ptr)
                        } else {
                            crate::object::layout::iter_target_bits(ptr)
                        };
                        assert!(MoltObject::from_bits(target).is_none());
                        dec_ref_bits(py, iter);
                    }
                }
                dec_ref_bits(py, source);
            }
        }
    });
}

#[test]
fn string_iteration_allocation_failure_matches_cpython_directional_transitions() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    struct ResetTracker;
    impl Drop for ResetTracker {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            // CPython v3.12.15 unicodeobject.c::unicodeiter_next advances before
            // unicode_char; enumobject.c::reversed_next instead clears its seq
            // on any retrieval failure and preserves errors other than EOF.
            let raw_pair = &[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80];
            let source = MoltObject::from_ptr(alloc_string(py, raw_pair)).bits();
            let source_owners = refcount(source);
            for reverse in [false, true] {
                for unboxed in [false, true] {
                    let iter = if reverse {
                        molt_reversed_builtin(source)
                    } else {
                        molt_iter(source)
                    };
                    assert!(!crate::exception_pending(py));
                    assert_eq!(refcount(source), source_owners + 1);
                    let ptr = MoltObject::from_bits(iter).as_ptr().expect("iterator");
                    let reset = ResetTracker;
                    set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                        max_allocations: Some(0),
                        ..ResourceLimits::default()
                    })));
                    let failed = if unboxed {
                        let mut value = MoltObject::none().bits();
                        molt_iter_next_unboxed(iter, (&raw mut value) as usize as u64);
                        value
                    } else {
                        molt_iter_next(iter)
                    };
                    assert!(MoltObject::from_bits(failed).is_none());
                    crate::test_support::assert_and_clear_emergency_memory_error(py);
                    drop(reset);
                    if reverse {
                        assert_eq!(crate::object::layout::reversed_index(ptr), 0);
                        assert!(
                            MoltObject::from_bits(crate::object::layout::reversed_target_bits(ptr))
                                .is_none()
                        );
                        assert_eq!(refcount(source), source_owners);
                    } else {
                        assert_eq!(crate::iter_index(ptr), 3);
                        assert_eq!(crate::object::layout::iter_target_bits(ptr), source);
                        assert_eq!(refcount(source), source_owners + 1);
                        let (done, value) = next_owned(py, iter, unboxed);
                        assert!(!done);
                        assert_character(value, &[0xed, 0xb8, 0x80]);
                        dec_ref_bits(py, value);
                    }
                    for _ in 0..2 {
                        let (done, value) = next_owned(py, iter, unboxed);
                        assert!(done);
                        assert!(MoltObject::from_bits(value).is_none());
                        dec_ref_bits(py, value);
                    }
                    assert_eq!(refcount(source), source_owners);
                    dec_ref_bits(py, iter);
                    assert_eq!(refcount(source), source_owners);
                }
            }
            dec_ref_bits(py, source);
        }
    });
}
